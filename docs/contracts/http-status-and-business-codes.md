# HTTP Status and Numeric Business Code Contract

Roze uses one numeric response envelope for generated REST application routes:

```json
{
  "code": 0,
  "msg": "OK",
  "data": {}
}
```

`code: 0` means success. Errors use numeric codes through `RozeError`; the
standard framework errors use their HTTP status as the envelope code. HTTP
status remains the transport outcome and must not be inferred only from the
JSON body.

Generated handlers use `roze_result::ApiResponse`. OpenAPI, generated
TypeScript/JavaScript clients, mocks, and contract tests must describe and
consume the same numeric shape. Roze does not expose a parallel string
business-code response contract, and `.api` does not define string-code
`@status` or `@error` annotations.

| HTTP status | Numeric code | Meaning |
| ---: | ---: | --- |
| 200 | 0 | Success |
| 400 | 400 | Invalid request |
| 401 | 401 | Missing or invalid authentication |
| 403 | 403 | Not authorized |
| 404 | 404 | Resource not found |
| 409 | 409 | State or uniqueness conflict |
| 412 | 412 | Failed precondition |
| 429 | 429 | Rate limited |
| 500 | 500 | Internal error |
| 503 | 503 | Service unavailable |

Use typed response data for domain details. Do not put user input, resource
identifiers, or other high-cardinality values into `code`.
