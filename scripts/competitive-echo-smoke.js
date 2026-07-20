"use strict";

const childProcess = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const net = require("node:net");
const path = require("node:path");
const { competitiveInputDigest, verifyRepository } = require("./competitive-input-verify.js");

const root = path.resolve(__dirname, "..");

function fail(message) {
  throw new Error(message);
}

function executable(name) {
  return process.platform === "win32" ? `${name}.exe` : name;
}

function run(command, args, options = {}) {
  const result = childProcess.spawnSync(command, args, {
    cwd: options.cwd || root,
    encoding: "utf8",
    env: options.env || process.env,
    stdio: options.capture ? "pipe" : "inherit",
    maxBuffer: 128 * 1024 * 1024,
  });
  if (result.error) fail(`cannot run ${command}: ${result.error.message}`);
  if (result.status !== 0) {
    const detail = options.capture
      ? `\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`
      : "";
    fail(`${command} ${args.join(" ")} exited ${result.status}${detail}`);
  }
  return options.capture ? result.stdout.trim() : "";
}

function sha256(file) {
  return `sha256:${crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex")}`;
}

function waitForPort(port, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    const attempt = () => {
      const socket = net.createConnection({ host: "127.0.0.1", port });
      socket.once("connect", () => {
        socket.destroy();
        resolve();
      });
      socket.once("error", () => {
        socket.destroy();
        if (Date.now() >= deadline) reject(new Error(`port ${port} did not open`));
        else setTimeout(attempt, 100);
      });
    };
    attempt();
  });
}

function start(name, command, args, cwd, logDirectory, env = process.env) {
  const stdout = fs.openSync(path.join(logDirectory, `${name}.out.log`), "w");
  const stderr = fs.openSync(path.join(logDirectory, `${name}.err.log`), "w");
  const child = childProcess.spawn(command, args, {
    cwd,
    env,
    windowsHide: true,
    stdio: ["ignore", stdout, stderr],
  });
  child.once("exit", () => {
    fs.closeSync(stdout);
    fs.closeSync(stderr);
  });
  return child;
}

async function stop(child) {
  if (!child || child.exitCode !== null) return;
  child.kill("SIGTERM");
  await Promise.race([
    new Promise((resolve) => child.once("exit", resolve)),
    new Promise((resolve) => setTimeout(resolve, 5000)),
  ]);
  if (child.exitCode === null) child.kill("SIGKILL");
}

async function postEcho(pathname) {
  const payload = "r".repeat(1024);
  const response = await fetch(`http://127.0.0.1:18080${pathname}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ payload }),
    signal: AbortSignal.timeout(2000),
  });
  if (!response.ok) fail(`${pathname} returned HTTP ${response.status}`);
  const body = await response.json();
  if (body.payload !== payload) {
    fail(`${pathname} payload mismatch`);
  }
  return { path: pathname, status: response.status, payloadBytes: payload.length };
}

async function exercise(
  implementation,
  rpcCommand,
  rpcArgs,
  rpcCwd,
  restCommand,
  restArgs,
  restCwd,
  probe,
  logDirectory,
  env,
) {
  let rpc;
  let rest;
  try {
    rpc = start(`${implementation}-rpc`, rpcCommand, rpcArgs, rpcCwd, logDirectory, env);
    await waitForPort(19090);
    const grpc = run(probe, ["-endpoint", "127.0.0.1:19090"], {
      capture: true,
      env,
    });
    rest = start(
      `${implementation}-rest`,
      restCommand,
      restArgs,
      restCwd,
      logDirectory,
      env,
    );
    await waitForPort(18080);
    const restEcho = await postEcho("/v1/echo");
    const restRpcEcho = await postEcho("/v1/rpc-echo");
    return { implementation, grpc, restEcho, restRpcEcho };
  } finally {
    await stop(rest);
    await stop(rpc);
  }
}

async function main() {
  verifyRepository();
  const build = process.argv[2] && path.resolve(process.argv[2]);
  if (!build) {
    fail("usage: node competitive-echo-smoke.js <competitive-source-build-dir>");
  }
  const manifestPath = path.join(build, "artifact-manifest.json");
  if (!fs.existsSync(manifestPath)) fail("artifact-manifest.json is missing");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  if (manifest.workloadDigest !== competitiveInputDigest()) {
    fail("source artifact workloadDigest does not match current competitive inputs");
  }

  const smokeRoot = path.resolve(
    process.env.ROZE_COMPETITIVE_SMOKE_DIR || `${build}-echo-smoke`,
  );
  if (fs.existsSync(smokeRoot)) fail(`smoke output already exists: ${smokeRoot}`);
  fs.mkdirSync(smokeRoot, { recursive: true });
  const bin = path.join(smokeRoot, "bin");
  const logs = path.join(smokeRoot, "logs");
  fs.mkdirSync(bin);
  fs.mkdirSync(logs);

  const workspace = path.join(build, "roze-workspace");
  const rozeRest = path.join(workspace, "apps", "competitive-roze-rest");
  const rozeRpc = path.join(workspace, "apps", "competitive-roze-rpc");
  const goZeroRest = path.join(build, "go-zero-rest");
  const goZeroRpc = path.join(build, "go-zero-rpc");
  const cargoTarget =
    process.env.ROZE_COMPETITIVE_CARGO_TARGET_DIR ||
    path.join(root, "target", "competitive-source-cargo");
  const cargoEnvironment = { ...process.env, CARGO_TARGET_DIR: cargoTarget };
  for (const project of [rozeRpc, rozeRest]) {
    run(
      "cargo",
      ["build", "--locked", "--manifest-path", path.join(project, "Cargo.toml")],
      { env: cargoEnvironment },
    );
  }

  const goEnvironment = { ...process.env, GOTELEMETRY: "off" };
  const goRpcBin = path.join(bin, executable("go-zero-rpc"));
  const goRestBin = path.join(bin, executable("go-zero-rest"));
  run("go", ["build", "-trimpath", "-o", goRpcBin, "."], {
    cwd: goZeroRpc,
    env: goEnvironment,
  });
  run("go", ["build", "-trimpath", "-o", goRestBin, "."], {
    cwd: goZeroRest,
    env: goEnvironment,
  });

  const probeRoot = path.join(smokeRoot, "probe");
  fs.mkdirSync(probeRoot);
  fs.copyFileSync(
    path.join(root, "benchmarks", "competitive", "probes", "grpc-echo", "main.go"),
    path.join(probeRoot, "main.go"),
  );
  fs.writeFileSync(
    path.join(probeRoot, "go.mod"),
    [
      "module competitive-echo-probe",
      "",
      "go 1.26.2",
      "",
      "require go-zero-rpc v0.0.0",
      "",
      `replace go-zero-rpc => ${goZeroRpc.replaceAll("\\", "/")}`,
      "",
    ].join("\n"),
    "utf8",
  );
  run("go", ["mod", "tidy"], { cwd: probeRoot, env: goEnvironment });
  const probe = path.join(bin, executable("grpc-echo-probe"));
  run("go", ["build", "-trimpath", "-o", probe, "."], {
    cwd: probeRoot,
    env: goEnvironment,
  });

  const rozeRpcBin = path.join(cargoTarget, "debug", executable("competitive-roze-rpc"));
  const rozeRestBin = path.join(cargoTarget, "debug", executable("competitive-roze-rest"));
  const startedAt = new Date().toISOString();
  const results = [];
  results.push(
    await exercise(
      "roze",
      rozeRpcBin,
      [],
      rozeRpc,
      rozeRestBin,
      [],
      rozeRest,
      probe,
      logs,
      cargoEnvironment,
    ),
  );
  results.push(
    await exercise(
      "go-zero",
      goRpcBin,
      ["-f", path.join(goZeroRpc, "etc", "competitive.v1.yaml")],
      goZeroRpc,
      goRestBin,
      ["-f", path.join(goZeroRest, "etc", "competitive-api.yaml")],
      goZeroRest,
      probe,
      logs,
      goEnvironment,
    ),
  );
  const report = {
    schemaVersion: 1,
    workloadDigest: competitiveInputDigest(),
    sourceManifestDigest: sha256(manifestPath),
    startedAt,
    finishedAt: new Date().toISOString(),
    results,
  };
  const reportPath = path.join(smokeRoot, "echo-smoke.json");
  fs.writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
  console.log(`competitive echo smoke passed: ${reportPath}`);
}

main().catch((error) => {
  console.error(`competitive echo smoke failed: ${error.message}`);
  process.exit(1);
});
