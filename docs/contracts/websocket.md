# Native HTTP WebSocket Contract

`roze-http` provides an application-facing RFC 6455 upgrade and frame API on
the same listener used by generated REST routes. Hyper and
`tokio-websockets` are private implementation details.

## Runtime API

The stable namespace is `roze_http::ws`:

- `WebSocketUpgrade` validates the GET upgrade request and constructs the
  `101 Switching Protocols` response.
- `WebSocket` provides asynchronous `send`, `recv`, and `close`, plus
  `split` for concurrent sending and receiving through `WebSocketSender` and
  `WebSocketReceiver`.
- `Message` covers text, binary, ping, pong, and close messages.
- `CloseFrame` carries an RFC close code and reason.
- `WebSocketConfig` bounds message size, outbound frame size, upgrade time,
  idle time, and close time.
- `WebSocketError` and `WebSocketUpgradeRejection` are non-exhaustive public
  error contracts.

These types and their documented behavior are part of Roze 1.x's
SemVer-governed public Rust API. Applications must not rely on the codec,
Hyper upgrade stream, or internal task representation.

```rust
use roze_http::{ws::{Message, WebSocketUpgrade}, IntoResponse};

async fn websocket(upgrade: WebSocketUpgrade) -> impl IntoResponse {
    upgrade
        .protocols(["chat.v2", "chat.v1"])
        .max_message_size(1024 * 1024)
        .on_upgrade(|mut socket| async move {
            while let Ok(Some(message)) = socket.recv().await {
                match message {
                    Message::Text(text) => {
                        let _ = socket.send(Message::Text(text)).await;
                    }
                    Message::Ping(bytes) => {
                        let _ = socket.send(Message::Pong(bytes)).await;
                    }
                    Message::Close(frame) => {
                        let _ = socket.close(frame).await;
                        break;
                    }
                    Message::Binary(_) | Message::Pong(_) => {}
                }
            }
        })
}
```

`protocols` selects the first server-preferred value requested by the client.
`select_protocol` rejects an unrequested or syntactically invalid protocol.
The handshake validates `Upgrade`, `Connection`, version 13, and a
base64-encoded 16-byte `Sec-WebSocket-Key`.

`with_shutdown` connects a socket to a
`roze_shutdown::ShutdownListener`. Roze sends close code `1001` when the
listener fires and bounds the write by `close_timeout`. Generated services
attach the `ServiceGroup` listener automatically.

The runtime emits bounded `roze_websocket_events_total` series with route and
outcome labels and traces lifecycle/protocol failures without logging message
bodies, authorization headers, keys, or selected credentials.

## Generator Contract

Annotate a GET route with `@websocket`:

```text
service realtime-api {
    @websocket
    @handler realtime
    get /ws
}
```

`rozectl api generate` creates:

- generator-owned route registration and upgrade handler glue under
  `src/route/**` and `src/handler/**`;
- application-owned frame logic under `src/logic/**`.

WebSocket routes use `EmptyReq` and `EmptyResp`, cannot use idempotency
middleware, and are excluded from OpenAPI and ordinary HTTP SDK generation.
`rozectl api generate --update` refreshes the route and handler while
preserving the logic file. Repeating `--update` is idempotent.

The generated logic starts as an echo loop for text/binary messages, answers
ping with pong, and propagates close. Replace it with product semantics; the
file remains application-owned.
