#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${ROZECTL_BIN:-$ROOT/target/debug/rozectl}"
BASE="${ROZE_ROZECTL_SMOKE_DIR:-/private/tmp/roze-rozectl-smoke}"
OUT="$BASE/out"
SEARCH_PORT="${ROZE_SMOKE_SEARCH_PORT:-18770}"

mkdir -p "$BASE"
rm -rf "$OUT"
mkdir -p "$OUT/fake-bin"

if [[ -z "${ROZECTL_BIN:-}" ]]; then
  cargo build -p rozectl
elif [[ ! -x "$BIN" ]]; then
  cargo build -p rozectl
fi

cat >"$BASE/user.api" <<'EOF_API'
@server (
    prefix: /api
)
service user-api {
    @handler getUser
    get /users/:id (GetUserReq) returns (UserResp)
    @handler createUser
    post /users (CreateUserReq) returns (UserResp)
}

type (
    GetUserReq {
        id u64 `path:"id"`
    }

    CreateUserReq {
        name string `json:"name"`
        email string `json:"email"`
    }

    UserResp {
        id u64 `json:"id"`
        name string `json:"name"`
        email string `json:"email"`
    }
)
EOF_API

cat >"$BASE/user-breaking.api" <<'EOF_API'
@server (
    prefix: /api
)
service user-api {
    @handler createUser
    post /users (CreateUserReq) returns (UserResp)
}

type (
    CreateUserReq {
        name string `json:"name"`
    }

    UserResp {
        id u64 `json:"id"`
    }
)
EOF_API

cat >"$BASE/user-rpc.api" <<'EOF_API'
service user {
    rpc GetUser (GetUserReq) returns (GetUserResp)
}

type (
    GetUserReq {
        id: u64
    }

    GetUserResp {
        id: u64
        name: string
    }
)
EOF_API

cat >"$BASE/user.proto" <<'EOF_PROTO'
syntax = "proto3";

package user;

service UserService {
  rpc GetUser (GetUserReq) returns (GetUserResp);
}

message GetUserReq {
  uint64 id = 1;
}

message GetUserResp {
  uint64 id = 1;
  string name = 2;
}
EOF_PROTO

cat >"$BASE/user.model" <<'EOF_MODEL'
model User {
    table: users
    primary: id
    cache: true
    field id u64
    field name string
    field email string
    unique_index: email
}
EOF_MODEL

cat >"$BASE/user.sql" <<'EOF_SQL'
CREATE TABLE users (
    id BIGINT PRIMARY KEY,
    name TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    created_at TIMESTAMP
);
EOF_SQL

cat >"$BASE/user.search" <<'EOF_SEARCH'
index users
primary id
field id u64 primary filterable sortable
field name text searchable
field email keyword filterable
field created_at datetime sortable
EOF_SEARCH

cat >"$BASE/plugin.sh" <<'EOF_PLUGIN'
#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null
mkdir -p "$ROZECTL_OUT_DIR/plugin-output"
printf 'plugin ok\n' > "$ROZECTL_OUT_DIR/plugin-output/result.txt"
EOF_PLUGIN
chmod +x "$BASE/plugin.sh"

cat >"$OUT/fake-bin/docker" <<'EOF_DOCKER'
#!/usr/bin/env bash
printf '%s\n' "$*" >> /private/tmp/roze-rozectl-smoke/out/docker-args.log
exit 0
EOF_DOCKER
chmod +x "$OUT/fake-bin/docker"

cat >"$BASE/search-server.js" <<EOF_NODE
const http = require("http");
const port = Number(process.env.ROZE_SMOKE_SEARCH_PORT || "$SEARCH_PORT");
const mapping = { users: { mappings: { properties: {
  id: { type: "long" },
  name: { type: "text" },
  email: { type: "keyword" },
  created_at: { type: "date" }
}}}};
const meiliSettings = {
  searchableAttributes: ["name"],
  filterableAttributes: ["email"],
  sortableAttributes: ["created_at"]
};
const meiliDocuments = {
  results: [{ id: 1, name: "Alice", email: "alice@example.com", created_at: "2026-06-24T00:00:00Z" }]
};
const meiliIndex = { uid: "users", primaryKey: "id" };
const server = http.createServer((req, res) => {
  res.setHeader("content-type", "application/json");
  if (req.url === "/users/_mapping") return res.end(JSON.stringify(mapping));
  if (req.url === "/indexes/users/settings") return res.end(JSON.stringify(meiliSettings));
  if (req.url.startsWith("/indexes/users/documents")) return res.end(JSON.stringify(meiliDocuments));
  if (req.url === "/indexes/users") return res.end(JSON.stringify(meiliIndex));
  if (req.url === "/health") return res.end(JSON.stringify({ status: "available" }));
  res.statusCode = 404;
  res.end(JSON.stringify({ error: "not found", url: req.url }));
});
server.listen(port, "127.0.0.1", () => console.log("search fake server listening"));
EOF_NODE

API="$BASE/user.api"
API_BREAKING="$BASE/user-breaking.api"
RPC_API="$BASE/user-rpc.api"
PROTO="$BASE/user.proto"
MODEL="$BASE/user.model"
SQL="$BASE/user.sql"
SEARCH="$BASE/user.search"
PLUGIN="$BASE/plugin.sh"

pass() {
  printf 'PASS %s\n' "$1"
}

skip() {
  printf 'SKIP %s\n' "$1"
}

"$BIN" --help >/dev/null
for help_args in \
  "api --help" \
  "api generate --help" \
  "api new --help" \
  "api client --help" \
  "api client ts --help" \
  "api client js --help" \
  "api client dart --help" \
  "api doc --help" \
  "api plugin --help" \
  "rpc --help" \
  "rpc generate --help" \
  "rpc new --help" \
  "rpc protoc --help" \
  "model --help" \
  "model generate --help" \
  "model inspect --help" \
  "search --help" \
  "search generate --help" \
  "search inspect --help" \
  "template --help" \
  "template list --help" \
  "template show --help" \
  "template init --help" \
  "diff --help" \
  "diff api --help" \
  "diff rpc --help" \
  "diff model --help" \
  "contract --help" \
  "contract check --help" \
  "mock --help" \
  "mock gen --help" \
  "test --help" \
  "test gen --help" \
  "dev --help" \
  "dev up --help" \
  "dev down --help" \
  "dev status --help" \
  "doctor --help" \
  "doc --help" \
  "doc service --help" \
  "doc ai-context --help" \
  "openapi --help" \
  "openapi generate --help" \
  "docker --help" \
  "kube --help" \
  "kube deploy --help" \
  "kube validate --help" \
  "helm --help" \
  "helm chart --help" \
  "helm validate --help"; do
  # shellcheck disable=SC2086
  "$BIN" $help_args >/dev/null
done
pass "help"

"$BIN" api generate "$API" --out "$OUT/api-generate" --force >/dev/null
test -f "$OUT/api-generate/src/main.rs"
pass "api generate"

"$BIN" api new smoke-api --out "$OUT/api-new" --force >/dev/null
test -f "$OUT/api-new/smoke-api.api"
pass "api new"

"$BIN" api client ts "$API" --out "$OUT/client.ts" >/dev/null
"$BIN" api client js "$API" --out "$OUT/client.js" >/dev/null
"$BIN" api client dart "$API" --out "$OUT/client.dart" >/dev/null
test -f "$OUT/client.ts"
test -f "$OUT/client.js"
test -f "$OUT/client.dart"
pass "api client ts/js/dart"

"$BIN" api doc --api "$API" --dir "$OUT/api-generate" --out "$OUT/api-doc" >/dev/null
test -d "$OUT/api-doc"
pass "api doc"

"$BIN" api plugin --plugin "$PLUGIN" --api "$API" --dir "$OUT/plugin-dir" >/dev/null
test -f "$OUT/plugin-dir/plugin-output/result.txt"
pass "api plugin"

"$BIN" rpc generate "$RPC_API" --out "$OUT/rpc-generate" --force >/dev/null
test -f "$OUT/rpc-generate/proto/service.proto"
pass "rpc generate"

"$BIN" rpc new smoke-rpc --out "$OUT/rpc-new" --force >/dev/null
test -f "$OUT/rpc-new/smoke-rpc.api"
pass "rpc new"

"$BIN" rpc protoc "$PROTO" --out "$OUT/rpc-protoc" --force >/dev/null
test -f "$OUT/rpc-protoc/proto/service.proto"
pass "rpc protoc"

"$BIN" model generate "$MODEL" --out "$OUT/model-generate" --force >/dev/null
test -f "$OUT/model-generate/src/model/user.rs"
pass "model generate dsl"

"$BIN" model generate "$SQL" --format sql --out "$OUT/model-sql" --force >/dev/null
test -f "$OUT/model-sql/src/model/user.rs"
pass "model generate sql"

sqlite_bin="${SQLITE_BIN:-$(command -v sqlite3 || true)}"
if [[ -n "$sqlite_bin" ]]; then
  sqlite_db="$OUT/users.sqlite"
  "$sqlite_bin" "$sqlite_db" 'CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT UNIQUE);'
  "$BIN" model inspect users --db-kind sqlite --db-url "sqlite://$sqlite_db" --out "$OUT/model-inspect-sqlite" --force >/dev/null
  test -f "$OUT/model-inspect-sqlite/src/model/user.rs"
  pass "model inspect sqlite"
else
  skip "model inspect sqlite: sqlite3 not found"
fi

if [[ -n "${ROZE_SMOKE_POSTGRES_URL:-}" ]]; then
  "$BIN" model inspect users --db-kind postgres --db-url "$ROZE_SMOKE_POSTGRES_URL" --out "$OUT/model-inspect-postgres" --force >/dev/null
  pass "model inspect postgres"
else
  skip "model inspect postgres: ROZE_SMOKE_POSTGRES_URL not set"
fi

if [[ -n "${ROZE_SMOKE_MYSQL_URL:-}" ]]; then
  "$BIN" model inspect users --db-kind mysql --db-url "$ROZE_SMOKE_MYSQL_URL" --out "$OUT/model-inspect-mysql" --force >/dev/null
  pass "model inspect mysql"
else
  skip "model inspect mysql: ROZE_SMOKE_MYSQL_URL not set"
fi

if [[ -n "${ROZE_SMOKE_MONGO_URL:-}" ]]; then
  "$BIN" model inspect users --db-kind mongo --db-url "$ROZE_SMOKE_MONGO_URL" --out "$OUT/model-inspect-mongo" --force >/dev/null
  pass "model inspect mongo"
else
  skip "model inspect mongo: ROZE_SMOKE_MONGO_URL not set"
fi

"$BIN" search generate "$SEARCH" --engine elasticsearch --out "$OUT/search-generate" --force >/dev/null
"$BIN" search generate "$SEARCH" --engine opensearch --out "$OUT/search-opensearch" --force >/dev/null
"$BIN" search generate "$SEARCH" --engine meilisearch --out "$OUT/search-meilisearch" --force >/dev/null
test -f "$OUT/search-generate/src/search/users.rs"
test -f "$OUT/search-opensearch/src/search/users.rs"
test -f "$OUT/search-meilisearch/src/search/users.rs"
pass "search generate elasticsearch/opensearch/meilisearch"

if command -v node >/dev/null 2>&1; then
  ROZE_SMOKE_SEARCH_PORT="$SEARCH_PORT" node "$BASE/search-server.js" >"$OUT/search-server.log" 2>&1 &
  search_pid=$!
  trap 'kill "$search_pid" >/dev/null 2>&1 || true' EXIT
  for _ in $(seq 1 50); do
    if grep -q "search fake server listening" "$OUT/search-server.log"; then
      break
    fi
    sleep 0.1
  done
  "$BIN" search inspect users --engine elasticsearch --url "http://127.0.0.1:$SEARCH_PORT" --out "$OUT/search-inspect-es" --force >/dev/null
  "$BIN" search inspect users --engine opensearch --url "http://127.0.0.1:$SEARCH_PORT" --out "$OUT/search-inspect-os" --force >/dev/null
  "$BIN" search inspect users --engine meilisearch --url "http://127.0.0.1:$SEARCH_PORT" --out "$OUT/search-inspect-meili" --force >/dev/null
  test -f "$OUT/search-inspect-es/src/search/users.rs"
  test -f "$OUT/search-inspect-os/src/search/users.rs"
  test -f "$OUT/search-inspect-meili/src/search/users.rs"
  pass "search inspect elasticsearch/opensearch/meilisearch"
else
  skip "search inspect: node not found"
fi

"$BIN" template list >/dev/null
"$BIN" template show api >/dev/null
"$BIN" template init --out "$OUT/templates" >/dev/null
test -d "$OUT/templates"
pass "template list/show/init"

"$BIN" diff api "$API" --out "$OUT/api-generate" >/dev/null
"$BIN" diff rpc "$RPC_API" --out "$OUT/rpc-generate" >/dev/null
"$BIN" diff model "$MODEL" --out "$OUT/model-generate" >/dev/null
pass "diff api/rpc/model"

"$BIN" contract check --old "$API" --new "$API" >/dev/null
if "$BIN" contract check --old "$API" --new "$API_BREAKING" >/dev/null 2>&1; then
  echo "expected breaking contract check to fail" >&2
  exit 1
fi
pass "contract check compatible/breaking"

"$BIN" mock gen --api "$API" --out "$OUT/mock" --force >/dev/null
test -f "$OUT/mock/src/main.rs"
pass "mock gen"

"$BIN" test gen --api "$API" --out "$OUT/contract-tests" --force >/dev/null
test -f "$OUT/contract-tests/Cargo.toml"
pass "test gen"

"$BIN" doc service --api "$API" --out "$OUT/SERVICE.md" --force >/dev/null
"$BIN" doc ai-context --api "$API" --out "$OUT/AI_CONTEXT.md" --force >/dev/null
test -f "$OUT/SERVICE.md"
test -f "$OUT/AI_CONTEXT.md"
pass "doc service/ai-context"

"$BIN" openapi generate "$API" --out "$OUT/openapi.json" >/dev/null
test -f "$OUT/openapi.json"
pass "openapi generate"

"$BIN" docker --binary smoke-api --out "$OUT/Dockerfile" >/dev/null
test -f "$OUT/Dockerfile"
pass "docker"

"$BIN" kube deploy --name smoke-api --image smoke-api:latest --out "$OUT/kubernetes.yaml" >/dev/null
"$BIN" kube validate --file "$OUT/kubernetes.yaml" >/dev/null
test -f "$OUT/kubernetes.yaml"
pass "kube deploy/validate"

"$BIN" helm chart --name smoke-api --image smoke-api:latest --out "$OUT/chart" >/dev/null
"$BIN" helm validate --chart "$OUT/chart" >/dev/null
test -f "$OUT/chart/Chart.yaml"
pass "helm chart/validate"

"$BIN" doctor --config "$OUT/api-generate/config.yaml" --tool cargo >/dev/null
pass "doctor"

PATH="$OUT/fake-bin:$PATH" "$BIN" dev status -f "$ROOT/docker-compose.integration.yml" >/dev/null
grep -q 'compose -f .*/docker-compose.integration.yml ps' "$OUT/docker-args.log"
PATH="$OUT/fake-bin:$PATH" "$BIN" dev up -f "$ROOT/docker-compose.integration.yml" --detach >/dev/null
grep -q 'compose -f .*/docker-compose.integration.yml up -d' "$OUT/docker-args.log"
PATH="$OUT/fake-bin:$PATH" "$BIN" dev down -f "$ROOT/docker-compose.integration.yml" -v >/dev/null
grep -q 'compose -f .*/docker-compose.integration.yml down -v' "$OUT/docker-args.log"
pass "dev status/up/down"

printf 'ALL PASSED\n'
