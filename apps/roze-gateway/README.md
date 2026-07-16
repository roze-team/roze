# roze-gateway

最小可用网关（Roze native HTTP + Tower HTTP 转发）示例，覆盖�?

- 跨服务路由映�?
- 方法约束 / 前缀匹配
- 鉴权（JWT / API Key�?
- 路由级限流与熔断
- trace/request-id 透传
- 统一 fallback 与超时控�?
- HTTP / WebSocket / SSE 同一路由模型
- CORS

## 运行

```bash
cargo run -p roze-gateway-app
```

网关监听地址�?`gateway.listen` 决定（示例：`127.0.0.1:8081`）�?

## 重载行为

- 配置中心有更新时，网关在主循环中动态替换运行时路由，无需重启�?
- 当路由签名与当前一致时，输�?`gateway.config.hot_reloaded.skipped`�?
- 当签名变化时，输�?`gateway.config.hot_reloaded` 并刷新路由�?

## 网关配置示例

```yaml
gateway:
  listen: "127.0.0.1:8081"
  middlewares:
    - trace
  # Optional. Applies to streaming responses such as SSE when no chunk is received.
  stream_idle_timeout_ms: 60000
  # Optional. Limits active SSE/WebSocket connections per route/protocol.
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
    # registry-only upstream:
    # - name: order
    #   registry_name: order-api
  routes:
    - path: /user
      service: user
      methods: [GET, POST]
      rewrite: /user
      retries: 2
      retry_backoff_ms: 100
      middlewares:
        - trace
        - rate
        - breaker
        - auth
      rate_limit:
        burst: 20
        refill_ms: 200
      breaker:
        failure_threshold: 5
        reset_timeout_ms: 5000

auth:
  jwt_keys:
    - id: "2026-07"
      secret: "secret"
  jwt_active_key_id: "2026-07"
  jwt_issuer: "roze"
  jwt_audience: "roze-services"
  api_keys:
    header: "x-api-key"
    keys:
      - key: "service-secret"
        subject: "internal-worker"
        roles: ["internal"]
        tenant: "acme"
```

路由中间�?`jwt` 只接�?`Authorization: Bearer <token>`，`api_key`/`apikey` 只接�?API key header，`auth` 接受 JWT �?API key 任一方式�?

## HTTP、WebSocket、SSE

网关可以同时承载普�?HTTP、WebSocket �?SSE�?

- 普�?HTTP：按 route/service 配置转发 request/response�?
- WebSocket：客户端�?`Upgrade: websocket` 时，网关完成上游握手并做双向流量转发�?
- SSE：上游返�?`Content-Type: text/event-stream` 时，网关自动启用流式响应，不会等待完�?body 结束�?

三者共用同一套路由、鉴权、限流、熔断、超时、fallback、registry upstream �?trace header 机制。SSE 不需要额外配置；只要上游按标准返�?`text/event-stream` 即可。长连接可选配�?`stream_idle_timeout_ms`，当 SSE 长时间没有任何事件或 heartbeat 时由网关主动结束空闲流；也可配置 `max_stream_connections` 限制 SSE/WebSocket 活跃连接数，超限返回 429�?

## 配置中心（可选）

- `ROZE_CONFIG_CENTER_ETCD_ENDPOINTS`：Etcd 端点（`,` 分隔�?
- `ROZE_CONFIG_CENTER_ETCD_KEY` / `ROZE_CONFIG_CENTER_KEY`
- `ROZE_CONFIG_CENTER_NAMESPACE` + `ROZE_CONFIG_CENTER_APP`
- `ROZE_CONFIG_CENTER_ENV_KEY`
- `ROZE_CONFIG_CENTER_FILE`
- `ROZE_CONFIG_CENTER_POLL_SECS`
- `ROZE_CONFIG_CENTER_DEBOUNCE_MS`

网关配置中心变更将触发运行时路由重建（无进程重启），并在 `gateway.config.hot_reloaded` 日志事件中可观测�?

## Admin API

`roze-gateway-app` 默认挂载 `roze-admin` 控制面路由：

- `GET /admin/registry/{service}`：查�?registry 服务实例（仅配置 registry 时可用）�?
- `GET /admin/config/reloads?offset=0&limit=100`：查询配置中�?reload 审计历史�?

未配置对应能力时返回 `404`。可通过环境变量启用内置鉴权�?

- `ROZE_ADMIN_TOKEN`：要�?`Authorization: Bearer <token>`
- `ROZE_ADMIN_API_KEY`：要�?API key header
- `ROZE_ADMIN_API_KEY_HEADER`：API key header 名称，默�?`x-api-key`

未设置上述变量时 Admin 路由不做鉴权，仅适合本地开发或外层已有访问控制的部署�?
