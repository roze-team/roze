# roze-gateway

最小可用网关（Axum + Tower HTTP 转发）示例，覆盖：

- 跨服务路由映射
- 方法约束 / 前缀匹配
- 鉴权（Auth/JWT）
- 路由级限流与熔断
- trace/request-id 透传
- 统一 fallback 与超时控制
- CORS

## 运行

```bash
cargo run -p roze-gateway-app
```

网关监听地址由 `gateway.listen` 决定（示例：`127.0.0.1:8081`）。

## 重载行为

- 配置中心有更新时，网关在主循环中动态替换运行时路由，无需重启。
- 当路由签名与当前一致时，输出 `gateway.config.hot_reloaded.skipped`。
- 当签名变化时，输出 `gateway.config.hot_reloaded` 并刷新路由。

## 网关配置示例

```yaml
gateway:
  listen: "127.0.0.1:8081"
  middlewares:
    - trace
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
```

## 配置中心（可选）

- `ROZE_CONFIG_CENTER_ETCD_ENDPOINTS`：Etcd 端点（`,` 分隔）
- `ROZE_CONFIG_CENTER_ETCD_KEY` / `ROZE_CONFIG_CENTER_KEY`
- `ROZE_CONFIG_CENTER_NAMESPACE` + `ROZE_CONFIG_CENTER_APP`
- `ROZE_CONFIG_CENTER_ENV_KEY`
- `ROZE_CONFIG_CENTER_FILE`
- `ROZE_CONFIG_CENTER_POLL_SECS`
- `ROZE_CONFIG_CENTER_DEBOUNCE_MS`

网关配置中心变更将触发运行时路由重建（无进程重启），并在 `gateway.config.hot_reloaded` 日志事件中可观测。
