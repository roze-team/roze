"use strict";

// Verify the portable evidence bundle emitted by reference-systems-integration.sh.
// This accepts both passed and failed runs: a failed recovery drill is still
// valuable evidence, but its artifact must be complete and tamper-evident.
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

function fail(message) {
  throw new Error(`reference-system evidence invalid: ${message}`);
}

function digest(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function verify(directory, requirePassed = false) {
  const root = path.resolve(directory);
  const runFile = path.join(root, "run.json");
  const summaryFile = path.join(root, "summary.txt");
  const logFile = path.join(root, "integration.log");
  const sumsFile = path.join(root, "SHA256SUMS");
  for (const file of [runFile, summaryFile, logFile, sumsFile]) {
    if (!fs.existsSync(file)) fail(`missing ${path.basename(file)}`);
  }
  let run;
  try {
    run = JSON.parse(fs.readFileSync(runFile, "utf8"));
  } catch (error) {
    fail(`run.json is not valid JSON: ${error.message}`);
  }
  if (run.schema_version !== 1) fail("unsupported schema_version");
  if (run.status !== "passed" && run.status !== "failed") fail("status must be passed or failed");
  if (requirePassed && run.status !== "passed") fail("run did not pass");
  if (!/^[0-9a-f]{40}$/.test(run.revision || "")) fail("revision must be a full Git SHA");
  const started = Date.parse(run.started_at);
  const finished = Date.parse(run.finished_at);
  if (!Number.isFinite(started) || !Number.isFinite(finished) || finished < started) {
    fail("timestamps are invalid");
  }
  if (!Number.isInteger(run.elapsed_seconds) || run.elapsed_seconds < 0) {
    fail("elapsed_seconds must be a non-negative integer");
  }
  const summary = fs.readFileSync(summaryFile, "utf8").trim();
  if (!summary.includes(`status=${run.status}`) || !summary.includes(`revision=${run.revision}`)) {
    fail("summary does not match run.json");
  }
  const expected = new Map();
  for (const line of fs.readFileSync(sumsFile, "utf8").trim().split(/\r?\n/)) {
    const match = line.match(/^([0-9a-f]{64})  (.+)$/);
    if (!match) fail(`invalid checksum line: ${line}`);
    expected.set(match[2], match[1]);
  }
  for (const name of ["integration.log", "run.json", "summary.txt"]) {
    if (expected.get(name) !== digest(path.join(root, name))) fail(`checksum mismatch: ${name}`);
  }
  return run;
}

if (require.main === module) {
  const directory = process.argv[2];
  const requirePassed = process.argv.includes("--require-passed");
  if (!directory) {
    console.error("usage: node scripts/reference-systems-evidence-verify.js <directory> [--require-passed]");
    process.exit(2);
  }
  try {
    const run = verify(directory, requirePassed);
    console.log(`reference-system evidence valid: ${run.status}, revision=${run.revision}`);
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}

module.exports = { verify };
