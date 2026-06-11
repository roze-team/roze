# Roze 网关（gateway/http）契约（v1）

本文档定义 `apps/roze-gateway` 与 `crates/roze-gateway` 的最小运行契约，目标对齐 go-zero gateway/http 可用行为集。

## 1. 入口约定

- 网关请求按 `GatewayConfig.routes` 做前缀匹配，匹配策略为“最长路径优先”。
- 路由方法默认值：
  - 空数组表示全部方法；
  - `"*"` 或 `"all"` 表示全部方法。
- 路由优先级：全局中间件参数 + 路由级中间件参数按以下顺序执行（语义顺序）  
  `trace -> auth -> rate -> breaker -> timeout -> upstream`

## 2. 配置结构（关键字段）

```yaml
gateway:
  listen: "127.0.0.1:8081"
  middlewares: [trace, ...]
  timeout_ms: 8000
  request_body_limit_bytes: 1048576
  services:
    - name: user
      upstream: http://127.0.0.1:3000
      timeout_ms: 3000
  routes:
    - path: /user
      service: user
      methods: [GET, POST]
      rewrite: /user
      fallback: { status: 503, body: { code: 503, message: "..." }, headers: {...} }
      middlewares: [trace, rate, breaker, auth]
      rate_limit: { burst: 20, refill_ms: 200 }
      breaker: { failure_threshold: 5, reset_timeout_ms: 5000 }
```

### fallback

- 404：无路由匹配
- 405：方法不匹配
- 429：限流触发
- 503：熔断开启
- 400：请求体读取失败/超限
- 502：上游转发失败
- 504：上游超时

字段覆盖策略：

- `route.fallback` 优先于 `gateway.fallback`，未设置时仅使用 HTTP code + message。

## 3. 鉴权

- `middlewares` 命中 `auth` 或 `jwt` 时，要求请求携带 `Authorization: Bearer <token>`。
- `jwt` 配置缺失时会返回 401（在网关层报错）。
- 成功后执行标准 JWT 验签（`roze-jwt::verify_token`）。

## 4. 路由与转发

- 默认转发 preserve path，支持 `rewrite`：
  - `route.rewrite` 若存在则用重写后的前缀替换匹配前缀；
  - 无 `rewrite` 则保留原始请求路径。
- 自动透传请求头、body 与 query-string，保留 `x-request-id`、`x-trace-id`（若未提供则自动补齐）。
- 默认请求体上限：2MB（可配置 `request_body_limit_bytes`）。

## 5. 熔断与限流

- `rate_limit` 使用令牌桶模型：
  - `burst`：可用 tokens 上限
  - `refill_ms`：刷新窗口
- `breaker` 失败计数满阈值后开启，持续 `reset_timeout_ms` 期间直接返回 503。

## 6. 可观测字段（tracing）

- `gateway.no_route`
- `gateway.method_not_allowed`
- `gateway.auth_failed`
- `gateway.rate_limited`
- `gateway.breaker_open`
- `gateway.request_body_invalid`
- `gateway.upstream_failed`
- `gateway.upstream_timeout`

网关配置热更新相关事件：

- `gateway.config.hot_reloaded`：签名变化后重建路由。
- `gateway.config.hot_reloaded.skipped`：配置签名未变，跳过重建。
- `gateway.config.reload.applied`：配置中心应用成功。
- `gateway.config.reload.failed`：配置中心读取/解析失败（服务继续保留旧配置）。

每个事件建议补充：

- `method`、`path`、`route`、`app/topic`、`error`（如有）、`timeout_ms`（如有）

推荐补充字段（`gateway.config.hot_reloaded`）：

- `listen`
- `signature`
- `version`/`hash`
