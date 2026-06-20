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
      outlier: { failure_threshold: 3, ejection_ms: 30000 }
      health_check:
        path: /healthz
        interval_ms: 10000
        timeout_ms: 1000
        unhealthy_threshold: 3
        healthy_threshold: 1
        expected_status: 200
    - name: order
      registry_name: order-api
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
- 上游可配置静态 `service.upstream`，也可配置 `service.registry_name` 从注册中心动态发现实例；两者同时存在时，`registry_name` 优先，静态 upstream 作为未启用动态发现时的默认路径。
- `service.outlier` 开启实例级被动摘除：
  - `failure_threshold`：同一实例连续失败阈值，默认 3；
  - `ejection_ms`：摘除时长，默认 30000；
  - 当前失败信号包含上游连接错误和 5xx 响应；若全部实例都处于摘除窗口，会临时允许全集合参与选择，避免服务完全不可达。
- `service.health_check` 开启实例级主动健康检查：
  - `path`：探测路径，默认 `/healthz`；
  - `interval_ms`：探测周期，默认 10000；
  - `timeout_ms`：单次探测超时，默认 1000；
  - `unhealthy_threshold`：连续失败多少次后标记不健康，默认 3；
  - `healthy_threshold`：连续成功多少次后恢复健康，默认 1；
  - `expected_status`：期望 HTTP 状态码，默认 200；
  - registry 服务会周期性发现当前实例并探测，路由选择会跳过不健康实例；若全部实例都不健康，会临时允许全集合参与选择，避免探测误判导致整体不可达。
- 支持路由级重试：
  - `retries`：失败后的额外重试次数，默认 0；
  - `retry_backoff_ms`：每次重试前的等待时间，默认 0；
  - 每次尝试独立应用 `timeout_ms`，全部尝试失败后才触发 breaker failure 记录。
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
- `gateway.upstream_retry_succeeded`
- `gateway.upstream_ejected`
- `gateway.upstream_unhealthy`
- `gateway.upstream_recovered`
- `gateway.health_check_discover_failed`

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
