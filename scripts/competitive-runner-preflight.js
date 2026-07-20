"use strict";

// Evidence runs must fail closed before an executor can reuse stale output or
// run on a host that cannot satisfy the pinned baseline. This script performs
// no cleanup and is intentionally safe to run from CI or a developer shell.
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const baseline = JSON.parse(
  fs.readFileSync(path.join(root, "benchmarks", "competitive", "baseline.yaml"), "utf8"),
);

function fail(message) {
  console.error(`competitive runner preflight failed: ${message}`);
  process.exit(1);
}

if (process.platform !== "linux") fail("authoritative runner must be Linux");
if (process.arch !== "x64") fail("authoritative runner must be x86_64");

const executor = process.env.ROZE_COMPETITIVE_EXECUTOR || "";
if (!executor) fail("ROZE_COMPETITIVE_EXECUTOR is required");
let executorStat;
try {
  executorStat = fs.statSync(executor);
} catch (error) {
  fail(`executor is not accessible: ${error.message}`);
}
if (!executorStat.isFile()) fail("ROZE_COMPETITIVE_EXECUTOR must be a regular file");
if ((executorStat.mode & 0o111) === 0) fail("ROZE_COMPETITIVE_EXECUTOR must be executable");

const outputDir = path.resolve(
  process.env.ROZE_COMPETITIVE_OUTPUT_DIR || path.join(root, "target", "competitive"),
);
fs.mkdirSync(outputDir, { recursive: true });
for (const name of ["roze.json", "go-zero.json", "schedule.json", "report.json"]) {
  const file = path.join(outputDir, name);
  if (fs.existsSync(file)) {
    fail(`output directory contains stale ${name}; use a fresh run directory`);
  }
}

for (const dependency of baseline.dependencies) {
  const digest = process.env[dependency.digestEnvironment] || "";
  if (!/^sha256:[0-9a-f]{64}$/.test(digest)) {
    fail(`${dependency.digestEnvironment} must contain a pinned sha256 digest`);
  }
}

const logicalCpus = os.cpus().length;
if (!Number.isInteger(logicalCpus) || logicalCpus < 1) fail("unable to determine logical CPU count");
const affinity = baseline.runner.cpuAffinity;
if (!/^[0-9,-]+$/.test(affinity)) fail("baseline cpuAffinity is invalid");

// A fixed runner must identify itself; the executor records the values in the
// raw sample, while these checks prevent accidental local runs from looking
// like production evidence.
if (process.env.ROZE_COMPETITIVE_RUN_ID &&
    !/^[A-Za-z0-9][A-Za-z0-9._-]{7,127}$/.test(process.env.ROZE_COMPETITIVE_RUN_ID)) {
  fail("ROZE_COMPETITIVE_RUN_ID must be 8-128 safe identifier characters");
}

console.log(`competitive runner preflight valid: linux/x86_64, executor=${executor}, output=${outputDir}`);
