# Configuration secrets and production stores

`roze_config::load` resolves secret references before typed deserialization and
validation:

```yaml
auth:
  jwt_keys:
    - id: platform-2026-07
      secret: env://PLATFORM_ADMIN_JWT_SECRET
    - id: merchant-2026-07
      secret: file://secrets/merchant-jwt
  jwt_active_key_id: platform-2026-07
```

Supported built-in references are `env://NAME`, `${NAME}`, and `file://path`.
Relative file paths resolve beside the primary configuration file and trailing
line endings are removed. Applications can call
`load_with_secret_provider` with a custom `SecretProvider`; unsupported
references must return `None`.

AI provider keys use the same resolver and are never loaded directly by
`roze-ai`:

```yaml
ai:
  default_provider: default
  providers:
    default:
      kind: openai_compatible
      base_url: https://api.openai.com/v1
      api_key: ${OPENAI_API_KEY}
      model: replace-with-your-model
```

Resolved keys are redacted from `AiProviderConfig` and `ServiceConfig` debug
output. Provider base URLs containing embedded usernames or passwords are
rejected.

Roze re-exports Veil's derive macros through `roze_config::redaction`, and
generated service manifests include Veil with default features disabled. The
runtime `toggle` feature is therefore absent, so production redaction cannot be
disabled by an environment variable. Application crates using the derive must
keep `veil` as a direct dependency because its generated code references the
crate. Use fixed-length masking for secrets:

```rust
use roze_config::redaction::Redact;

#[derive(Redact)]
struct ProviderCredentials {
    name: String,
    #[redact(fixed = 12)]
    api_key: String,
}
```

This contract protects only `Debug` formatting such as `tracing::info!(?value)`.
`Display` fields (`%value`), Serde output, response bodies, and storage remain
separate security boundaries; never log a secret field directly.

Generated REST/RPC services also load a typed top-level `application` section
through the preserved `src/application_config.rs` declaration. Its values use
the same `env://`, `${NAME}`, `file://`, and custom `SecretProvider` resolution
before deserialization. `ServiceConfigWithApplication<A>` dereferences to the
built-in service config, exposes the typed value as `config.application`, and
redacts the entire application value from `Debug` output. Production unknown
fields are rejected against both schemas and built-in service validation runs
before a listener is bound.

Newly generated `ApplicationConfig` types also derive `veil::Redact` with
fixed-length masking for every field, so
direct `Debug` logging remains fail-safe. Existing application-owned files are
preserved by `--update`; adopt the derive explicitly when migrating them.
The initial empty type contains a public, documentation-hidden, Serde-skipped
zero-sized marker so Veil can derive redaction before application fields exist;
keep the marker when extending the type.

When updating an older generated REST/RPC service, `rozectl --update` migrates
only exact historical generated config-loader shapes (`load`, `load_service`,
and the first typed loader). Custom `src/config/mod.rs` files remain unchanged
and produce a manual-migration warning. The generated config module declares
the application-config submodule itself; extra binary targets can therefore
reuse that config module without separately declaring `application_config`.

`ROZE_AUTH_JWT_KEYS` accepts one JSON array and merges entries by key `id`.
This supports adding a new key and replacing an existing key without relying
on array indexes. The YAML active key remains explicit and can use the normal
`ROZE__AUTH__JWT_ACTIVE_KEY_ID` scalar override.

JWT key IDs must be unique, every resolved HMAC secret must contain at least
32 bytes, and the active key must exist. Startup fails before the HTTP/RPC
listener is created when a reference is missing or validation fails. Errors
identify only the reference/key ID; `Debug` output redacts key material.

Generated production idempotency configuration uses:

```yaml
profile: production
cache:
  url: env://REDIS_URL
idempotency:
  store: auto
  key_prefix: captcha:idempotency:v1
  record_ttl_millis: 86400000
  unavailable_policy: fail_fast
```

`auto` selects Redis when cache configuration exists. `redis` requires
`cache.url`; `fail_fast` checks Redis during startup, while `fail_closed`
allows startup and returns the existing observable storage-unavailable error
when an idempotent request cannot reach Redis. A generated service containing
idempotent routes refuses a memory store in the production profile.
