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
const crypto = require("crypto");
let flaky = 0;
let activeShed = 0;
let activeSse = 0;
let activeWebSocket = 0;
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
    activeShed += 1;
    return setTimeout(() => {
      activeShed -= 1;
      res.end(JSON.stringify({ slow: true }));
    }, 1000);
  }
  if (req.url === "/shed-active") {
    return res.end(String(activeShed));
  }
  if (req.url === "/events") {
    activeSse += 1;
    res.setHeader("content-type", "text/event-stream");
    res.setHeader("cache-control", "no-cache");
    res.write("data: first\n\n");
    return setTimeout(() => {
      activeSse -= 1;
      res.end("data: second\n\n");
    }, 1000);
  }
  if (req.url === "/events-active") {
    return res.end(String(activeSse));
  }
  if (req.url === "/websocket-active") {
    return res.end(String(activeWebSocket));
  }
  res.statusCode = 404;
  res.end(JSON.stringify({ error: "not found", url: req.url }));
});
server.on("upgrade", (req, socket) => {
  if (req.url !== "/socket" || !req.headers["sec-websocket-key"]) {
    socket.end("HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n");
    return;
  }
  const accept = crypto
    .createHash("sha1")
    .update(req.headers["sec-websocket-key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11")
    .digest("base64");
  socket.write(
    "HTTP/1.1 101 Switching Protocols\r\n" +
    "Upgrade: websocket\r\n" +
    "Connection: Upgrade\r\n" +
    "Sec-WebSocket-Accept: " + accept + "\r\n\r\n"
  );
  activeWebSocket += 1;
  socket.once("data", (frame) => {
    const length = frame[1] & 0x7f;
    const masked = (frame[1] & 0x80) !== 0;
    if (!masked || length > 125 || frame.length < 6 + length) {
      socket.destroy();
      return;
    }
    const mask = frame.subarray(2, 6);
    const payload = Buffer.alloc(length);
    for (let index = 0; index < length; index += 1) {
      payload[index] = frame[6 + index] ^ mask[index % 4];
    }
    if (payload.toString() === "ping") {
      socket.write(Buffer.concat([Buffer.from([0x81, 4]), Buffer.from("pong")]));
    }
  });
  socket.on("close", () => {
    activeWebSocket = Math.max(0, activeWebSocket - 1);
  });
});
server.listen(Number("$UPSTREAM_PORT"), "127.0.0.1", () => {
  console.log("upstream listening");
});
EOF_NODE

cat >"$BASE/ws-client.js" <<'EOF_WS_CLIENT'
const crypto = require("crypto");
const net = require("net");
const port = Number(process.argv[2]);
const expectedStatus = Number(process.argv[3]);
const holdMs = Number(process.argv[4] || 0);
const key = crypto.randomBytes(16).toString("base64");
const expectedAccept = crypto
  .createHash("sha1")
  .update(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11")
  .digest("base64");
const socket = net.connect(port, "127.0.0.1");
let pending = Buffer.alloc(0);
let upgraded = false;
socket.on("connect", () => {
  socket.write(
    "GET /socket HTTP/1.1\r\n" +
    "Host: 127.0.0.1:" + port + "\r\n" +
    "Connection: Upgrade\r\n" +
    "Upgrade: websocket\r\n" +
    "Sec-WebSocket-Version: 13\r\n" +
    "Sec-WebSocket-Key: " + key + "\r\n\r\n"
  );
});
socket.on("data", (chunk) => {
  pending = Buffer.concat([pending, chunk]);
  if (!upgraded) {
    const headerEnd = pending.indexOf("\r\n\r\n");
    if (headerEnd < 0) return;
    const headers = pending.subarray(0, headerEnd).toString();
    const status = Number(headers.split("\r\n")[0].split(" ")[1]);
    if (status !== expectedStatus) throw new Error("unexpected WebSocket status " + status);
    if (status !== 101) {
      socket.destroy();
      process.exit(0);
    }
    if (!headers.toLowerCase().includes("sec-websocket-accept: " + expectedAccept.toLowerCase())) {
      throw new Error("invalid WebSocket accept header");
    }
    upgraded = true;
    pending = pending.subarray(headerEnd + 4);
    const payload = Buffer.from("ping");
    const mask = crypto.randomBytes(4);
    const masked = Buffer.alloc(payload.length);
    for (let index = 0; index < payload.length; index += 1) {
      masked[index] = payload[index] ^ mask[index % 4];
    }
    socket.write(Buffer.concat([Buffer.from([0x81, 0x80 | payload.length]), mask, masked]));
  }
  if (upgraded && pending.length >= 6 && pending[0] === 0x81 && pending[1] === 4) {
    if (pending.subarray(2, 6).toString() !== "pong") throw new Error("invalid WebSocket reply");
    console.log("websocket pong");
    setTimeout(() => socket.end(), holdMs);
    pending = Buffer.alloc(0);
  }
});
socket.on("error", (error) => {
  console.error(error);
  process.exit(1);
});
socket.on("close", () => process.exit(0));
setTimeout(() => {
  console.error("WebSocket client timeout");
  process.exit(1);
}, 5000).unref();
EOF_WS_CLIENT

cat >"$BASE/config.yaml" <<EOF_CONFIG
name: gateway-smoke
auth:
  jwt_keys:
    - id: smoke
      secret: smoke-secret
  jwt_active_key_id: smoke
  jwt_issuer: gateway-smoke
  jwt_audience: gateway-smoke
  api_keys:
    header: x-api-key
    keys:
      - key: smoke-key
        subject: smoke-user
        roles:
          - tester
gateway:
  listen: "127.0.0.1:$GATEWAY_PORT"
  cors:
    allow_origins: ["https://console.example"]
    allow_methods: [GET]
    allow_headers: [authorization, x-tenant]
    max_age_seconds: 600
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
    - path: /events
      service: upstream
      methods: [GET]
      rewrite: /events
      timeout_ms: 30
      stream_idle_timeout_ms: 2000
      max_stream_connections: 1
    - path: /socket
      service: upstream
      methods: [GET]
      rewrite: /socket
      timeout_ms: 500
      stream_idle_timeout_ms: 2000
      max_stream_connections: 1
governance: {}
EOF_CONFIG

cleanup() {
  if [[ -n "${ws_pid:-}" ]]; then
    kill "$ws_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "${events_pid:-}" ]]; then
    kill "$events_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "${shed_pid:-}" ]]; then
    kill "$shed_pid" >/dev/null 2>&1 || true
  fi
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

ROZE_GATEWAY_CONFIG_FILE="$BASE/config.yaml" \
ROZE_CONFIG_CENTER_APP="gateway-smoke" \
ROZE_CONFIG_CENTER_FILE="$BASE/config.yaml" \
ROZE_CONFIG_CENTER_POLL_SECS=1 \
ROZE_CONFIG_CENTER_DEBOUNCE_MS=50 \
"$GATEWAY_BIN" >"$BASE/gateway.out" 2>"$BASE/gateway.err" &
gateway_pid=$!
for _ in $(seq 1 80); do
  if curl -sS "http://127.0.0.1:$GATEWAY_PORT/rewrite" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

require_status 200 "http://127.0.0.1:$GATEWAY_PORT/rewrite"
grep -q '"route":"rewritten"' "$BASE/response.json"

cors_status="$(curl -sS -o /dev/null -D "$BASE/cors.headers" -w "%{http_code}" \
  -X OPTIONS \
  -H "Origin: https://console.example" \
  -H "Access-Control-Request-Method: GET" \
  -H "Access-Control-Request-Headers: Authorization, X-Tenant" \
  "http://127.0.0.1:$GATEWAY_PORT/not-a-business-route")"
if [[ "$cors_status" != "204" ]]; then
  echo "expected allowed CORS preflight to return 204, got $cors_status" >&2
  exit 1
fi
grep -qi '^access-control-allow-origin: https://console.example' "$BASE/cors.headers"
grep -qi '^access-control-max-age: 600' "$BASE/cors.headers"
require_status 403 "http://127.0.0.1:$GATEWAY_PORT/rewrite" \
  -X OPTIONS \
  -H "Origin: https://untrusted.example" \
  -H "Access-Control-Request-Method: GET"

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
for _ in $(seq 1 200); do
  if [[ "$(curl -sS "http://127.0.0.1:$UPSTREAM_PORT/shed-active" 2>/dev/null || true)" == "1" ]]; then
    break
  fi
  sleep 0.02
done
require_status 429 "http://127.0.0.1:$GATEWAY_PORT/shed"
wait "$shed_pid"

curl -sS -o "$BASE/events-first.txt" "http://127.0.0.1:$GATEWAY_PORT/events" &
events_pid=$!
for _ in $(seq 1 200); do
  if [[ "$(curl -sS "http://127.0.0.1:$UPSTREAM_PORT/events-active" 2>/dev/null || true)" == "1" ]]; then
    break
  fi
  sleep 0.02
done
require_status 429 "http://127.0.0.1:$GATEWAY_PORT/events"
wait "$events_pid"
grep -q 'data: first' "$BASE/events-first.txt"
grep -q 'data: second' "$BASE/events-first.txt"

node "$BASE/ws-client.js" "$GATEWAY_PORT" 101 1000 >"$BASE/ws-first.out" 2>"$BASE/ws-first.err" &
ws_pid=$!
for _ in $(seq 1 200); do
  if [[ "$(curl -sS "http://127.0.0.1:$UPSTREAM_PORT/websocket-active" 2>/dev/null || true)" == "1" ]]; then
    break
  fi
  sleep 0.02
done
node "$BASE/ws-client.js" "$GATEWAY_PORT" 429 0
wait "$ws_pid"
grep -q 'websocket pong' "$BASE/ws-first.out"

cp "$BASE/config.yaml" "$BASE/config.valid.yaml"
sed -i '0,/rewrite: \/rewritten/s//rewrite: \/healthz/' "$BASE/config.yaml"
hot_reloaded=false
for _ in $(seq 1 80); do
  if curl -sS "http://127.0.0.1:$GATEWAY_PORT/rewrite" 2>/dev/null | grep -q '"ok":true'; then
    hot_reloaded=true
    break
  fi
  sleep 0.1
done
if [[ "$hot_reloaded" != "true" ]]; then
  echo "gateway config hot reload did not apply" >&2
  cat "$BASE/gateway.err" >&2 || true
  exit 1
fi
printf 'gateway: [invalid\n' >"$BASE/config.yaml"
reload_failed=false
for _ in $(seq 1 80); do
  if grep -q 'gateway.config.reload.failed' "$BASE/gateway.out"; then
    reload_failed=true
    break
  fi
  sleep 0.1
done
if [[ "$reload_failed" != "true" ]]; then
  echo "gateway invalid config reload was not observed" >&2
  cat "$BASE/gateway.out" >&2 || true
  exit 1
fi
curl -sS "http://127.0.0.1:$GATEWAY_PORT/rewrite" | grep -q '"ok":true'
sleep 3
reload_failure_count="$(grep -c 'gateway.config.reload.failed' "$BASE/gateway.out" || true)"
if [[ "$reload_failure_count" != "1" ]]; then
  echo "expected one invalid reload notification, got $reload_failure_count" >&2
  cat "$BASE/gateway.out" >&2 || true
  exit 1
fi

echo "gateway smoke passed"
