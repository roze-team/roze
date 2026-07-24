//! Stable application-facing WebSocket support for Roze native HTTP services.
//!
//! This module owns the HTTP upgrade handshake and frame codec so applications
//! do not need to depend on Hyper or a WebSocket implementation directly.

use std::{
    fmt,
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use bytes::Bytes;
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use http::{
    header::{self, HeaderValue},
    Method, StatusCode,
};
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper_util::rt::TokioIo;
use sha1::{Digest, Sha1};
use tokio::sync::Mutex;
use tokio_websockets::{
    CloseCode as CodecCloseCode, Config as CodecConfig, Limits, Message as CodecMessage,
    ServerBuilder, WebSocketStream,
};

use crate::{
    extract::{ExtractFuture, FromRequest},
    response::IntoResponse,
    rest::{empty_body, HttpResponse, IncomingRequest},
};

const WEBSOCKET_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

type UpgradedIo = TokioIo<Upgraded>;
type CodecSocket = WebSocketStream<UpgradedIo>;
type SocketSink = SplitSink<CodecSocket, CodecMessage>;
type SocketStream = SplitStream<CodecSocket>;

/// Runtime limits and timeout behavior for an upgraded WebSocket connection.
#[derive(Debug, Clone, Copy)]
pub struct WebSocketConfig {
    /// Maximum accepted inbound message size.
    pub max_message_size: usize,
    /// Maximum payload in each outbound frame.
    pub max_frame_size: usize,
    /// Maximum time allowed for Hyper to yield the upgraded connection.
    pub upgrade_timeout: Duration,
    /// Maximum time to wait for the next inbound message.
    pub idle_timeout: Option<Duration>,
    /// Maximum time allowed for a graceful close write.
    pub close_timeout: Duration,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            max_message_size: 16 * 1024 * 1024,
            max_frame_size: 1024 * 1024,
            upgrade_timeout: Duration::from_secs(10),
            idle_timeout: Some(Duration::from_secs(60)),
            close_timeout: Duration::from_secs(5),
        }
    }
}

/// A framework-owned WebSocket message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Bytes),
    Ping(Bytes),
    Pong(Bytes),
    Close(Option<CloseFrame>),
}

impl Message {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    pub fn binary(value: impl Into<Bytes>) -> Self {
        Self::Binary(value.into())
    }

    pub fn ping(value: impl Into<Bytes>) -> Self {
        Self::Ping(value.into())
    }

    pub fn pong(value: impl Into<Bytes>) -> Self {
        Self::Pong(value.into())
    }

    pub fn close(code: u16, reason: impl Into<String>) -> Self {
        Self::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        }))
    }
}

/// RFC 6455 close status and reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseFrame {
    pub code: u16,
    pub reason: String,
}

/// Errors produced after a connection has upgraded.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WebSocketError {
    #[error("WebSocket upgrade timed out")]
    UpgradeTimeout,
    #[error("WebSocket upgrade failed: {0}")]
    Upgrade(String),
    #[error("WebSocket connection was idle for too long")]
    IdleTimeout,
    #[error("WebSocket close timed out")]
    CloseTimeout,
    #[error("invalid WebSocket close code {0}")]
    InvalidCloseCode(u16),
    #[error("WebSocket protocol error: {0}")]
    Protocol(String),
}

/// Rejection returned when a request is not a valid RFC 6455 upgrade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSocketUpgradeRejection {
    status: StatusCode,
    message: &'static str,
}

impl WebSocketUpgradeRejection {
    fn bad_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message,
        }
    }

    fn upgrade_required(message: &'static str) -> Self {
        Self {
            status: StatusCode::UPGRADE_REQUIRED,
            message,
        }
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn message(&self) -> &'static str {
        self.message
    }
}

impl fmt::Display for WebSocketUpgradeRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for WebSocketUpgradeRejection {}

impl IntoResponse for WebSocketUpgradeRejection {
    fn into_response(self) -> HttpResponse {
        let mut builder = http::Response::builder().status(self.status);
        if self.status == StatusCode::UPGRADE_REQUIRED {
            builder = builder.header(header::SEC_WEBSOCKET_VERSION, "13");
        }
        builder
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(crate::rest::full_body(self.message))
            .expect("valid WebSocket rejection response")
    }
}

/// Extractor for an RFC 6455 HTTP upgrade request.
///
/// The extractor validates the handshake immediately. Return [`on_upgrade`](Self::on_upgrade)
/// from the handler to construct the `101 Switching Protocols` response.
pub struct WebSocketUpgrade {
    on_upgrade: OnUpgrade,
    accept: HeaderValue,
    requested_protocols: Vec<String>,
    selected_protocol: Option<HeaderValue>,
    config: WebSocketConfig,
    route: String,
    shutdown: Option<roze_shutdown::ShutdownListener>,
}

/// Request extension used by generated services to attach ServiceGroup shutdown.
#[derive(Clone, Debug)]
pub struct WebSocketShutdown(roze_shutdown::ShutdownListener);

impl WebSocketShutdown {
    pub fn new(listener: roze_shutdown::ShutdownListener) -> Self {
        Self(listener)
    }

    pub fn listener(&self) -> roze_shutdown::ShutdownListener {
        self.0.clone()
    }
}

impl WebSocketUpgrade {
    pub fn config(mut self, config: WebSocketConfig) -> Self {
        self.config = config;
        self
    }

    pub fn max_message_size(mut self, size: usize) -> Self {
        self.config.max_message_size = size;
        self
    }

    pub fn max_frame_size(mut self, size: usize) -> Self {
        self.config.max_frame_size = size;
        self
    }

    pub fn upgrade_timeout(mut self, timeout: Duration) -> Self {
        self.config.upgrade_timeout = timeout;
        self
    }

    pub fn idle_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.config.idle_timeout = timeout;
        self
    }

    pub fn close_timeout(mut self, timeout: Duration) -> Self {
        self.config.close_timeout = timeout;
        self
    }

    /// Attach the service group's shutdown listener.
    ///
    /// When shutdown begins, Roze sends code `1001` and bounds the close write
    /// by [`WebSocketConfig::close_timeout`].
    pub fn with_shutdown(mut self, shutdown: roze_shutdown::ShutdownListener) -> Self {
        self.shutdown = Some(shutdown);
        self
    }

    /// Select the first server-preferred protocol that the client requested.
    pub fn protocols<I, P>(mut self, protocols: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: AsRef<str>,
    {
        self.selected_protocol = protocols
            .into_iter()
            .map(|protocol| protocol.as_ref().to_string())
            .find(|protocol| {
                self.requested_protocols
                    .iter()
                    .any(|requested| requested == protocol)
            })
            .and_then(|protocol| HeaderValue::from_str(&protocol).ok());
        self
    }

    /// Select one exact protocol, rejecting values the client did not request.
    pub fn select_protocol(mut self, protocol: &str) -> Result<Self, WebSocketUpgradeRejection> {
        if !is_protocol_token(protocol) {
            return Err(WebSocketUpgradeRejection::bad_request(
                "invalid WebSocket protocol",
            ));
        }
        if !self
            .requested_protocols
            .iter()
            .any(|requested| requested == protocol)
        {
            return Err(WebSocketUpgradeRejection::bad_request(
                "selected WebSocket protocol was not requested",
            ));
        }
        self.selected_protocol =
            Some(HeaderValue::from_str(protocol).map_err(|_| {
                WebSocketUpgradeRejection::bad_request("invalid WebSocket protocol")
            })?);
        Ok(self)
    }

    pub fn requested_protocols(&self) -> &[String] {
        &self.requested_protocols
    }

    /// Construct the upgrade response and run the application callback.
    pub fn on_upgrade<F, Fut, Output>(self, callback: F) -> HttpResponse
    where
        F: FnOnce(WebSocket) -> Fut + Send + 'static,
        Fut: Future<Output = Output> + Send + 'static,
        Output: Send + 'static,
    {
        let Self {
            on_upgrade,
            accept,
            selected_protocol,
            config,
            route,
            shutdown,
            ..
        } = self;

        let task_route = route.clone();
        tokio::spawn(async move {
            let upgraded = match tokio::time::timeout(config.upgrade_timeout, on_upgrade).await {
                Ok(Ok(upgraded)) => upgraded,
                Ok(Err(error)) => {
                    roze_metrics::record_websocket_event(&task_route, "upgrade_failed");
                    tracing::debug!(
                        route = %task_route,
                        error = %error,
                        "WebSocket upgrade failed"
                    );
                    return;
                }
                Err(_) => {
                    roze_metrics::record_websocket_event(&task_route, "upgrade_timeout");
                    tracing::debug!(route = %task_route, "WebSocket upgrade timed out");
                    return;
                }
            };

            let builder = ServerBuilder::new()
                .config(CodecConfig::default().frame_size(config.max_frame_size.max(1)))
                .limits(Limits::default().max_payload_len(Some(config.max_message_size.max(1))));
            let socket = builder.serve(TokioIo::new(upgraded));
            let (sender, receiver) = socket.split();
            let sender = Arc::new(Mutex::new(sender));
            let websocket = WebSocket {
                sender: sender.clone(),
                receiver,
                idle_timeout: config.idle_timeout,
                close_timeout: config.close_timeout,
                route: task_route.clone(),
                closed: Arc::new(AtomicBool::new(false)),
                peer_closed: Arc::new(AtomicBool::new(false)),
            };

            roze_metrics::record_websocket_event(&task_route, "opened");
            tracing::debug!(route = %task_route, "WebSocket connection opened");
            let callback = callback(websocket);
            if let Some(shutdown) = shutdown {
                tokio::pin!(callback);
                tokio::select! {
                    _ = &mut callback => {}
                    _ = shutdown.wait() => {
                        let close = async {
                            let mut sender = sender.lock().await;
                            sender
                                .send(CodecMessage::close(
                                    Some(CodecCloseCode::GOING_AWAY),
                                    "service shutting down",
                                ))
                                .await
                        };
                        if tokio::time::timeout(config.close_timeout, close).await.is_err() {
                            tracing::debug!(route = %task_route, "WebSocket graceful close timed out");
                        }
                    }
                }
            } else {
                callback.await;
            }
            roze_metrics::record_websocket_event(&task_route, "closed");
            tracing::debug!(route = %task_route, "WebSocket connection closed");
        });

        let mut builder = http::Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_ACCEPT, accept);
        if let Some(protocol) = selected_protocol {
            builder = builder.header(header::SEC_WEBSOCKET_PROTOCOL, protocol);
        }
        builder
            .body(empty_body())
            .expect("valid WebSocket upgrade response")
    }
}

impl FromRequest for WebSocketUpgrade {
    type Rejection = WebSocketUpgradeRejection;

    fn from_request(mut request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        Box::pin(async move {
            validate_upgrade_request(&request)?;
            let accept = websocket_accept(request.headers())?;
            let requested_protocols = requested_protocols(request.headers())?;
            let route = request
                .extensions()
                .get::<crate::extract::MatchedPath>()
                .map(ToString::to_string)
                .unwrap_or_else(|| request.uri().path().to_string());
            let on_upgrade = hyper::upgrade::on(&mut request);
            Ok(Self {
                on_upgrade,
                accept,
                requested_protocols,
                selected_protocol: None,
                config: WebSocketConfig::default(),
                route,
                shutdown: None,
            })
        })
    }
}

/// Framework-owned bidirectional WebSocket stream.
pub struct WebSocket {
    sender: Arc<Mutex<SocketSink>>,
    receiver: SocketStream,
    idle_timeout: Option<Duration>,
    close_timeout: Duration,
    route: String,
    closed: Arc<AtomicBool>,
    peer_closed: Arc<AtomicBool>,
}

impl WebSocket {
    pub async fn send(&mut self, message: Message) -> Result<(), WebSocketError> {
        send_message(&self.sender, message).await
    }

    pub async fn recv(&mut self) -> Result<Option<Message>, WebSocketError> {
        recv_message(&mut self.receiver, self.idle_timeout, &self.peer_closed).await
    }

    pub async fn close(&mut self, frame: Option<CloseFrame>) -> Result<(), WebSocketError> {
        close_socket(
            &self.sender,
            &self.closed,
            &self.peer_closed,
            self.close_timeout,
            frame,
        )
        .await
    }

    pub fn route(&self) -> &str {
        &self.route
    }

    /// Split the socket for independently owned concurrent send and receive tasks.
    pub fn split(self) -> (WebSocketSender, WebSocketReceiver) {
        (
            WebSocketSender {
                sender: self.sender,
                close_timeout: self.close_timeout,
                route: self.route.clone(),
                closed: self.closed,
                peer_closed: self.peer_closed.clone(),
            },
            WebSocketReceiver {
                receiver: self.receiver,
                idle_timeout: self.idle_timeout,
                route: self.route,
                peer_closed: self.peer_closed,
            },
        )
    }
}

/// Cloneable sending half returned by [`WebSocket::split`].
#[derive(Clone)]
pub struct WebSocketSender {
    sender: Arc<Mutex<SocketSink>>,
    close_timeout: Duration,
    route: String,
    closed: Arc<AtomicBool>,
    peer_closed: Arc<AtomicBool>,
}

impl WebSocketSender {
    pub async fn send(&self, message: Message) -> Result<(), WebSocketError> {
        send_message(&self.sender, message).await
    }

    pub async fn close(&self, frame: Option<CloseFrame>) -> Result<(), WebSocketError> {
        close_socket(
            &self.sender,
            &self.closed,
            &self.peer_closed,
            self.close_timeout,
            frame,
        )
        .await
    }

    pub fn route(&self) -> &str {
        &self.route
    }
}

/// Receiving half returned by [`WebSocket::split`].
pub struct WebSocketReceiver {
    receiver: SocketStream,
    idle_timeout: Option<Duration>,
    route: String,
    peer_closed: Arc<AtomicBool>,
}

impl WebSocketReceiver {
    pub async fn recv(&mut self) -> Result<Option<Message>, WebSocketError> {
        recv_message(&mut self.receiver, self.idle_timeout, &self.peer_closed).await
    }

    pub fn route(&self) -> &str {
        &self.route
    }
}

async fn send_message(
    sender: &Arc<Mutex<SocketSink>>,
    message: Message,
) -> Result<(), WebSocketError> {
    let message = to_codec_message(message)?;
    let mut sender = sender.lock().await;
    sender
        .send(message)
        .await
        .map_err(|error| WebSocketError::Protocol(error.to_string()))
}

async fn recv_message(
    receiver: &mut SocketStream,
    idle_timeout: Option<Duration>,
    peer_closed: &AtomicBool,
) -> Result<Option<Message>, WebSocketError> {
    let next = async { receiver.next().await };
    let result = match idle_timeout {
        Some(timeout) => tokio::time::timeout(timeout, next)
            .await
            .map_err(|_| WebSocketError::IdleTimeout)?,
        None => next.await,
    };
    match result {
        Some(Ok(message)) => {
            peer_closed.store(message.is_close(), Ordering::Release);
            Ok(Some(from_codec_message(message)))
        }
        Some(Err(error)) => Err(WebSocketError::Protocol(error.to_string())),
        None => Ok(None),
    }
}

async fn close_socket(
    sender: &Arc<Mutex<SocketSink>>,
    closed: &AtomicBool,
    peer_closed: &AtomicBool,
    close_timeout: Duration,
    frame: Option<CloseFrame>,
) -> Result<(), WebSocketError> {
    if closed.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let peer_closed = peer_closed.load(Ordering::Acquire);
    let close = async {
        let mut sender = sender.lock().await;
        if peer_closed {
            sender
                .flush()
                .await
                .map_err(|error| WebSocketError::Protocol(error.to_string()))
        } else {
            sender
                .send(to_codec_message(Message::Close(frame))?)
                .await
                .map_err(|error| WebSocketError::Protocol(error.to_string()))
        }
    };
    match tokio::time::timeout(close_timeout, close).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            closed.store(false, Ordering::Release);
            Err(error)
        }
        Err(_) => {
            closed.store(false, Ordering::Release);
            Err(WebSocketError::CloseTimeout)
        }
    }
}

fn validate_upgrade_request(request: &IncomingRequest) -> Result<(), WebSocketUpgradeRejection> {
    if request.method() != Method::GET {
        return Err(WebSocketUpgradeRejection::bad_request(
            "WebSocket upgrade requires GET",
        ));
    }
    if !header_eq(request.headers().get(header::UPGRADE), "websocket") {
        return Err(WebSocketUpgradeRejection::upgrade_required(
            "missing or invalid Upgrade header",
        ));
    }
    if !header_contains_token(request.headers().get(header::CONNECTION), "upgrade") {
        return Err(WebSocketUpgradeRejection::upgrade_required(
            "missing or invalid Connection header",
        ));
    }
    if !header_eq(request.headers().get(header::SEC_WEBSOCKET_VERSION), "13") {
        return Err(WebSocketUpgradeRejection::upgrade_required(
            "unsupported WebSocket version",
        ));
    }
    let key = request
        .headers()
        .get(header::SEC_WEBSOCKET_KEY)
        .ok_or_else(|| WebSocketUpgradeRejection::bad_request("missing Sec-WebSocket-Key"))?;
    let decoded = STANDARD
        .decode(key.as_bytes())
        .map_err(|_| WebSocketUpgradeRejection::bad_request("invalid Sec-WebSocket-Key"))?;
    if decoded.len() != 16 {
        return Err(WebSocketUpgradeRejection::bad_request(
            "invalid Sec-WebSocket-Key",
        ));
    }
    Ok(())
}

fn websocket_accept(headers: &http::HeaderMap) -> Result<HeaderValue, WebSocketUpgradeRejection> {
    let key = headers
        .get(header::SEC_WEBSOCKET_KEY)
        .ok_or_else(|| WebSocketUpgradeRejection::bad_request("missing Sec-WebSocket-Key"))?;
    let mut digest = Sha1::new();
    digest.update(key.as_bytes());
    digest.update(WEBSOCKET_GUID);
    HeaderValue::from_str(&STANDARD.encode(digest.finalize()))
        .map_err(|_| WebSocketUpgradeRejection::bad_request("invalid Sec-WebSocket-Key"))
}

fn requested_protocols(
    headers: &http::HeaderMap,
) -> Result<Vec<String>, WebSocketUpgradeRejection> {
    let Some(value) = headers.get(header::SEC_WEBSOCKET_PROTOCOL) else {
        return Ok(Vec::new());
    };
    let value = value
        .to_str()
        .map_err(|_| WebSocketUpgradeRejection::bad_request("invalid WebSocket protocol"))?;
    let mut protocols = Vec::new();
    for protocol in value.split(',').map(str::trim) {
        if !is_protocol_token(protocol) || HeaderValue::from_str(protocol).is_err() {
            return Err(WebSocketUpgradeRejection::bad_request(
                "invalid WebSocket protocol",
            ));
        }
        protocols.push(protocol.to_string());
    }
    Ok(protocols)
}

fn is_protocol_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn header_eq(value: Option<&HeaderValue>, expected: &str) -> bool {
    value
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn header_contains_token(value: Option<&HeaderValue>, expected: &str) -> bool {
    value
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case(expected))
        })
}

fn to_codec_message(message: Message) -> Result<CodecMessage, WebSocketError> {
    match message {
        Message::Text(value) => Ok(CodecMessage::text(value)),
        Message::Binary(value) => Ok(CodecMessage::binary(value)),
        Message::Ping(value) => Ok(CodecMessage::ping(value)),
        Message::Pong(value) => Ok(CodecMessage::pong(value)),
        Message::Close(None) => Ok(CodecMessage::close(None, "")),
        Message::Close(Some(frame)) => {
            let code = CodecCloseCode::try_from(frame.code)
                .map_err(|_| WebSocketError::InvalidCloseCode(frame.code))?;
            Ok(CodecMessage::close(Some(code), &frame.reason))
        }
    }
}

fn from_codec_message(message: CodecMessage) -> Message {
    if message.is_text() {
        return Message::Text(message.as_text().unwrap_or_default().to_string());
    }
    if message.is_binary() {
        return Message::Binary(Bytes::copy_from_slice(message.as_payload()));
    }
    if message.is_ping() {
        return Message::Ping(Bytes::copy_from_slice(message.as_payload()));
    }
    if message.is_pong() {
        return Message::Pong(Bytes::copy_from_slice(message.as_payload()));
    }
    Message::Close(message.as_close().and_then(|(code, reason)| {
        let code = u16::from(code);
        (code != 1005 || !reason.is_empty()).then(|| CloseFrame {
            code,
            reason: reason.to_string(),
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{Request, Uri};
    use tokio::net::TcpListener;
    use tokio_websockets::ClientBuilder;

    use crate::{rest::RestServer, routing::get, Router};

    fn request() -> IncomingRequest {
        Request::builder()
            .method(Method::GET)
            .uri("/ws")
            .header(header::CONNECTION, "keep-alive, Upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==")
            .body(empty_body())
            .expect("request")
    }

    #[test]
    fn validates_handshake_and_rfc_accept_value() {
        let request = request();
        validate_upgrade_request(&request).expect("valid upgrade");
        assert_eq!(
            websocket_accept(request.headers())
                .expect("accept")
                .to_str()
                .expect("text"),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn rejects_malformed_upgrade_requests() {
        let malformed = Request::builder()
            .method(Method::POST)
            .uri("/ws")
            .body(empty_body())
            .expect("request");
        assert_eq!(
            validate_upgrade_request(&malformed)
                .expect_err("invalid upgrade")
                .status(),
            StatusCode::BAD_REQUEST
        );

        let mut request = request();
        request
            .headers_mut()
            .insert(header::SEC_WEBSOCKET_KEY, HeaderValue::from_static("bad"));
        assert!(validate_upgrade_request(&request).is_err());
    }

    #[test]
    fn parses_requested_subprotocols() {
        let mut request = request();
        request.headers_mut().insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static("chat.v2, chat.v1"),
        );
        assert_eq!(
            requested_protocols(request.headers()).expect("protocols"),
            vec!["chat.v2", "chat.v1"]
        );
    }

    #[tokio::test]
    async fn rejects_unrequested_selected_subprotocol() {
        async fn reject(upgrade: WebSocketUpgrade) -> HttpResponse {
            match upgrade.select_protocol("chat.v2") {
                Ok(upgrade) => upgrade.on_upgrade(|_| async {}),
                Err(rejection) => rejection.into_response(),
            }
        }

        let addr = test_addr().await;
        let (shutdown, listener) = roze_shutdown::channel();
        let server = tokio::spawn(
            RestServer::new(addr, Router::new().route("/ws", get(reject)))
                .serve_with_shutdown(listener.wait()),
        );
        let uri: Uri = format!("ws://{addr}/ws").parse().expect("URI");
        let error = ClientBuilder::from_uri(uri)
            .add_header(
                header::SEC_WEBSOCKET_PROTOCOL,
                HeaderValue::from_static("chat.v1"),
            )
            .expect("valid subprotocol header")
            .connect()
            .await
            .expect_err("unrequested selection must fail");
        assert!(error.to_string().contains("400"));

        shutdown.trigger();
        server.await.expect("server task").expect("server result");
    }

    #[test]
    fn converts_all_public_frame_types() {
        for message in [
            Message::text("hello"),
            Message::binary(Bytes::from_static(b"bytes")),
            Message::ping(Bytes::from_static(b"ping")),
            Message::pong(Bytes::from_static(b"pong")),
            Message::close(1000, "done"),
            Message::Close(None),
        ] {
            let round_trip =
                from_codec_message(to_codec_message(message.clone()).expect("codec message"));
            assert_eq!(round_trip, message);
        }
    }

    async fn test_addr() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local address");
        drop(listener);
        addr
    }

    async fn connect_with_retry(
        uri: Uri,
        protocol: Option<&'static str>,
    ) -> (
        tokio_websockets::WebSocketStream<tokio_websockets::MaybeTlsStream<tokio::net::TcpStream>>,
        tokio_websockets::upgrade::Response,
    ) {
        let mut last_error = None;
        for _ in 0..20 {
            let mut builder = ClientBuilder::from_uri(uri.clone());
            if let Some(protocol) = protocol {
                builder = builder
                    .add_header(
                        header::SEC_WEBSOCKET_PROTOCOL,
                        HeaderValue::from_static(protocol),
                    )
                    .expect("valid subprotocol header");
            }
            match builder.connect().await {
                Ok(connection) => return connection,
                Err(error) => {
                    last_error = Some(error);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
        panic!("client failed to connect: {last_error:?}");
    }

    #[tokio::test]
    async fn upgrades_and_transfers_bidirectional_frames() {
        async fn echo(upgrade: WebSocketUpgrade) -> HttpResponse {
            upgrade
                .protocols(["chat.v2", "chat.v1"])
                .on_upgrade(|socket| async move {
                    let (sender, mut receiver) = socket.split();
                    while let Ok(Some(message)) = receiver.recv().await {
                        match message {
                            Message::Text(text) => {
                                sender
                                    .send(Message::Text(format!("echo:{text}")))
                                    .await
                                    .expect("send text");
                            }
                            Message::Binary(bytes) => {
                                sender
                                    .send(Message::Binary(bytes))
                                    .await
                                    .expect("send binary");
                            }
                            Message::Ping(bytes) => {
                                sender.send(Message::Pong(bytes)).await.expect("send pong");
                            }
                            Message::Close(frame) => {
                                sender.close(frame).await.expect("close");
                                break;
                            }
                            Message::Pong(_) => {}
                        }
                    }
                })
        }

        let addr = test_addr().await;
        let (shutdown, listener) = roze_shutdown::channel();
        let server = tokio::spawn(
            RestServer::new(addr, Router::new().route("/ws", get(echo)))
                .serve_with_shutdown(listener.wait()),
        );
        let uri: Uri = format!("ws://{addr}/ws").parse().expect("URI");
        let (mut client, response) = connect_with_retry(uri, Some("chat.v1")).await;
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(
            response.headers().get(header::SEC_WEBSOCKET_PROTOCOL),
            Some(&HeaderValue::from_static("chat.v1"))
        );

        client
            .send(CodecMessage::text("hello"))
            .await
            .expect("send text");
        let text = client
            .next()
            .await
            .expect("text response")
            .expect("valid text response");
        assert_eq!(text.as_text(), Some("echo:hello"));

        client
            .send(CodecMessage::binary(&b"\x01\x02"[..]))
            .await
            .expect("send binary");
        let binary = client
            .next()
            .await
            .expect("binary response")
            .expect("valid binary response");
        assert_eq!(binary.as_payload().to_vec(), b"\x01\x02");

        client
            .send(CodecMessage::ping(&b"alive"[..]))
            .await
            .expect("send ping");
        let pong = client
            .next()
            .await
            .expect("pong response")
            .expect("valid pong response");
        assert!(pong.is_pong());
        assert_eq!(pong.as_payload().to_vec(), b"alive");

        client
            .send(CodecMessage::close(
                Some(CodecCloseCode::NORMAL_CLOSURE),
                "done",
            ))
            .await
            .expect("send close");
        let mut propagated_close = None;
        for _ in 0..3 {
            let Some(close) = client.next().await else {
                break;
            };
            let close = close.expect("valid close response");
            if let Some((code, reason)) = close.as_close() {
                propagated_close = Some((u16::from(code), reason.to_string()));
                break;
            }
        }
        assert_eq!(propagated_close.as_ref().map(|(code, _)| *code), Some(1000));

        shutdown.trigger();
        server.await.expect("server task").expect("server result");
    }

    #[tokio::test]
    async fn enforces_message_size_and_idle_timeout() {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let result_tx = Arc::new(Mutex::new(Some(result_tx)));
        let handler_tx = result_tx.clone();
        let router = Router::new().route(
            "/ws",
            get(move |upgrade: WebSocketUpgrade| {
                let handler_tx = handler_tx.clone();
                async move {
                    upgrade
                        .max_message_size(4)
                        .idle_timeout(Some(Duration::from_millis(100)))
                        .on_upgrade(move |mut socket| async move {
                            let result = socket.recv().await;
                            if let Some(sender) = handler_tx.lock().await.take() {
                                let _ = sender.send(result);
                            }
                        })
                }
            }),
        );
        let addr = test_addr().await;
        let (shutdown, listener) = roze_shutdown::channel();
        let server =
            tokio::spawn(RestServer::new(addr, router).serve_with_shutdown(listener.wait()));
        let uri: Uri = format!("ws://{addr}/ws").parse().expect("URI");
        let (mut client, _) = connect_with_retry(uri, None).await;
        client
            .send(CodecMessage::text("too large"))
            .await
            .expect("send oversized message");
        let result = result_rx.await.expect("server result");
        assert!(matches!(result, Err(WebSocketError::Protocol(_))));

        shutdown.trigger();
        server.await.expect("server task").expect("server result");

        let (idle_tx, idle_rx) = tokio::sync::oneshot::channel();
        let idle_tx = Arc::new(Mutex::new(Some(idle_tx)));
        let handler_tx = idle_tx.clone();
        let router = Router::new().route(
            "/ws",
            get(move |upgrade: WebSocketUpgrade| {
                let handler_tx = handler_tx.clone();
                async move {
                    upgrade
                        .idle_timeout(Some(Duration::from_millis(25)))
                        .on_upgrade(move |mut socket| async move {
                            let result = socket.recv().await;
                            if let Some(sender) = handler_tx.lock().await.take() {
                                let _ = sender.send(result);
                            }
                        })
                }
            }),
        );
        let addr = test_addr().await;
        let (shutdown, listener) = roze_shutdown::channel();
        let server =
            tokio::spawn(RestServer::new(addr, router).serve_with_shutdown(listener.wait()));
        let uri: Uri = format!("ws://{addr}/ws").parse().expect("URI");
        let (_client, _) = connect_with_retry(uri, None).await;
        assert!(matches!(
            idle_rx.await.expect("idle result"),
            Err(WebSocketError::IdleTimeout)
        ));
        shutdown.trigger();
        server.await.expect("server task").expect("server result");
    }

    #[tokio::test]
    async fn service_shutdown_sends_going_away_close() {
        let addr = test_addr().await;
        let (shutdown, listener) = roze_shutdown::channel();
        let websocket_shutdown = listener.clone();
        let router = Router::new().route(
            "/ws",
            get(move |upgrade: WebSocketUpgrade| {
                let websocket_shutdown = websocket_shutdown.clone();
                async move {
                    upgrade
                        .with_shutdown(websocket_shutdown)
                        .on_upgrade(|mut socket| async move {
                            while socket.recv().await.is_ok_and(|message| message.is_some()) {}
                        })
                }
            }),
        );
        let server = tokio::spawn(
            RestServer::new(addr, router).serve_with_shutdown(listener.clone().wait()),
        );
        let uri: Uri = format!("ws://{addr}/ws").parse().expect("URI");
        let (mut client, _) = connect_with_retry(uri, None).await;

        shutdown.trigger();
        let close = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let message = client
                    .next()
                    .await
                    .expect("close response")
                    .expect("valid close response");
                if let Some((code, _)) = message.as_close() {
                    break u16::from(code);
                }
            }
        })
        .await
        .expect("shutdown close timeout");
        assert_eq!(close, 1001);
        server.await.expect("server task").expect("server result");
    }
}
