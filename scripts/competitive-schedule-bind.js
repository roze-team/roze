"use strict";

// Bind a schedule manifest to the exact raw documents it orders. The shared
// executor owns timing/order; this post-run binding makes replacement of either
// document after execution detectable by the schedule verifier.
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

function sha256(file) {
  return `sha256:${crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex")}`;
}

function bind(scheduleFile, rozeFile, goZeroFile, workloadsFile) {
  const schedule = JSON.parse(fs.readFileSync(scheduleFile, "utf8"));
  const roze = JSON.parse(fs.readFileSync(rozeFile, "utf8"));
  const goZero = JSON.parse(fs.readFileSync(goZeroFile, "utf8"));
  JSON.parse(fs.readFileSync(workloadsFile, "utf8"));
  if (roze.runId !== goZero.runId) throw new Error("raw documents have different runId");
  if (roze.workloadDigest !== goZero.workloadDigest) {
    throw new Error("raw documents have different workloadDigest");
  }
  if (roze.environmentFingerprint !== goZero.environmentFingerprint) {
    throw new Error("raw documents have different environmentFingerprint");
  }
  schedule.schema_version = 1;
  schedule.mode = "pair";
  schedule.run_id = roze.runId;
  schedule.workload_digest = roze.workloadDigest;
  schedule.environment_fingerprint = roze.environmentFingerprint;
  schedule.roze_sha256 = sha256(rozeFile);
  schedule.go_zero_sha256 = sha256(goZeroFile);
  fs.writeFileSync(scheduleFile, `${JSON.stringify(schedule, null, 2)}\n`);
}

if (require.main === module) {
  const [schedule, roze, goZero, workloads] = process.argv.slice(2);
  if (!schedule || !roze || !goZero || !workloads) {
    console.error("usage: node competitive-schedule-bind.js <schedule> <roze> <go-zero> <workloads>");
    process.exit(2);
  }
  try {
    bind(...[schedule, roze, goZero, workloads].map((file) => path.resolve(file)));
  } catch (error) {
    console.error(`competitive schedule binding failed: ${error.message}`);
    process.exit(1);
  }
}

module.exports = { bind };
