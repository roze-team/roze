# HTTP Status and Business Code Contract

Roze supports an opt-in string business-code envelope without changing the
existing numeric `ApiResponse` contract.

Use `CodedApiResponse::ok(data)` when a public API requires `"code": "OK"`:

```rust
use roze_http::http::StatusCode;
use roze_result::CodedApiResponse;

let queried = CodedApiResponse::ok(resource); // HTTP 200
let created = (StatusCode::CREATED, CodedApiResponse::ok(resource)); // HTTP 201
let accepted = (StatusCode::ACCEPTED, CodedApiResponse::ok(job)); // HTTP 202
let no_content = StatusCode::NO_CONTENT; // HTTP 204, empty body
```

Existing code that uses `ApiResponse::ok(data)` continues to serialize the
numeric success code `0`. This compatibility behavior is not changed.

## `.api` response contracts

Generated REST services opt into the string envelope with `@status`. Generated
RPC and REST methods declare their bounded business-error catalog with
repeatable `@error` annotations:

```go
service order-api {
  @handler createOrder
  @status 201
  @error ORD-CONFLICT-001 409
  @error COM-LIMIT-001 429
  post /orders (CreateOrderReq) returns (OrderResp)

  @handler deleteOrder
  @status 204
  delete /orders/:id (DeleteOrderReq) returns (EmptyResp)
}
```

`@status` accepts 200, 201, 202, or 204. A 204 route must return `EmptyResp`.
Routes without `@status` retain the legacy HTTP 200 numeric envelope. Business
codes must match `DOMAIN-CATEGORY-NNN`; `@error` accepts only the status values
in the table below. A code cannot map to different statuses in one service.

Generation projects this contract into handlers, native OpenAPI, TypeScript
and JavaScript SDK types, HTTP contract tests, and
`ops/error-contract.yaml`. Generated services validate the shared catalog at
startup. When a catalog is present, an undeclared or status-mismatched coded
error is converted to a generic internal error before it crosses REST or RPC.

Use `RozeError::coded(status, code, message)` for catalogued string business
errors. Use `RozeError::coded_rate_limited(code, message, retry_after)` for a
429 response that also includes `Retry-After`.

```rust
use roze_error::RozeError;
use roze_http::http::StatusCode;

return Err(RozeError::coded(
    StatusCode::CONFLICT,
    "ORD-CONFLICT-001",
    "order version conflicts with the current state",
));
```

The standard mapping is:

| HTTP status | Meaning | Example code |
| ---: | --- | --- |
| 200 | Query, update, or business command succeeded | `OK` |
| 201 | Resource created | `OK` |
| 202 | Accepted for asynchronous processing | `OK` |
| 204 | Succeeded without a response body | none |
| 400 | Invalid JSON, field format, or basic parameter | `COM-VAL-001` |
| 401 | Missing, invalid, or expired authentication | `AUTH-AUTHN-001` |
| 403 | Authenticated but not authorized | `AUTH-AUTHZ-001` |
| 404 | Resource not found | `ORD-NFD-001` |
| 409 | State, version, idempotency, or uniqueness conflict | `ORD-CONFLICT-001` |
| 422 | Syntactically valid request rejected by a business rule | `RISK-REJECT-001` |
| 429 | Rate limit exceeded | `COM-LIMIT-001` |
| 500 | Unknown internal failure | `COM-INTERNAL-001` |
| 502 | Invalid upstream response | `COM-DEP-002` |
| 503 | Service temporarily unavailable | `COM-DEP-001` |
| 504 | Upstream timeout | `COM-TIMEOUT-001` |

HTTP 412 remains available through `RozeError::FailedPrecondition` for
technical conditional-request failures such as a failed `If-Match`. HTTP 422
is intended for domain rule rejection after request syntax and basic fields
have been accepted.

## RPC propagation

RPC transports the string business code in `x-roze-error-code` and the exact
HTTP status in `x-roze-http-status`. The receiving adapter reconstructs the
original coded error. Retry delays use the existing `retry-after` metadata.
The gRPC mapping is deterministic: 422 maps to `FailedPrecondition`, 502 and
503 to `Unavailable`, and 504 to `DeadlineExceeded`.

Business codes must come from a bounded application-owned catalog. Do not put
user input, resource identifiers, or other high-cardinality values in `code`;
put those details in typed response data or safe messages instead.

## Generated SDK transport policy

Generated clients treat HTTP status as the transport outcome and the envelope
code as the business outcome. They invoke `onUnauthorized` for HTTP 401 and
honor `Retry-After` for HTTP 429. Automatic retries are limited to bounded
budgets and HTTP 429/502/503/504 or transport failures; mutation requests are
eligible only when they carry an `Idempotency-Key`. HTTP 204 resolves as
`void` and is never parsed as JSON.
