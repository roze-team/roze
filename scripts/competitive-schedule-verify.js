"use strict";

// Validate the schedule manifest emitted by the shared pair executor.  The
// report verifier checks timestamps, while this predicate makes the executor's
// declared ordering auditable and prevents a missing schedule from being
// treated as an ordinary two-run benchmark.
const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");

function fail(message) {
  throw new Error(`competitive schedule invalid: ${message}`);
}

function sha256(file) {
  return `sha256:${crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex")}`;
}

function validateSchedule(schedule, roze, goZero, workloads, bindings = {}) {

if (schedule.schema_version !== 1 || schedule.mode !== "pair") fail("schema_version/mode mismatch");
if (typeof schedule.run_id !== "string" || schedule.run_id.length < 8) fail("run_id is missing");
if (schedule.run_id !== roze.runId || schedule.run_id !== goZero.runId) fail("run_id is not bound to both documents");
if (schedule.workload_digest !== roze.workloadDigest || schedule.workload_digest !== goZero.workloadDigest) {
  fail("workload_digest is not bound to both documents");
}
if (schedule.environment_fingerprint !== roze.environmentFingerprint ||
    schedule.environment_fingerprint !== goZero.environmentFingerprint) {
  fail("environment_fingerprint is not bound to both documents");
}
if (bindings.rozeFile && schedule.roze_sha256 !== sha256(bindings.rozeFile)) fail("roze_sha256 mismatch");
if (bindings.goZeroFile && schedule.go_zero_sha256 !== sha256(bindings.goZeroFile)) fail("go_zero_sha256 mismatch");
if (!/^sha256:[0-9a-f]{64}$/.test(schedule.roze_sha256 || "") ||
    !/^sha256:[0-9a-f]{64}$/.test(schedule.go_zero_sha256 || "")) fail("document bindings are missing");
if (!Array.isArray(schedule.pairs) || schedule.pairs.length === 0) fail("pairs must be non-empty");

const scenarios = new Map((workloads.scenarios || []).map((item) => [item.id, item]));
if (scenarios.size !== 6) fail("the fixed contract must contain six scenarios");
const rozeScenarios = new Map((roze.scenarios || []).map((item) => [item.id, item]));
const goZeroScenarios = new Map((goZero.scenarios || []).map((item) => [item.id, item]));
const seen = new Set();
let rozeFirst = 0;
let goZeroFirst = 0;

for (const pair of schedule.pairs) {
  if (!pair || typeof pair.scenario !== "string" || !scenarios.has(pair.scenario)) {
    fail("pair references an unknown scenario");
  }
  if (!Number.isInteger(pair.sample_index) || pair.sample_index < 0) fail("sample_index is invalid");
  if (!Array.isArray(pair.order) || pair.order.length !== 2 ||
      !pair.order.every((value) => value === "roze" || value === "go-zero") ||
      pair.order[0] === pair.order[1]) {
    fail("pair order must contain Roze and go-zero exactly once");
  }
  const key = `${pair.scenario}:${pair.sample_index}`;
  if (seen.has(key)) fail(`duplicate pair ${key}`);
  seen.add(key);
  if (pair.order[0] === "roze") rozeFirst += 1;
  else goZeroFirst += 1;
  const rozeSamples = rozeScenarios.get(pair.scenario)?.samples || [];
  const goZeroSamples = goZeroScenarios.get(pair.scenario)?.samples || [];
  if (!rozeSamples[pair.sample_index] || !goZeroSamples[pair.sample_index]) {
    fail(`pair ${key} has no matching samples in both implementations`);
  }
}

for (const scenario of scenarios.keys()) {
  const rozeCount = rozeScenarios.get(scenario)?.samples?.length || 0;
  const goZeroCount = goZeroScenarios.get(scenario)?.samples?.length || 0;
  if (rozeCount !== goZeroCount) fail(`${scenario} sample counts differ between implementations`);
  const count = [...seen].filter((key) => key.startsWith(`${scenario}:`)).length;
  const expected = rozeCount;
  if (count !== expected || count < 5) fail(`${scenario} must pair every sample and have at least five pairs`);
}
if (Math.abs(rozeFirst - goZeroFirst) > 1) fail("pair order is not counterbalanced");
return schedule.pairs.length;
}

if (require.main === module) {
  const [schedulePath, rozePath, goZeroPath, workloadsPath] = process.argv.slice(2);
  if (!schedulePath || !rozePath || !goZeroPath || !workloadsPath) {
    console.error("usage: node scripts/competitive-schedule-verify.js <schedule> <roze> <go-zero> <workloads>");
    process.exit(2);
  }
  const readJson = (file) => JSON.parse(fs.readFileSync(path.resolve(file), "utf8"));
  const count = validateSchedule(readJson(schedulePath), readJson(rozePath), readJson(goZeroPath), readJson(workloadsPath), {
    rozeFile: path.resolve(rozePath),
    goZeroFile: path.resolve(goZeroPath),
  });
  console.log(`competitive schedule valid: ${count} adjacent pairs`);
}

module.exports = { validateSchedule };
