use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

use http::{request::Parts as HttpParts, HeaderMap};
use ipnet::IpNet;
use thiserror::Error;

use crate::{
    extract::{
        ExtractFuture, FromRequest, FromRequestParts, OptionalFromRequest, OptionalFromRequestParts,
    },
    IncomingRequest,
};

/// The client address selected from the TCP peer and a trusted proxy chain.
///
/// This extension is inserted by Roze's trusted-proxy middleware. The TCP peer
/// remains independently available as [`crate::extract::ConnectInfo`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClientIp(pub IpAddr);

impl std::ops::Deref for ClientIp {
    type Target = IpAddr;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for ClientIp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromRequest for ClientIp {
    type Rejection = roze_error::RozeError;

    fn from_request(request: IncomingRequest) -> ExtractFuture<'static, Self, Self::Rejection> {
        let client_ip = request.extensions().get::<Self>().copied();
        Box::pin(async move {
            client_ip.ok_or_else(|| {
                roze_error::RozeError::Internal(
                    "missing client IP; enable rest.connect_info".to_string(),
                )
            })
        })
    }
}

impl FromRequestParts for ClientIp {
    type Rejection = roze_error::RozeError;

    fn from_request_parts(parts: &mut HttpParts) -> ExtractFuture<'_, Self, Self::Rejection> {
        let client_ip = parts.extensions.get::<Self>().copied();
        Box::pin(async move {
            client_ip.ok_or_else(|| {
                roze_error::RozeError::Internal(
                    "missing client IP; enable rest.connect_info".to_string(),
                )
            })
        })
    }
}

impl OptionalFromRequest for ClientIp {
    type Rejection = std::convert::Infallible;

    fn optional_from_request(
        request: IncomingRequest,
    ) -> ExtractFuture<'static, Option<Self>, Self::Rejection> {
        let client_ip = request.extensions().get::<Self>().copied();
        Box::pin(async move { Ok(client_ip) })
    }
}

impl OptionalFromRequestParts for ClientIp {
    type Rejection = std::convert::Infallible;

    fn optional_from_request_parts(
        parts: &mut HttpParts,
    ) -> ExtractFuture<'_, Option<Self>, Self::Rejection> {
        let client_ip = parts.extensions.get::<Self>().copied();
        Box::pin(async move { Ok(client_ip) })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustedProxyConfig {
    networks: Vec<IpNet>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TrustedProxyConfigError {
    #[error("trusted proxy CIDR `{value}` is invalid")]
    InvalidCidr { value: String },
}

impl TrustedProxyConfig {
    pub fn new<I, S>(networks: I) -> Result<Self, TrustedProxyConfigError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let networks = networks
            .into_iter()
            .map(|value| {
                let value = value.as_ref().trim();
                IpNet::from_str(value).map_err(|_| TrustedProxyConfigError::InvalidCidr {
                    value: value.to_string(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { networks })
    }

    pub fn is_empty(&self) -> bool {
        self.networks.is_empty()
    }

    pub fn is_trusted(&self, address: IpAddr) -> bool {
        self.networks
            .iter()
            .any(|network| network.contains(&address))
    }

    /// Resolves the effective client address by removing trusted proxies from
    /// the right side of `X-Forwarded-For`.
    ///
    /// Forwarding headers are ignored unless the direct TCP peer is trusted.
    /// A malformed chain also fails closed to the direct peer address.
    pub fn resolve(&self, peer: SocketAddr, headers: &HeaderMap) -> ClientIp {
        let peer_ip = normalize_ip(peer.ip());
        if !self.is_trusted(peer_ip) {
            return ClientIp(peer_ip);
        }

        let Some(value) = headers.get(http::header::HeaderName::from_static("x-forwarded-for"))
        else {
            return ClientIp(peer_ip);
        };
        let Ok(value) = value.to_str() else {
            return ClientIp(peer_ip);
        };
        let Some(mut chain) = parse_forwarded_chain(value) else {
            return ClientIp(peer_ip);
        };
        if chain.is_empty() {
            return ClientIp(peer_ip);
        }

        chain.push(peer_ip);
        let mut selected = peer_ip;
        for address in chain.into_iter().rev() {
            selected = address;
            if !self.is_trusted(address) {
                break;
            }
        }
        ClientIp(selected)
    }
}

fn parse_forwarded_chain(value: &str) -> Option<Vec<IpAddr>> {
    value
        .split(',')
        .map(|part| parse_forwarded_ip(part.trim()))
        .collect()
}

fn parse_forwarded_ip(value: &str) -> Option<IpAddr> {
    if value.is_empty() || value.eq_ignore_ascii_case("unknown") {
        return None;
    }
    let value = value.trim_matches('"');
    if let Ok(address) = value.parse::<IpAddr>() {
        return Some(normalize_ip(address));
    }
    if let Ok(address) = value.parse::<SocketAddr>() {
        return Some(normalize_ip(address.ip()));
    }
    let bracketed = value
        .strip_prefix('[')
        .and_then(|value| value.split_once(']'))
        .map(|(address, _)| address)?;
    bracketed.parse::<IpAddr>().ok().map(normalize_ip)
}

fn normalize_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(address)),
        address => address,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_str(value).expect("header"),
        );
        headers
    }

    #[test]
    fn untrusted_peer_cannot_spoof_forwarded_chain() {
        let config = TrustedProxyConfig::new(["10.0.0.0/8"]).expect("config");
        let peer = "203.0.113.9:443".parse().expect("peer");
        assert_eq!(
            config.resolve(peer, &headers("198.51.100.10")).0,
            "203.0.113.9".parse::<IpAddr>().expect("ip")
        );
    }

    #[test]
    fn strips_multiple_trusted_proxies_from_the_right() {
        let config = TrustedProxyConfig::new(["10.0.0.0/8", "2001:db8:ffff::/48"]).expect("config");
        let peer = "[2001:db8:ffff::2]:443".parse().expect("peer");
        assert_eq!(
            config.resolve(peer, &headers("198.51.100.7, 10.2.0.4")).0,
            "198.51.100.7".parse::<IpAddr>().expect("ip")
        );
    }

    #[test]
    fn supports_ipv6_clients_and_bracketed_proxy_values() {
        let config = TrustedProxyConfig::new(["10.0.0.0/8"]).expect("config");
        let peer = "10.0.0.2:443".parse().expect("peer");
        assert_eq!(
            config.resolve(peer, &headers("[2001:db8::42]:8443")).0,
            "2001:db8::42".parse::<IpAddr>().expect("ip")
        );
    }

    #[test]
    fn malformed_forwarded_chain_fails_closed_to_peer() {
        let config = TrustedProxyConfig::new(["10.0.0.0/8"]).expect("config");
        let peer = "10.0.0.2:443".parse().expect("peer");
        assert_eq!(
            config.resolve(peer, &headers("198.51.100.7, nope")).0,
            "10.0.0.2".parse::<IpAddr>().expect("ip")
        );
    }
}
