# roze-gateway

`roze-gateway-app` 是 Roze native HTTP + Tower 的可运行网关示例。它覆盖：

- 静态 upstream 与 registry 服务发现；
- 方法约束、前缀匹配与 path rewrite；
- JWT / API key 鉴权；
- route/service/global 三级治理；
- timeout、retry、rate limit、breaker、shedding、fallback；
- 主动健康检查、被动 outlier ejection 与加权选址；
- request ID、trace ID 和标准 Context header 传播；
- HTTP、WebSocket 与 SSE；
- 配置中心热更新。

## 运行

```bash
cargo run -p roze-gateway-app
```

默认读取 `apps/roze-gateway/config.yaml`。可用
`ROZE_GATEWAY_CONFIG_FILE=/path/to/config.yaml` 指定其他文件。监听地址由
`gateway.listen` 决定，未配置时使用 `127.0.0.1:8081`。

## 配置

仓库中的 [config.yaml](config.yaml) 是权威示例。核心结构如下：

```yaml
gateway:
  listen: "127.0.0.1:8081"
  stream_idle_timeout_ms: 60000
  max_stream_connections: 1000
  services:
    - name: user
      upstream: "http://127.0.0.1:3000"
      outlier:
        failure_threshold: 3
        ejection_ms: 30000
      health_check:
        path: /healthz
        interval_ms: 10000
        timeout_ms: 1000
        unhealthy_threshold: 3
        healthy_threshold: 1
        expected_status: 200
    # 也可只配置 registry_name，由 registry 动态提供实例。
    # - name: order
    #   registry_name: order-api
  routes:
    - path: /user
      service: user
      methods: [GET, POST]
      rewrite: /user
      retries: 2
      retry_backoff_ms: 100
      middlewares: [trace, rate, breaker, auth]
      rate_limit:
        burst: 20
        refill_ms: 200
      breaker:
        failure_threshold: 5
        reset_timeout_ms: 5000

auth:
  jwt_keys:
    - id: "2026-07"
      secret: "replace-in-production"
  jwt_active_key_id: "2026-07"
  jwt_issuer: "roze"
  jwt_audience: "roze-services"
  api_keys:
    header: "x-api-key"
    keys:
      - key: "replace-in-production"
        subject: "internal-worker"
        roles: ["internal"]
        tenant: "acme"
```

路由 middleware `jwt` 只接受 `Authorization: Bearer <token>`；
`api_key` / `apikey` 只接受配置的 API key header；`auth` 接受两者之一。
生产部署必须从 secret 管理系统注入真实密钥，不能提交明文凭据。

## HTTP、WebSocket 与 SSE

三种协议复用同一套路由、身份、治理、registry 和 Context 传播规则。

- 普通 HTTP 原样转发受允许的方法、headers、body 和上游响应。
- WebSocket 在标准 upgrade 握手后进行双向流量转发。
- 上游返回 `Content-Type: text/event-stream` 时使用流式 SSE body，不等待完整
  response body。
- `stream_idle_timeout_ms` 可在 route、service 或 gateway 层设置；优先级依次
  从具体到全局。
- `max_stream_connections` 同样支持 route、service、gateway 三级配置；容量
  耗尽时返回 429，body 生命周期结束后 permit 由 RAII 释放。

## 热更新

配置中心 reload 只在 `gateway`、`auth`、`governance` 或 `registry` section
变化时重建 runtime：

- 有效变更会原子替换 runtime，并记录 `gateway.config.hot_reloaded`。
- 无关变更记录 `gateway.config.hot_reloaded.skipped`。
- 解析、校验、registry 或 runtime 构建失败时保留最后有效快照，并记录
  `gateway.config.reload.failed`。
- `gateway.listen` 变化需要进程重启，不会在热更新中偷偷更换监听 socket。

可选环境变量：

- `ROZE_CONFIG_CENTER_ETCD_ENDPOINTS`：逗号分隔的 Etcd endpoints。
- `ROZE_CONFIG_CENTER_ETCD_KEY` 或 `ROZE_CONFIG_CENTER_KEY`。
- `ROZE_CONFIG_CENTER_NAMESPACE`、`ROZE_CONFIG_CENTER_APP`、
  `ROZE_CONFIG_CENTER_ENV_KEY`。
- `ROZE_CONFIG_CENTER_FILE`、`ROZE_CONFIG_CENTER_FORMAT`。
- `ROZE_CONFIG_CENTER_POLL_SECS`、`ROZE_CONFIG_CENTER_DEBOUNCE_MS`、
  `ROZE_CONFIG_CENTER_LISTENER_TIMEOUT_MS`。

## Admin 控制面

`roze-admin` 提供可组合的控制面 service 和数据适配器，但当前
`roze-gateway-app` 不会默认把 admin endpoint 暴露到公网 listener。需要控制面
时，应显式构造 `AdminState`、配置 `AdminAuthConfig`，并挂载到受网络策略保护的
内部 listener。详细契约见
[Admin 控制面](../../docs/contracts/admin.md)。

## 验证

```bash
cargo test -p roze-gateway
cargo test -p roze-admin
bash scripts/gateway-smoke.sh
```

真实 registry、故障恢复与长连接 soak 必须在 Linux/Docker 权威环境执行；本地
单元测试不能替代生产证据。
