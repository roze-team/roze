# Roze 网关（gateway/http）契约（v1）

本文档定义 `apps/roze-gateway` 与 `crates/roze-gateway` 的最小运行契约，目标是提供 Roze 原生的 gateway/http 可用行为集。

## 1. 入口约定

- 网关请求按 `GatewayConfig.routes` 做前缀匹配，匹配策略为“最长路径优先”。
- 路由方法默认值：
  - 空数组表示全部方法；
  - `"*"` 或 `"all"` 表示全部方法。
- 路由优先级：全局中间件参数 + 路由级中间件参数按以下顺序执行（语义顺序）  
  `trace -> auth -> rate -> breaker -> shedding -> timeout -> upstream`

## 2. 配置结构（关键字段）

```yaml
gateway:
  listen: "127.0.0.1:8081"
  middlewares: [trace, ...]
  timeout_ms: 8000
  stream_idle_timeout_ms: 60000
  max_stream_connections: 1000
  request_body_limit_bytes: 1048576
  services:
    - name: user
      upstream: http://127.0.0.1:3000
      timeout_ms: 3000
      stream_idle_timeout_ms: 60000
      max_stream_connections: 500
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
      instance_tags: { env: prod }
      timeout_ms: 3000
  routes:
    - path: /user
      service: user
      methods: [GET, POST]
      weight: 90
      rewrite: /user
      stream_idle_timeout_ms: 60000
      max_stream_connections: 100
      fallback: { status: 503, body: { code: 503, message: "..." }, headers: {...} }
      middlewares: [trace, rate, breaker, auth]
      rate_limit: { burst: 20, refill_ms: 200 }
      breaker: { failure_threshold: 5, reset_timeout_ms: 5000 }
    - path: /user
      service: user-v2
      methods: [GET, POST]
      weight: 10
      instance_tags: { version: v2 }
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
- Gateway route 显式字段优先于 `governance.routes`，`governance.routes` 优先于全局 `governance`。
- Gateway 当前继承的统一治理字段包括 `timeout_ms`、`retry`、`rate_limit`、`breaker`、`shedding` 和 `fallback`。
- `stream_idle_timeout_ms` 是流式响应空闲超时，按 `route > service > gateway` 覆盖；未配置时不额外限制流式 body。
- `max_stream_connections` 是 SSE/WebSocket 活跃连接数上限，按 `route > service > gateway` 覆盖；未配置时不额外限制长连接数，超限返回 429。

## 3. 鉴权

- `middlewares` 命中 `jwt` 时，要求请求携带 `Authorization: Bearer <token>`，并使用 `roze-jwt::verify_token` 验签。
- `middlewares` 命中 `api_key` 或 `apikey` 时，要求请求携带 `auth.api_keys.header` 指定的 API key header，默认 `x-api-key`。
- `middlewares` 命中 `auth` 时，允许 JWT 或 API key 任一方式通过。
- `jwt` 配置缺失、`api_keys` 配置缺失或凭据校验失败时会返回 401。
- 鉴权成功后，网关会向上游注入标准 auth context header：subject、roles、tenant。

```yaml
auth:
  jwt_keys:
    - id: "2026-07"
      secret: "secret"
  jwt_active_key_id: "2026-07"
  jwt_issuer: "roze"
  jwt_audience: "roze-services"
  jwt_clock_skew_secs: 30
  api_keys:
    header: "x-api-key"
    keys:
      - key: "service-secret"
        subject: "internal-worker"
        roles: ["internal"]
        tenant: "acme"
```

JWT headers must contain a trusted `kid`. Rotation keeps old verification keys
in `jwt_keys` while changing `jwt_active_key_id`; issuer, audience, expiry,
clock skew, and `jti` revocation are enforced on every gateway verification.

Gateway routes additionally support `match_headers`, `match_cookies`,
`traffic_percent`, `mirror_service`, and `mirror_percent`. Ordered duplicate
paths provide deterministic canary, blue-green, and A/B routing. Mirrored
requests run independently of the primary response and invalid percentages or
unknown mirror services fail config validation.

## 4. 路由与转发

- 默认转发 preserve path，支持 `rewrite`：
  - `route.rewrite` 若存在则用重写后的前缀替换匹配前缀；
  - 无 `rewrite` 则保留原始请求路径。
- 多条路由匹配同一路径和方法时，先按最长路径分组，再按 `route.weight` 做稳定加权选择；可用于 `v1:90 / v2:10` 灰度路由。
- 上游可配置静态 `service.upstream`，也可配置 `service.registry_name` 从注册中心动态发现实例；两者同时存在时，`registry_name` 优先，静态 upstream 作为未启用动态发现时的默认路径。
- registry 动态实例支持 `weight` 和 `metadata`：
  - `weight` 影响网关实例轮询顺序，用于蓝绿/金丝雀流量比例；
  - `service.instance_tags` 和 `route.instance_tags` 会合并后过滤实例 metadata，route 级同名 key 覆盖 service 级 key；
  - 若配置了标签且没有匹配实例，网关不会回退到无标签实例，避免灰度流量误打到错误版本。
- 注册中心发现由 `CachedRegistryResolver` 维护本地实例快照；etcd registry 支持原生 `/v3/watch`，实例变更会即时刷新缓存，周期 refresh 作为 watch 断线兜底。
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
  - 未设置 `retries` 时继承 `governance.routes.<path>.retry.max_attempts` 或 `governance.retry.max_attempts`，并转换为额外重试次数；
  - 未设置 `retry_backoff_ms` 时继承 `governance.routes.<path>.retry.backoff_ms` 或 `governance.retry.backoff_ms`；
  - 每次尝试独立应用 `timeout_ms`，全部尝试失败后才触发 breaker failure 记录。
- 自动透传请求头、body 与 query-string，保留 `x-request-id`、`x-trace-id`（若未提供则自动补齐）。
- 默认请求体上限：2MB（可配置 `request_body_limit_bytes`）。

### HTTP、WebSocket、SSE

- 普通 HTTP：网关按 route/service 转发请求并透传上游响应。
- WebSocket：客户端发送 `Upgrade: websocket` 时，网关会对上游发起 WebSocket 握手，并在握手成功后做双向字节流转发。
- SSE：上游响应 `Content-Type: text/event-stream` 时，网关使用流式响应透传事件，不等待完整 body 结束。
- SSE 启用不需要额外配置。建议上游定期发送 heartbeat，例如 `: ping\n\n`。
- `timeout_ms` 约束普通请求和上游响应头等待时间；`stream_idle_timeout_ms` 只约束 SSE 等流式响应在两次 chunk 之间允许空闲的最长时间。
- `max_stream_connections` 同时约束 SSE 和 WebSocket 的活跃连接数，按 route + protocol 分开计数。
- 长连接观测指标：
  - `roze_gateway_stream_connection_events_total{service,route,protocol,outcome}`：SSE/WS opened、closed、rejected 事件数。
  - `roze_gateway_stream_connections_active{service,route,protocol}`：当前活跃 SSE/WS 连接数。
  - `roze_gateway_stream_connection_duration_ms_total{service,route,protocol}`：已关闭 SSE/WS 连接累计持续时间。

## 5. 熔断与限流

- `rate_limit` 使用令牌桶模型：
  - `burst`：可用 tokens 上限
  - `refill_ms`：刷新窗口
- `breaker` 失败计数满阈值后开启，持续 `reset_timeout_ms` 期间直接返回 503。
- `shedding` 使用路由级并发上限做第一阶段负载保护；活跃请求数达到 `concurrency` 时直接返回 429。
- `fallback` 可由 `route.fallback`、`governance.routes`、`governance` 或 `gateway.fallback` 提供，按显式路由、路由治理、全局治理、网关默认的顺序选择；可配置 `status`、`body` 和 `headers`。

## 6. 可观测字段（tracing）

- `gateway.no_route`
- `gateway.method_not_allowed`
- `gateway.auth_failed`
- `gateway.rate_limited`
- `gateway.breaker_open`
- `gateway.request_body_invalid`
- `gateway.upstream_failed`
- `gateway.upstream_timeout`
- `gateway.websocket_failed`
- `gateway.websocket_timeout`
- `gateway.stream_connection_opened`
- `gateway.stream_connection_closed`
- `gateway.stream_connection_rejected`
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
