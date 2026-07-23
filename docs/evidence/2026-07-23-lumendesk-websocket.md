# LumenDesk WebSocket Framework Gap Verification

- Date: 2026-07-23
- Baseline revision: `9d670b9a7abb259747cb576d6799080359cb9ef2`
- Reproduction: generated REST service, HTTP and `/ws` on one listener

The application-facing WebSocket gap is resolved in the framework:

- `roze_http::ws::WebSocketUpgrade` owns RFC 6455 validation and the `101`
  response.
- `roze_http::ws::WebSocket` and `Message` own bidirectional frames.
- message/frame bounds, upgrade/idle/close timeouts, subprotocol selection,
  metrics, tracing, cancellation, and graceful ServiceGroup shutdown are
  framework behavior.
- `@websocket` generates route/handler glue and preserves application logic
  through two consecutive update generations.
- WebSocket operations are excluded from OpenAPI and ordinary HTTP SDK output.

Regression coverage includes malformed handshakes, the RFC accept value,
subprotocol negotiation and rejection, text/binary/ping/pong/close transfer,
message limits, idle timeout, shutdown close code `1001`, generator output,
OpenAPI exclusion, repeated update preservation, and generated-project
compile/clippy smoke.

This file records implementation evidence, not long-run production evidence.
Operational soak claims remain governed by `docs/production-evidence.md`.
