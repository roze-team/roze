# Client address and trusted proxies

Roze REST services can expose both the accepted TCP peer and a policy-resolved
client IP without application code importing Hyper.

Enable peer injection in `config.yaml`:

```yaml
rest:
  connect_info: true
  middlewares:
    trusted_proxy_cidrs:
      - 10.0.0.0/8
      - 2001:db8:ffff::/48
```

Generated entrypoints retain the standard `ServiceGroup` lifecycle and select
`RestServer::with_connect_info()`. Handlers and middleware can extract:

- `roze_http::extract::ConnectInfo<std::net::SocketAddr>` for the direct TCP
  peer;
- `roze_http::client_ip::ClientIp` for the resolved client address.

Forwarding headers are not trusted by default. `X-Forwarded-For` is considered
only when the direct peer matches a configured CIDR. Roze then walks the chain
from right to left, removes trusted proxies, and selects the first untrusted
address. An untrusted peer or malformed chain resolves to the direct peer.
IPv4, IPv4-mapped IPv6, IPv6, bracketed IPv6, and address-with-port forms are
covered by runtime tests.

Configuring `trusted_proxy_cidrs` while `rest.connect_info` is disabled is a
startup error. The accepted peer is never replaced by a forwarding header and
remains independently available for auditing.

The `ConnectInfo`, `ClientIp`, `TrustedProxyConfig`, and
`RestServer::with_connect_info` APIs are public Roze 1.x SemVer contracts.
