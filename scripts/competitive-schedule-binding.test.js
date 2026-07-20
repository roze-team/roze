"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { bind } = require("./competitive-schedule-bind.js");

const dir = fs.mkdtempSync(path.join(os.tmpdir(), "roze-schedule-binding-"));
const write = (name, value) => {
  const file = path.join(dir, name);
  fs.writeFileSync(file, JSON.stringify(value));
  return file;
};
const roze = write("roze.json", {
  runId: "binding-test-1234",
  workloadDigest: "sha256:" + "a".repeat(64),
  environmentFingerprint: "b".repeat(64),
});
const goZero = write("go-zero.json", {
  runId: "binding-test-1234",
  workloadDigest: "sha256:" + "a".repeat(64),
  environmentFingerprint: "b".repeat(64),
});
const workloads = write("workloads.json", { scenarios: [] });
const schedule = write("schedule.json", { schema_version: 1, mode: "pair", pairs: [] });
bind(schedule, roze, goZero, workloads);
const bound = JSON.parse(fs.readFileSync(schedule, "utf8"));
const digest = (file) => `sha256:${crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex")}`;
if (bound.run_id !== "binding-test-1234" || bound.workload_digest !== "sha256:" + "a".repeat(64) ||
    bound.environment_fingerprint !== "b".repeat(64) || bound.roze_sha256 !== digest(roze) ||
    bound.go_zero_sha256 !== digest(goZero)) {
  throw new Error("schedule binding did not record exact document metadata");
}
console.log("competitive schedule binding tests passed");
