# roze-pink framework gap closure (2026-07-30)

This change set addresses the six issues reported by the `roze-pink`
regeneration exercise without adding project-local replacements.

| Issue | Framework outcome | Evidence |
| --- | --- | --- |
| RZ-011 | Generated TS/JS clients unwrap `{code,msg,data}` and use `msg` for business errors. | Client generator unit tests. |
| RZ-012 | Generated REST authorization propagates JWT permissions and scopes into the request context. | REST generator permission test. |
| RZ-013 | SeaORM grouped integer sums cast PostgreSQL `SUM(bigint)` to `BIGINT` before `Option<i64>` decoding; the same expression is used by HAVING. | Model render test and generated-crate compile smoke. |
| RZ-014 | `QiniuKodo` executes the existing S3 SigV4 runtime against Qiniu's official S3-compatible endpoint. | Kodo SigV4 unit test; real bucket evidence remains credential-gated. |
| RZ-015 | `roze-redis` owns standalone/Cluster topology; cache, idempotency, and rate limit reuse it. | Package tests plus ignored real Cluster round trip. |
| RZ-016 | `model generate --update` inherits an ORM marker or legacy manifest dependency, refuses ambiguous updates, and requires `--switch-orm` for a change. | ORM inheritance/switch tests and CLI parsing tests. |

## Compatibility

- Existing model create behavior remains Toasty by default.
- Existing model update commands with explicit matching `--orm` remain valid.
- Existing single-URL Redis configuration remains valid.
- Application-owned `src/model/*_ext.rs` files remain preserved on update and
  explicit ORM switch.
- Provider credentials remain redacted from `Debug`.

## Remaining credential-gated evidence

The repository does not contain cloud or Redis credentials. Run the ignored
Kodo and Redis Cluster tests in dedicated test environments before declaring
environment-specific production readiness. Qiniu-native upload callbacks and
multipart/CDN operations remain outside the S3-compatible provider contract.
