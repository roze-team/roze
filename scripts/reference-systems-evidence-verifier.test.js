"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { verify } = require("./reference-systems-evidence-verify.js");

const dir = fs.mkdtempSync(path.join(os.tmpdir(), "roze-reference-evidence-"));
const revision = "a".repeat(40);
const write = (name, content) => {
  fs.writeFileSync(path.join(dir, name), content);
};
write("integration.log", "recovery failed after injected outage\n");
const run = {
  schema_version: 1,
  status: "failed",
  revision,
  started_at: "2026-07-18T00:00:00Z",
  finished_at: "2026-07-18T00:00:02Z",
  elapsed_seconds: 2,
};
write("run.json", JSON.stringify(run) + "\n");
write("summary.txt", `status=failed revision=${revision} elapsed_seconds=2\n`);
const sha = (name) => crypto.createHash("sha256").update(fs.readFileSync(path.join(dir, name))).digest("hex");
write("SHA256SUMS", ["integration.log", "run.json", "summary.txt"].map((name) => `${sha(name)}  ${name}`).join("\n") + "\n");
if (verify(dir).status !== "failed") throw new Error("failed evidence was not accepted");
try {
  verify(dir, true);
  throw new Error("failed evidence passed --require-passed");
} catch (error) {
  if (!String(error.message).includes("run did not pass")) throw error;
}
fs.appendFileSync(path.join(dir, "summary.txt"), "tampered\n");
try {
  verify(dir);
  throw new Error("tampered evidence was accepted");
} catch (error) {
  if (!String(error.message).includes("checksum mismatch")) throw error;
}
console.log("reference-system evidence verifier tests passed");
