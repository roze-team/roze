use std::convert::Infallible;

use http::StatusCode;
use roze_http::rest::{self, IncomingRequest, RestServer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    roze_log::init_tracing();
    let addr = std::env::var("ROZE_DTM_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8090".to_string())
        .parse()?;
    let service = tower::service_fn(|request: IncomingRequest| async move {
        let response = match request.uri().path() {
            "/healthz" => rest::api_response(&roze_result::ApiResponse::ok("ok")),
            _ => rest::text_response(
                StatusCode::NOT_FOUND,
                "DTM route not migrated to Roze native HTTP",
            ),
        };
        Ok::<_, Infallible>(response)
    });
    RestServer::new(addr, service).serve().await?;
    Ok(())
}
