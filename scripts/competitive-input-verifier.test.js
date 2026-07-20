"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const {
  competitiveInputDigest,
  loadDocuments,
  verifyDocuments,
} = require("./competitive-input-verify.js");

function cloneDocuments() {
  const loaded = loadDocuments();
  return {
    baseline: structuredClone(loaded.baseline),
    workloads: structuredClone(loaded.workloads),
    contract: structuredClone(loaded.contract),
    inputs: { ...loaded.inputs },
  };
}

function verify(documents) {
  verifyDocuments(
    documents.baseline,
    documents.workloads,
    documents.contract,
    documents.inputs,
  );
}

test("accepts the shared competitive input package", () => {
  assert.doesNotThrow(() => verify(cloneDocuments()));
  assert.match(competitiveInputDigest(), /^sha256:[0-9a-f]{64}$/);
});

test("rejects workload and scenario drift", () => {
  const documents = cloneDocuments();
  documents.contract.scenarios["db-cache-aside"].datasetRows = 99999;
  assert.throws(() => verify(documents), /datasetRows drift/);
});

test("rejects framework-specific generation command drift", () => {
  const documents = cloneDocuments();
  documents.contract.generation["go-zero"].rest.push("--experimental-fast-path");
  assert.throws(() => verify(documents), /generation commands/);
});

test("rejects a smaller or non-deterministic SQL dataset", () => {
  const documents = cloneDocuments();
  documents.inputs["items.sql"] = documents.inputs["items.sql"].replace(
    "generate_series(1, 100000)",
    "generate_series(1, 1000)",
  );
  assert.throws(() => verify(documents), /items.sql/);
});

test("rejects relaxed event payload correctness", () => {
  const documents = cloneDocuments();
  const schema = JSON.parse(documents.inputs["event.schema.json"]);
  schema.properties.payload.maxLength = 2048;
  documents.inputs["event.schema.json"] = JSON.stringify(schema);
  assert.throws(() => verify(documents), /event payload/);
});

test("rejects wire-size and application-payload ambiguity", () => {
  const documents = cloneDocuments();
  documents.workloads.global.sizeBasis = "wire-bytes";
  assert.throws(() => verify(documents), /size basis/);
});

test("rejects removal of persistent inbox uniqueness", () => {
  const documents = cloneDocuments();
  documents.inputs["events.sql"] = documents.inputs["events.sql"].replace(
    "event_id TEXT PRIMARY KEY",
    "event_id TEXT NOT NULL",
  );
  assert.throws(() => verify(documents), /events.sql/);
});

test("rejects asymmetric application overlay coverage", () => {
  const documents = cloneDocuments();
  const manifest = JSON.parse(documents.inputs["overlay-manifest.json"]);
  manifest.mappings = manifest.mappings.filter(
    (mapping) =>
      !(mapping.implementation === "go-zero" && mapping.scenario === "unary-rpc"),
  );
  documents.inputs["overlay-manifest.json"] = JSON.stringify(manifest);
  assert.throws(() => verify(documents), /overlay coverage/);
});
