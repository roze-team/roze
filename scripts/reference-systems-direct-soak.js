"use strict";

// Fixed-duration dependency soak for the Linux diagnostic runner. This is
// intentionally separate from production-soak-ci.js: it exercises only the
// five direct reference probes and never upgrades their result to release
// maturity.

const childProcess = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const durations = { "24h": 86400, "72h": 259200 };
const durationName = process.argv[2];
const output = path.resolve(
  process.argv[3] || path.join(root, "target", `reference-systems-direct-${durationName || "invalid"}`),
);

function fail(message) {
  console.error(`reference direct soak failed: ${message}`);
  process.exit(2);
}

if (!(durationName in durations)) {
  fail("usage: node reference-systems-direct-soak.js <24h|72h> [output]");
}
const dryRun = process.env.ROZE_DIRECT_SOAK_DRY_RUN === "1";
if (!dryRun && process.platform !== "linux") fail("the fixed soak runner must run on Linux");
if (fs.existsSync(output)) fail(`output already exists: ${output}`);
fs.mkdirSync(output, { recursive: true });

const requiredSeconds = durations[durationName];
const intervalSeconds = Number(process.env.ROZE_DIRECT_SOAK_INTERVAL_SECONDS || 300);
if (!Number.isInteger(intervalSeconds) || intervalSeconds < 30) {
  fail("ROZE_DIRECT_SOAK_INTERVAL_SECONDS must be an integer >= 30");
}
if (dryRun) {
  console.log(JSON.stringify({ duration: durationName, requiredSeconds, intervalSeconds }));
  process.exit(0);
}

const profile = process.env.ROZE_REFERENCE_DIRECT_PROFILE || "managed-services";
const env = {
  ...process.env,
  ROZE_REFERENCE_DIRECT_PROFILE: profile,
  ROZE_REFERENCE_DIRECT_EVIDENCE_DIR: path.join(output, "latest-probe"),
};
if (process.env.ROZE_DIRECT_SOAK_MINIO_BIN) {
  env.ROZE_TEST_S3_ENDPOINT = "http://127.0.0.1:19000";
  env.ROZE_TEST_S3_ACCESS_KEY = process.env.ROZE_DIRECT_SOAK_S3_ACCESS_KEY || "rozeadmin";
  env.ROZE_TEST_S3_SECRET_KEY = process.env.ROZE_DIRECT_SOAK_S3_SECRET_KEY || "diagnostic-only-secret";
}
if (process.env.ROZE_DIRECT_SOAK_REDIS_BIN) {
  env.ROZE_TEST_REDIS_URL = `redis://:${process.env.ROZE_DIRECT_SOAK_REDIS_PASSWORD || "diagnostic-only-secret"}@127.0.0.1:16379`;
}
const startedAt = new Date().toISOString();
const startedEpoch = Math.floor(Date.now() / 1000);
const revision = childProcess
  .execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" })
  .trim();
const expectedRevision = process.env.ROZE_DIRECT_SOAK_EXPECTED_REVISION || "";
if (expectedRevision && expectedRevision !== revision) {
  fail(`HEAD ${revision} does not match ROZE_DIRECT_SOAK_EXPECTED_REVISION ${expectedRevision}`);
}
const samples = [];
const children = [];
let stopping = false;

function startOptional(command, args, name, extraEnv = {}) {
  if (!process.env[name]) return;
  const child = childProcess.spawn(command, args, {
    cwd: root,
    env: { ...process.env, ...extraEnv },
    stdio: ["ignore", fs.openSync(path.join(output, `${name}.out.log`), "a"), fs.openSync(path.join(output, `${name}.err.log`), "a")],
  });
  children.push(child);
}

// The caller may provide these binaries for isolated localhost diagnostics;
// leaving them unset means the runner uses externally managed services.
startOptional(
  process.env.ROZE_DIRECT_SOAK_MINIO_BIN,
  [
    "server",
    process.env.ROZE_DIRECT_SOAK_MINIO_DATA || path.join(output, "minio-data"),
    "--address",
    "127.0.0.1:19000",
    "--console-address",
    "127.0.0.1:19001",
  ],
  "ROZE_DIRECT_SOAK_MINIO_BIN",
  {
    MINIO_ROOT_USER: process.env.ROZE_DIRECT_SOAK_S3_ACCESS_KEY || "rozeadmin",
    MINIO_ROOT_PASSWORD: process.env.ROZE_DIRECT_SOAK_S3_SECRET_KEY || "diagnostic-only-secret",
  },
);
startOptional(
  process.env.ROZE_DIRECT_SOAK_REDIS_BIN,
  [
    "--daemonize",
    "no",
    "--bind",
    "127.0.0.1",
    "--port",
    "16379",
    "--requirepass",
    process.env.ROZE_DIRECT_SOAK_REDIS_PASSWORD || "diagnostic-only-secret",
    "--appendonly",
    "no",
  ],
  "ROZE_DIRECT_SOAK_REDIS_BIN",
);

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function stopChildren() {
  for (const child of children) {
    if (child.exitCode === null) child.kill("SIGTERM");
  }
}

function cleanup() {
  if (stopping) return;
  stopping = true;
  stopChildren();
}

process.on("SIGINT", () => {
  cleanup();
  process.exitCode = 130;
});
process.on("SIGTERM", () => {
  cleanup();
  process.exitCode = 143;
});
process.on("exit", cleanup);

async function sleep(seconds) {
  await new Promise((resolve) => setTimeout(resolve, seconds * 1000));
}

async function main() {
  if (process.env.ROZE_DIRECT_SOAK_MINIO_BIN) {
    await sleep(3);
    const accessKey = process.env.ROZE_DIRECT_SOAK_S3_ACCESS_KEY || "rozeadmin";
    const secretKey = process.env.ROZE_DIRECT_SOAK_S3_SECRET_KEY || "diagnostic-only-secret";
    const bucket = process.env.ROZE_TEST_S3_BUCKET || "roze";
    const bucketResult = childProcess.spawnSync(
      "aws",
      ["--endpoint-url", "http://127.0.0.1:19000", "s3", "mb", `s3://${bucket}`],
      {
        env: {
          ...process.env,
          AWS_ACCESS_KEY_ID: accessKey,
          AWS_SECRET_ACCESS_KEY: secretKey,
          AWS_DEFAULT_REGION: "us-east-1",
        },
        encoding: "utf8",
      },
    );
    if (bucketResult.status !== 0 && !String(bucketResult.stderr).includes("BucketAlreadyOwnedByYou")) {
      fail(`cannot create diagnostic S3 bucket: ${bucketResult.stderr}`);
    }
  }
  writeJson(path.join(output, "runner.json"), {
    schema_version: 1,
    revision,
    duration: durationName,
    required_seconds: requiredSeconds,
    interval_seconds: intervalSeconds,
    profile,
    started_at: startedAt,
    platform: process.platform,
    arch: process.arch,
    node: process.version,
  });
  const log = fs.createWriteStream(path.join(output, "samples.jsonl"), { flags: "a" });
  let nextAt = startedEpoch;
  let failed = 0;
  try {
    while (!stopping && Math.floor(Date.now() / 1000) - startedEpoch < requiredSeconds) {
      const iteration = samples.length + 1;
      const probeDir = path.join(output, `probe-${String(iteration).padStart(5, "0")}`);
      fs.mkdirSync(probeDir, { recursive: true });
      const probeEnv = {
        ...env,
        ROZE_REFERENCE_DIRECT_EVIDENCE_DIR: probeDir,
      };
      const stdoutPath = path.join(probeDir, "runner.stdout.log");
      const stderrPath = path.join(probeDir, "runner.stderr.log");
      const stdoutFd = fs.openSync(stdoutPath, "w");
      const stderrFd = fs.openSync(stderrPath, "w");
      const result = childProcess.spawnSync("bash", ["scripts/reference-systems-direct.sh"], {
        cwd: root,
        env: probeEnv,
        stdio: ["ignore", stdoutFd, stderrFd],
        timeout: Math.max(120000, intervalSeconds * 1000 - 5000),
      });
      fs.closeSync(stdoutFd);
      fs.closeSync(stderrFd);
      const stdout = fs.readFileSync(stdoutPath, "utf8");
      const stderr = fs.readFileSync(stderrPath, "utf8");
      const status = result.status === 0 ? "passed" : "failed";
      if (status === "failed") failed += 1;
      const sample = {
        iteration,
        started_at: new Date().toISOString(),
        status,
        exit_code: result.status,
        stdout_sha256: crypto.createHash("sha256").update(stdout).digest("hex"),
        stderr_sha256: crypto.createHash("sha256").update(stderr).digest("hex"),
      };
      samples.push(sample);
      log.write(`${JSON.stringify(sample)}\n`);
      nextAt += intervalSeconds;
      const wait = Math.max(0, nextAt - Math.floor(Date.now() / 1000));
      if (wait > 0) await sleep(wait);
    }
  } finally {
    log.end();
    cleanup();
  }
  const finishedAt = new Date().toISOString();
  const elapsedSeconds = Math.floor(Date.now() / 1000) - startedEpoch;
  const finalStatus = elapsedSeconds >= requiredSeconds && failed === 0 ? "passed" : "failed";
  writeJson(path.join(output, "run.json"), {
    schema_version: 1,
    status: finalStatus,
    revision,
    duration: durationName,
    required_seconds: requiredSeconds,
    elapsed_seconds: elapsedSeconds,
    failed_samples: failed,
    sample_count: samples.length,
    started_at: startedAt,
    finished_at: finishedAt,
    profile,
  });
  const files = ["runner.json", "samples.jsonl", "run.json"];
  fs.writeFileSync(
    path.join(output, "SHA256SUMS"),
    `${files.map((file) => `${crypto.createHash("sha256").update(fs.readFileSync(path.join(output, file))).digest("hex")}  ${file}`).join("\n")}\n`,
    "utf8",
  );
  console.log(`reference direct soak ${finalStatus}: ${output}`);
  if (finalStatus !== "passed") process.exitCode = 1;
}

main().catch((error) => {
  cleanup();
  console.error(`reference direct soak failed: ${error.stack || error.message}`);
  process.exitCode = 1;
});
