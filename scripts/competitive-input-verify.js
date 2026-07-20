"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const relativeFiles = [
  "benchmarks/competitive/baseline.yaml",
  "benchmarks/competitive/workloads.json",
  "benchmarks/competitive/contracts/scenario-contract.json",
  "benchmarks/competitive/contracts/competitive.api",
  "benchmarks/competitive/contracts/competitive.proto",
  "benchmarks/competitive/contracts/items.sql",
  "benchmarks/competitive/contracts/events.sql",
  "benchmarks/competitive/contracts/event.schema.json",
  "benchmarks/competitive/overlays/overlay-manifest.json",
  "benchmarks/competitive/overlays/roze-rest/src/logic/echo/echo.rs",
  "benchmarks/competitive/overlays/roze-rpc/src/logic/echo.rs",
  "benchmarks/competitive/overlays/roze-rest/src/logic/rpc_echo/rpc_echo.rs",
  "benchmarks/competitive/overlays/go-zero-rest/internal/logic/echologic.go",
  "benchmarks/competitive/overlays/go-zero-rpc/internal/logic/echologic.go",
  "benchmarks/competitive/overlays/go-zero-rpc/etc/competitive.v1.yaml",
  "benchmarks/competitive/overlays/go-zero-rest/internal/config/config.go",
  "benchmarks/competitive/overlays/go-zero-rest/internal/svc/servicecontext.go",
  "benchmarks/competitive/overlays/go-zero-rest/internal/logic/rpcechologic.go",
  "benchmarks/competitive/overlays/go-zero-rest/etc/competitive-api.yaml",
  "benchmarks/competitive/probes/grpc-echo/main.go",
];

function reject(message) {
  throw new Error(message);
}

function sameJson(left, right) {
  return JSON.stringify(left) === JSON.stringify(right);
}

function assertEqual(actual, expected, label) {
  if (!sameJson(actual, expected)) {
    reject(`${label} does not match the competitive workload`);
  }
}

function expectedGeneration() {
  return {
    roze: {
      rest: [
        "cargo",
        "run",
        "-p",
        "rozectl",
        "--",
        "api",
        "generate",
        "{restApi}",
        "--out",
        "{restOut}",
        "--roze-source",
        "path",
      ],
      rpc: [
        "cargo",
        "run",
        "-p",
        "rozectl",
        "--",
        "rpc",
        "protoc",
        "{rpcProto}",
        "--out",
        "{rpcOut}",
        "--roze-source",
        "path",
      ],
      linkRestRpc: [
        "cargo",
        "run",
        "-p",
        "rozectl",
        "--",
        "service",
        "dependency",
        "add",
        "competitive",
        "--project",
        "{restOut}",
        "--crate",
        "competitive-roze-rpc",
        "--path",
        "../competitive-roze-rpc",
        "--endpoint",
        "127.0.0.1:19090",
        "--timeout-ms",
        "2000",
      ],
      verifyRestRpcLink: [
        "cargo",
        "run",
        "-p",
        "rozectl",
        "--",
        "service",
        "sync",
        "--project",
        "{restOut}",
        "--check",
      ],
    },
    "go-zero": {
      rest: [
        "goctl",
        "api",
        "go",
        "--api",
        "{restApi}",
        "--dir",
        "{restOut}",
      ],
      rpc: [
        "goctl",
        "rpc",
        "protoc",
        "{rpcProto}",
        "--go_out",
        "{rpcOut}/competitive",
        "--go-grpc_out",
        "{rpcOut}/competitive",
        "--zrpc_out",
        "{rpcOut}",
      ],
    },
  };
}

function verifyDocuments(baseline, workloads, contract, inputs) {
  if (contract.schemaVersion !== 1) reject("scenario contract schemaVersion must be 1");
  if (contract.contractId !== "roze-go-zero-production-competition-v1") {
    reject("unexpected competitive contractId");
  }
  assertEqual(
    contract.inputs,
    {
      restApi: "competitive.api",
      rpcProto: "competitive.proto",
      databaseSeed: "items.sql",
      eventPersistence: "events.sql",
      eventSchema: "event.schema.json",
      applicationOverlays: "../overlays/overlay-manifest.json",
      grpcEchoProbe: "../probes/grpc-echo/main.go",
    },
    "contract inputs",
  );
  assertEqual(contract.generation, expectedGeneration(), "generation commands");
  assertEqual(
    Object.keys(contract.scenarios).sort(),
    workloads.scenarios.map((scenario) => scenario.id).sort(),
    "scenario IDs",
  );
  if (contract.runtime.requestTimeoutMs !== workloads.global.requestTimeoutMs) {
    reject("runtime request timeout drift");
  }
  if (
    workloads.global.sizeBasis !== "application-payload" ||
    contract.runtime.sizeBasis !== workloads.global.sizeBasis
  ) {
    reject("size basis must be application-payload for every protocol");
  }
  if (contract.runtime.connectTimeoutMs !== workloads.global.connectTimeoutMs) {
    reject("runtime connect timeout drift");
  }
  assertEqual(
    contract.runtime.telemetry,
    baseline.measurement.telemetry,
    "runtime telemetry",
  );
  if (contract.correctness.payloadSeed !== workloads.global.payloadSeed) {
    reject("payload seed drift");
  }
  if (contract.correctness.errorBudgetRatio !== workloads.global.errorBudgetRatio) {
    reject("error budget drift");
  }

  for (const workload of workloads.scenarios) {
    const scenario = contract.scenarios[workload.id];
    for (const field of [
      "requestBytes",
      "responseBytes",
      "datasetRows",
      "zipfTheta",
      "targetCacheHitRatio",
      "messageBytes",
      "messages",
      "instances",
      "slowInstanceDelayMs",
      "faultAfterSeconds",
      "faultDurationSeconds",
    ]) {
      if (workload[field] !== undefined && scenario[field] !== workload[field]) {
        reject(`${workload.id}.${field} drift`);
      }
    }
    if (typeof scenario.semantics !== "string" || scenario.semantics.length < 20) {
      reject(`${workload.id}.semantics must be explicit`);
    }
  }

  const api = inputs["competitive.api"];
  for (const fragment of [
    "post /echo (EchoRequest) returns (EchoResponse)",
    "post /rpc-echo (EchoRequest) returns (EchoResponse)",
    "get /items/:id (ItemRequest) returns (ItemResponse)",
    'payload string `json:"payload" validate:"required,len=1024"`',
    'response: "raw"',
  ]) {
    if (!api.includes(fragment)) reject(`competitive.api missing ${fragment}`);
  }
  const proto = inputs["competitive.proto"];
  for (const fragment of [
    "package competitive.v1;",
    "service Competitive",
    "rpc Echo(EchoRequest) returns (EchoResponse);",
    "bytes payload = 1;",
  ]) {
    if (!proto.includes(fragment)) reject(`competitive.proto missing ${fragment}`);
  }
  const sql = inputs["items.sql"];
  for (const pattern of [
    /CREATE TABLE items\s*\(/,
    /generate_series\(1,\s*100000\)/,
    /repeat\(chr\(97 \+ \(id % 26\)::integer\),\s*1024\)/,
    /TIMESTAMPTZ '2026-07-18 00:00:00\+00'/,
  ]) {
    if (!pattern.test(sql)) reject(`items.sql does not satisfy ${pattern}`);
  }
  const eventsSql = inputs["events.sql"];
  for (const pattern of [
    /CREATE TABLE competitive_inbox\s*\(\s*event_id TEXT PRIMARY KEY,/,
    /idempotency_key TEXT NOT NULL UNIQUE/,
    /CREATE TABLE competitive_outbox\s*\(\s*event_id TEXT PRIMARY KEY REFERENCES competitive_inbox\(event_id\),/,
    /state TEXT NOT NULL CHECK \(state IN \('pending', 'published'\)\)/,
    /CREATE TABLE competitive_effects\s*\(\s*event_id TEXT PRIMARY KEY REFERENCES competitive_inbox\(event_id\),/,
    /sequence BIGINT NOT NULL UNIQUE/,
    /WHERE state = 'pending'/,
  ]) {
    if (!pattern.test(eventsSql)) {
      reject(`events.sql does not satisfy ${pattern}`);
    }
  }
  const eventSchema = JSON.parse(inputs["event.schema.json"]);
  if (eventSchema.additionalProperties !== false) {
    reject("event schema must reject undeclared fields");
  }
  for (const field of [
    "schemaVersion",
    "eventId",
    "requestId",
    "traceId",
    "idempotencyKey",
    "sequence",
    "payload",
  ]) {
    if (!eventSchema.required.includes(field)) reject(`event schema must require ${field}`);
  }
  assertEqual(
    eventSchema.properties.payload,
    { type: "string", minLength: 1024, maxLength: 1024 },
    "event payload",
  );
  if (
    contract.scenarios["mq-outbox-inbox"].ingressBroker !== "nats" ||
    contract.scenarios["mq-outbox-inbox"].egressBroker !== "kafka"
  ) {
    reject("MQ topology must exercise NATS ingress and Kafka egress");
  }
  assertEqual(
    contract.scenarios["registry-slow-node-fault"].registryBackends,
    ["etcd", "consul"],
    "registry backends",
  );
  if (
    contract.correctness.requiresExactEcho !== true ||
    contract.correctness.requiresZeroDuplicateEffects !== true
  ) {
    reject("correctness gates must require exact echo and zero duplicate effects");
  }
  assertEqual(
    contract.correctness.requiresContextRoundTrip,
    ["requestId", "traceId", "deadline", "idempotencyKey", "retryBudget"],
    "context round-trip fields",
  );
  if (
    !Array.isArray(contract.correctness.forbiddenOptimizations) ||
    contract.correctness.forbiddenOptimizations.length < 5
  ) {
    reject("forbidden optimization list is incomplete");
  }

  const overlays = JSON.parse(inputs["overlay-manifest.json"]);
  if (overlays.schemaVersion !== 1) reject("overlay manifest schemaVersion must be 1");
  const overlayCoverage = new Set(
    overlays.mappings.map(
      (mapping) =>
        `${mapping.implementation}:${mapping.scenario}:${mapping.project}`,
    ),
  );
  for (const required of [
    "roze:rest-json:rest",
    "roze:unary-rpc:rpc",
    "roze:rest-rpc:rest",
    "go-zero:rest-json:rest",
    "go-zero:unary-rpc:rpc",
    "go-zero:rest-rpc:rest",
  ]) {
    if (!overlayCoverage.has(required)) reject(`overlay coverage missing ${required}`);
  }
  for (const mapping of overlays.mappings) {
    if (
      typeof mapping.source !== "string" ||
      typeof mapping.target !== "string" ||
      mapping.source.includes("..") ||
      mapping.target.includes("..") ||
      path.isAbsolute(mapping.source) ||
      path.isAbsolute(mapping.target)
    ) {
      reject("overlay paths must stay inside their declared project");
    }
    if (!(mapping.source in inputs)) {
      reject(`overlay source is not digest-bound: ${mapping.source}`);
    }
  }
  for (const replacement of overlays.textReplacements || []) {
    if (
      typeof replacement.target !== "string" ||
      replacement.target.includes("..") ||
      path.isAbsolute(replacement.target) ||
      typeof replacement.from !== "string" ||
      typeof replacement.to !== "string" ||
      replacement.from === replacement.to
    ) {
      reject("overlay text replacement is invalid");
    }
  }
  const replacements = new Set(
    (overlays.textReplacements || []).map(
      (replacement) =>
        `${replacement.implementation}:${replacement.project}:${replacement.from}:${replacement.to}`,
    ),
  );
  for (const required of [
    "roze:rest:rest:\n  addr: 127.0.0.1:3000:rest:\n  addr: 127.0.0.1:18080",
    "roze:rpc:rpc:\n  addr: 127.0.0.1:4000:rpc:\n  addr: 127.0.0.1:19090",
    "roze:rpc:# advertise_addr: 127.0.0.1:4000:# advertise_addr: 127.0.0.1:19090",
  ]) {
    if (!replacements.has(required)) reject(`runtime replacement missing ${required}`);
  }
  if (
    !inputs["roze-rpc/src/logic/echo.rs"].includes("payload: req.payload") ||
    !inputs["go-zero-rpc/internal/logic/echologic.go"].includes(
      "Payload: in.Payload",
    )
  ) {
    reject("unary RPC overlays must echo the exact payload");
  }
  for (const fragment of [
    "bytes.Repeat([]byte{'r'}, 1024)",
    "competitive.NewCompetitiveClient(conn).Echo(",
    "bytes.Equal(response.Payload, payload)",
  ]) {
    if (!inputs["probes/grpc-echo/main.go"].includes(fragment)) {
      reject(`gRPC echo probe missing ${fragment}`);
    }
  }
}

function loadDocuments(repositoryRoot = root) {
  const read = (relative) =>
    fs.readFileSync(path.join(repositoryRoot, relative), "utf8");
  const contractDir = "benchmarks/competitive/contracts";
  return {
    baseline: JSON.parse(read("benchmarks/competitive/baseline.yaml")),
    workloads: JSON.parse(read("benchmarks/competitive/workloads.json")),
    contract: JSON.parse(read(`${contractDir}/scenario-contract.json`)),
    inputs: {
      "competitive.api": read(`${contractDir}/competitive.api`),
      "competitive.proto": read(`${contractDir}/competitive.proto`),
      "items.sql": read(`${contractDir}/items.sql`),
      "events.sql": read(`${contractDir}/events.sql`),
      "event.schema.json": read(`${contractDir}/event.schema.json`),
      "overlay-manifest.json": read(
        "benchmarks/competitive/overlays/overlay-manifest.json",
      ),
      "roze-rest/src/logic/echo/echo.rs": read(
        "benchmarks/competitive/overlays/roze-rest/src/logic/echo/echo.rs",
      ),
      "roze-rpc/src/logic/echo.rs": read(
        "benchmarks/competitive/overlays/roze-rpc/src/logic/echo.rs",
      ),
      "roze-rest/src/logic/rpc_echo/rpc_echo.rs": read(
        "benchmarks/competitive/overlays/roze-rest/src/logic/rpc_echo/rpc_echo.rs",
      ),
      "go-zero-rest/internal/logic/echologic.go": read(
        "benchmarks/competitive/overlays/go-zero-rest/internal/logic/echologic.go",
      ),
      "go-zero-rpc/internal/logic/echologic.go": read(
        "benchmarks/competitive/overlays/go-zero-rpc/internal/logic/echologic.go",
      ),
      "go-zero-rpc/etc/competitive.v1.yaml": read(
        "benchmarks/competitive/overlays/go-zero-rpc/etc/competitive.v1.yaml",
      ),
      "go-zero-rest/internal/config/config.go": read(
        "benchmarks/competitive/overlays/go-zero-rest/internal/config/config.go",
      ),
      "go-zero-rest/internal/svc/servicecontext.go": read(
        "benchmarks/competitive/overlays/go-zero-rest/internal/svc/servicecontext.go",
      ),
      "go-zero-rest/internal/logic/rpcechologic.go": read(
        "benchmarks/competitive/overlays/go-zero-rest/internal/logic/rpcechologic.go",
      ),
      "go-zero-rest/etc/competitive-api.yaml": read(
        "benchmarks/competitive/overlays/go-zero-rest/etc/competitive-api.yaml",
      ),
      "probes/grpc-echo/main.go": read(
        "benchmarks/competitive/probes/grpc-echo/main.go",
      ),
    },
  };
}

function competitiveInputDigest(repositoryRoot = root) {
  const hash = crypto.createHash("sha256");
  for (const relative of relativeFiles) {
    const content = fs.readFileSync(path.join(repositoryRoot, relative));
    hash.update(relative);
    hash.update("\0");
    hash.update(String(content.length));
    hash.update("\0");
    hash.update(content);
  }
  return `sha256:${hash.digest("hex")}`;
}

function verifyRepository(repositoryRoot = root) {
  const documents = loadDocuments(repositoryRoot);
  verifyDocuments(
    documents.baseline,
    documents.workloads,
    documents.contract,
    documents.inputs,
  );
  return competitiveInputDigest(repositoryRoot);
}

if (require.main === module) {
  try {
    const digest = verifyRepository();
    if (process.argv.includes("--digest")) process.stdout.write(`${digest}\n`);
    else console.log(`competitive inputs valid: ${digest}`);
  } catch (error) {
    console.error(`competitive inputs invalid: ${error.message}`);
    process.exit(1);
  }
}

module.exports = {
  competitiveInputDigest,
  expectedGeneration,
  loadDocuments,
  verifyDocuments,
  verifyRepository,
};
