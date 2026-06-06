//! gRPC transport helpers built on tonic.

use tonic::transport::{Channel, Endpoint};

pub fn normalize_endpoint(addr: &str) -> anyhow::Result<String> {
    if addr.starts_with("http://") || addr.starts_with("https://") {
        Ok(addr.to_string())
    } else {
        Ok(format!("http://{addr}"))
    }
}

pub async fn connect(addr: impl AsRef<str>) -> anyhow::Result<Channel> {
    let url = normalize_endpoint(addr.as_ref())?;
    Ok(Endpoint::from_shared(url)?.connect().await?)
}
