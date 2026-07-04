#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${ROZE_GATEWAY_SMOKE_DIR:-/tmp/roze-gateway-smoke}"
GATEWAY_PORT="${ROZE_GATEWAY_SMOKE_PORT:-19081}"
UPSTREAM_PORT="${ROZE_GATEWAY_SMOKE_UPSTREAM_PORT:-19082}"
GATEWAY_BIN="$ROOT/target/debug/roze-gateway-app"

mkdir -p "$BASE"
rm -f "$BASE/gateway.out" "$BASE/gateway.err" "$BASE/upstream.out" "$BASE/upstream.err"

cat >"$BASE/upstream.js" <<EOF_NODE
const http = require("http");
let flaky = 0;
const server = http.createServer((req, res) => {
  res.setHeader("content-type", "application/json");
  if (req.url === "/healthz") {
    return res.end(JSON.stringify({ ok: true }));
  }
  if (req.url === "/rewritten") {
    return res.end(JSON.stringify({ route: "rewritten", method: req.method }));
  }
  if (req.url === "/secure") {
    return res.end(JSON.stringify({ subject: req.headers["x-roze-subject"] || "" }));
  }
  if (req.url === "/flaky") {
    flaky += 1;
    if (flaky === 1) {
      res.statusCode = 503;
      return res.end(JSON.stringify({ retry: "first" }));
    }
    return res.end(JSON.stringify({ retry: "ok", attempts: flaky }));
  }
  if (req.url === "/slow") {
    console.log("slow hit");
    return setTimeout(() => res.end(JSON.stringify({ slow: true })), 1000);
  }
  if (req.url === "/shed-slow") {
    console.log("shed hit");
    return setTimeout(() => res.end(JSON.stringify({ slow: true })), 1000);
  }
  res.statusCode = 404;
  res.end(JSON.stringify({ error: "not found", url: req.url }));
});
server.listen(Number("$UPSTREAM_PORT"), "127.0.0.1", () => {
  console.log("upstream listening");
});
EOF_NODE

cat >"$BASE/config.yaml" <<EOF_CONFIG
name: gateway-smoke
auth:
  jwt_secret: smoke-secret
  api_keys:
    header: x-api-key
    keys:
      - key: smoke-key
        subject: smoke-user
        roles:
          - tester
gateway:
  listen: "127.0.0.1:$GATEWAY_PORT"
  services:
    - name: upstream
      upstream: "http://127.0.0.1:$UPSTREAM_PORT"
  routes:
    - path: /rewrite
      service: upstream
      methods: [GET]
      rewrite: /rewritten
    - path: /secure
      service: upstream
      methods: [GET]
      rewrite: /secure
      middlewares: [auth]
    - path: /flaky
      service: upstream
      methods: [GET]
      rewrite: /flaky
      retries: 1
      retry_backoff_ms: 1
    - path: /slow
      service: upstream
      methods: [GET]
      rewrite: /slow
      timeout_ms: 30
      fallback:
        status: 599
        body:
          code: 599
          message: timeout fallback
    - path: /limited
      service: upstream
      methods: [GET]
      rewrite: /rewritten
      middlewares: [rate]
      rate_limit:
        burst: 1
        refill_ms: 10000
    - path: /shed
      service: upstream
      methods: [GET]
      rewrite: /shed-slow
      shedding:
        concurrency: 1
governance: {}
EOF_CONFIG

cleanup() {
  if [[ -n "${gateway_pid:-}" ]]; then
    kill "$gateway_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "${upstream_pid:-}" ]]; then
    kill "$upstream_pid" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

require_status() {
  local expected="$1"
  local url="$2"
  shift 2
  local status
  status="$(curl -sS -o "$BASE/response.json" -w "%{http_code}" "$@" "$url")"
  if [[ "$status" != "$expected" ]]; then
    echo "expected HTTP $expected for $url, got $status" >&2
    cat "$BASE/response.json" >&2 || true
    echo "--- gateway stderr ---" >&2
    cat "$BASE/gateway.err" >&2 || true
    exit 1
  fi
}

cargo build -p roze-gateway-app

node "$BASE/upstream.js" >"$BASE/upstream.out" 2>"$BASE/upstream.err" &
upstream_pid=$!
for _ in $(seq 1 50); do
  if grep -q "upstream listening" "$BASE/upstream.out"; then
    break
  fi
  sleep 0.1
done

ROZE_GATEWAY_CONFIG_FILE="$BASE/config.yaml" "$GATEWAY_BIN" >"$BASE/gateway.out" 2>"$BASE/gateway.err" &
gateway_pid=$!
for _ in $(seq 1 80); do
  if curl -sS "http://127.0.0.1:$GATEWAY_PORT/rewrite" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

require_status 200 "http://127.0.0.1:$GATEWAY_PORT/rewrite"
grep -q '"route":"rewritten"' "$BASE/response.json"

require_status 401 "http://127.0.0.1:$GATEWAY_PORT/secure"
require_status 200 "http://127.0.0.1:$GATEWAY_PORT/secure" -H "x-api-key: smoke-key"
grep -q '"subject":"smoke-user"' "$BASE/response.json"

require_status 200 "http://127.0.0.1:$GATEWAY_PORT/flaky"
grep -q '"retry":"ok"' "$BASE/response.json"

require_status 599 "http://127.0.0.1:$GATEWAY_PORT/slow"
grep -q 'timeout fallback' "$BASE/response.json"

require_status 200 "http://127.0.0.1:$GATEWAY_PORT/limited"
require_status 429 "http://127.0.0.1:$GATEWAY_PORT/limited"

curl -sS -o "$BASE/shed-first.json" "http://127.0.0.1:$GATEWAY_PORT/shed" &
shed_pid=$!
for _ in $(seq 1 50); do
  if grep -q "shed hit" "$BASE/upstream.out"; then
    break
  fi
  sleep 0.02
done
require_status 429 "http://127.0.0.1:$GATEWAY_PORT/shed"
wait "$shed_pid"

echo "gateway smoke passed"
