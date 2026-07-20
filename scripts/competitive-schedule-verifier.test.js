"use strict";

const { validateSchedule } = require("./competitive-schedule-verify.js");

const scenarios = ["rest-crud", "rest-rpc", "db-cache", "mq-outbox", "context", "fault"];
const workloads = { scenarios: scenarios.map((id) => ({ id })) };
const samples = () => Array.from({ length: 5 }, (_, index) => ({ sample_index: index }));
const implementation = {
  runId: "schedule-test-1234",
  workloadDigest: "sha256:" + "a".repeat(64),
  environmentFingerprint: "b".repeat(64),
  scenarios: scenarios.map((id) => ({ id, samples: samples() })),
};
const pairs = [];
for (const scenario of scenarios) {
  for (let sample_index = 0; sample_index < 5; sample_index += 1) {
    pairs.push({
      scenario,
      sample_index,
      order: pairs.length % 2 === 0 ? ["roze", "go-zero"] : ["go-zero", "roze"],
    });
  }
}

const valid = {
  schema_version: 1,
  mode: "pair",
  run_id: "schedule-test-1234",
  workload_digest: implementation.workloadDigest,
  environment_fingerprint: implementation.environmentFingerprint,
  roze_sha256: "sha256:" + "c".repeat(64),
  go_zero_sha256: "sha256:" + "d".repeat(64),
  pairs,
};
if (validateSchedule(valid, implementation, implementation, workloads) !== 30) {
  throw new Error("valid schedule returned the wrong pair count");
}

const duplicate = {
  ...valid,
  pairs: [...pairs.slice(0, -1), pairs[0]],
};
try {
  validateSchedule(duplicate, implementation, implementation, workloads);
  throw new Error("duplicate schedule pair was accepted");
} catch (error) {
  if (!String(error.message).includes("duplicate pair")) throw error;
}

const uneven = {
  ...valid,
  pairs: pairs.filter((pair) => !(pair.scenario === "rest-crud" && pair.sample_index === 4)),
};
const unevenGoZero = {
  ...implementation,
  scenarios: implementation.scenarios.map((scenario, index) =>
    index === 0 ? { ...scenario, samples: scenario.samples.slice(0, 4) } : scenario,
  ),
};
try {
  validateSchedule(uneven, implementation, unevenGoZero, workloads);
  throw new Error("uneven implementation samples were accepted");
} catch (error) {
  if (!String(error.message).includes("sample counts differ")) throw error;
}

console.log("competitive schedule verifier tests passed");
