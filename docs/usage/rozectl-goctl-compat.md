# rozectl goctl compatibility

`rozectl` keeps Rust-native generated output while accepting the common goctl
command shapes used for HTTP API, RPC, model, deployment, docs, clients, and
plugins.

The compatibility target is generator coverage, not automatic business
implementation. `rozectl` generates scaffolding and glue code such as routing,
protocol bindings, basic CRUD modules, SDKs, docs, Dockerfiles, and Kubernetes
templates. Product-specific logic still needs to be implemented in generated
logic modules and supporting application code.

## Command mapping

| goctl command | rozectl command | Status |
| --- | --- | --- |
| `goctl api new user` | `rozectl api new user` | Supported |
| `goctl api go -api user.api -dir .` | `rozectl api go -api user.api -dir .` | Supported |
| `goctl rpc new user` | `rozectl rpc new user` | Supported |
| `goctl rpc protoc user.proto --go_out=./pb --go-grpc_out=./pb --zrpc_out=.` | `rozectl rpc protoc user.proto --go_out=./pb --go-grpc_out=./pb --zrpc_out=.` | Supported with Rust tonic output under `--zrpc_out` |
| `goctl model mysql ddl -src user.sql -dir .` | `rozectl model mysql ddl -src user.sql -dir .` | Supported |
| `goctl model mysql datasource -url ... -table users -dir .` | `rozectl model mysql datasource -url ... -table users -dir .` | Supported |
| `goctl model pg datasource -url ... -table users -dir .` | `rozectl model pg datasource -url ... -table users -dir .` | Supported |
| `goctl model mongo --type User --dir .` | `rozectl model mongo --type User -dir .` | Supported |
| `goctl docker -go main.go` | `rozectl docker -go main.go` | Supported with Rust multi-stage Dockerfile |
| `goctl kube deploy ...` | `rozectl kube deploy ...` | Supported |
| `goctl api swagger --api user.api --dir .` | `rozectl api swagger -api user.api -dir . --format json` | Supported |
| `goctl api doc --dir . --o ./doc` | `rozectl api doc -dir . -o ./doc` | Supported |
| `goctl api ts --api user.api --dir .` | `rozectl api ts -api user.api -dir .` | Supported |
| `goctl api dart --api user.api --dir .` | `rozectl api dart -api user.api -dir .` | Supported |
| `goctl api plugin -p xxx --api user.api --dir .` | `rozectl api plugin -p xxx -api user.api -dir .` | Supported |

## Roze extensions

These commands remain available in addition to the compatibility aliases:

```bash
rozectl api generate example/user.api --out services/user-api
rozectl rpc generate example/user.api --out services/user-rpc
rozectl model generate example/user.sql --out services/user-api --format sql
rozectl model inspect users --db-kind postgres --db-url postgres://... --out services/user-api
rozectl openapi generate example/user.api --out openapi.json
rozectl api client js example/user.api --out sdk/user.js
```

## Output differences from goctl

`rozectl` accepts goctl command names for operator familiarity, but generated
projects remain Rust-native:

- REST services use Poem and Roze HTTP helpers.
- RPC services use tonic/prost and Roze RPC helpers.
- SQL models use SeaORM-style Rust modules.
- Mongo models generate Rust repository helpers.
- Dockerfiles build Rust binaries.
- Kubernetes output targets generated Roze service containers.

`--go_out` and `--go-grpc_out` are accepted by `rozectl rpc protoc` for command
compatibility. Rust RPC project files are generated under `--zrpc_out`.
