# Security Policy

Roze is pre-release. Security-sensitive production use should pin a reviewed
Git revision or a future tagged release.

## Supported Versions

No stable version is published yet. Security fixes currently land on `main`.
After the first release, this file should list supported release lines.

## Reporting a Vulnerability

Until a private advisory process is configured, report vulnerabilities through a
private channel maintained by the repository owners. Do not open public issues
for exploitable vulnerabilities.

Reports should include:

- Affected crate, binary, generated code path, or template.
- Minimal reproduction.
- Impact.
- Suggested fix, if known.

## Security Areas

High-priority areas:

- JWT verification, key rotation, claims, and tenant isolation.
- Permission/RBAC/ABAC behavior.
- Gateway auth, fallback, and proxy passthrough boundaries.
- Config center integrity and rollback.
- MQ retry/dead-letter/idempotency semantics.
- Generated code that handles auth, headers, validation, or file paths.
