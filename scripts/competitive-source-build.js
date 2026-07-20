"use strict";

const childProcess = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const {
  competitiveInputDigest,
  loadDocuments,
  verifyRepository,
} = require("./competitive-input-verify.js");

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

function capture(command, args, cwd = root) {
  return run(command, args, { cwd, capture: true });
}

function revision(repository) {
  return capture("git", ["rev-parse", "HEAD"], repository);
}

function requireClean(repository, label) {
  if (capture("git", ["status", "--porcelain"], repository) !== "") {
    fail(`${label} source must be clean for a structural build`);
  }
}

function parseVersion(output, pattern, label) {
  const match = output.match(pattern);
  if (!match) fail(`cannot parse ${label} version from: ${output}`);
  return match[1];
}

function verifyToolchains(expected) {
  const actual = {
    rust: parseVersion(capture("rustc", ["--version"]), /^rustc ([^\s]+)/, "rustc"),
    cargo: parseVersion(capture("cargo", ["--version"]), /^cargo ([^\s]+)/, "cargo"),
    go: parseVersion(capture("go", ["version"]), /\bgo([0-9][^\s]*)/, "go"),
    node: process.versions.node,
    protoc: parseVersion(
      capture("protoc", ["--version"]),
      /^libprotoc ([^\s]+)/,
      "protoc",
    ),
  };
  for (const [name, version] of Object.entries(expected)) {
    if (actual[name] !== version) {
      fail(`${name} version drift: expected ${version}, got ${actual[name]}`);
    }
  }
  return actual;
}

function freshDirectory(directory) {
  if (fs.existsSync(directory)) {
    fail(`output already exists; refusing a non-fresh build: ${directory}`);
  }
  fs.mkdirSync(directory, { recursive: true });
}

function workspaceManifest(projects) {
  const source = fs.readFileSync(path.join(root, "Cargo.toml"), "utf8");
  const marker = "[workspace.package]";
  const markerIndex = source.indexOf(marker);
  if (markerIndex < 0) fail("repository Cargo.toml lacks [workspace.package]");
  const members = projects
    .map((project) => `    "apps/${project}",`)
    .join("\n");
  return `[workspace]\nmembers = [\n${members}\n]\nresolver = "2"\n\n${source.slice(markerIndex)}`;
}

function prepareRozeWorkspace(workspace, projects) {
  fs.mkdirSync(path.join(workspace, "apps"), { recursive: true });
  fs.writeFileSync(
    path.join(workspace, "Cargo.toml"),
    workspaceManifest(projects),
    "utf8",
  );
  fs.copyFileSync(path.join(root, "Cargo.lock"), path.join(workspace, "Cargo.lock"));
  fs.symlinkSync(
    path.join(root, "crates"),
    path.join(workspace, "crates"),
    process.platform === "win32" ? "junction" : "dir",
  );
}

function substitute(argv, values) {
  return argv.map((argument) =>
    argument.replace(/\{([^}]+)\}/g, (_, name) => {
      if (!(name in values)) fail(`unknown generation placeholder: ${name}`);
      return values[name];
    }),
  );
}

function runContractCommand(argv, values, replacements = {}, options = {}) {
  const expanded = substitute(argv, values);
  const command = replacements[expanded[0]] || expanded[0];
  run(command, expanded.slice(1), options);
}

function fileDigest(file) {
  const content = fs.readFileSync(file);
  return `sha256:${crypto.createHash("sha256").update(content).digest("hex")}`;
}

function directoryFiles(directory) {
  const files = [];
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...directoryFiles(absolute));
    else if (entry.isFile()) files.push(absolute);
  }
  return files.sort();
}

function artifactManifest(output, metadata) {
  const manifestPath = path.join(output, "artifact-manifest.json");
  const files = directoryFiles(output)
    .filter((file) => file !== manifestPath)
    .map((file) => ({
      path: path.relative(output, file).split(path.sep).join("/"),
      bytes: fs.statSync(file).size,
      digest: fileDigest(file),
    }));
  return { ...metadata, files };
}

function installOverlays(projects) {
  const overlayRoot = path.join(root, "benchmarks", "competitive", "overlays");
  const manifest = JSON.parse(
    fs.readFileSync(path.join(overlayRoot, "overlay-manifest.json"), "utf8"),
  );
  for (const mapping of manifest.mappings) {
    const project = projects[`${mapping.implementation}:${mapping.project}`];
    if (!project) fail(`unknown overlay project ${mapping.implementation}:${mapping.project}`);
    const source = path.resolve(overlayRoot, mapping.source);
    const target = path.resolve(project, mapping.target);
    if (!source.startsWith(`${overlayRoot}${path.sep}`)) {
      fail(`overlay source escapes root: ${mapping.source}`);
    }
    if (!target.startsWith(`${project}${path.sep}`)) {
      fail(`overlay target escapes project: ${mapping.target}`);
    }
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.copyFileSync(source, target);
  }
  for (const replacement of manifest.textReplacements || []) {
    const project =
      projects[`${replacement.implementation}:${replacement.project}`];
    if (!project) {
      fail(
        `unknown replacement project ${replacement.implementation}:${replacement.project}`,
      );
    }
    const target = path.resolve(project, replacement.target);
    if (!target.startsWith(`${project}${path.sep}`)) {
      fail(`replacement target escapes project: ${replacement.target}`);
    }
    const content = fs.readFileSync(target, "utf8");
    const first = content.indexOf(replacement.from);
    if (first < 0 || content.indexOf(replacement.from, first + 1) >= 0) {
      fail(
        `replacement must match exactly once: ${replacement.target} ${replacement.from}`,
      );
    }
    fs.writeFileSync(
      target,
      content.replace(replacement.from, replacement.to),
      "utf8",
    );
  }
  return manifest;
}

function goModuleVersion(goMod, modulePath) {
  const escaped = modulePath.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = goMod.match(new RegExp(`^\\s*${escaped}\\s+(v[^\\s]+)`, "m"));
  if (!match) fail(`pinned go-zero go.mod lacks ${modulePath}`);
  return match[1];
}

function main() {
  verifyRepository();
  const { baseline, contract } = loadDocuments();
  const toolchains = verifyToolchains(baseline.toolchains);
  const actualRozeRevision = revision(root);
  if (actualRozeRevision !== baseline.roze.revision) {
    fail(
      `Roze revision drift: expected ${baseline.roze.revision}, got ${actualRozeRevision}`,
    );
  }
  requireClean(root, "Roze");

  const goZeroSource = process.env.ROZE_COMPETITIVE_GO_ZERO_SOURCE;
  if (!goZeroSource) fail("ROZE_COMPETITIVE_GO_ZERO_SOURCE is required");
  const goZeroRevision = revision(goZeroSource);
  if (goZeroRevision !== baseline.goZero.revision) {
    fail(
      `go-zero revision drift: expected ${baseline.goZero.revision}, got ${goZeroRevision}`,
    );
  }
  requireClean(goZeroSource, "go-zero");

  const output = path.resolve(
    process.env.ROZE_COMPETITIVE_BUILD_DIR ||
      path.join(root, "target", `competitive-source-${Date.now()}`),
  );
  freshDirectory(output);
  const workspace = path.join(output, "roze-workspace");
  prepareRozeWorkspace(workspace, [
    "competitive-roze-rest",
    "competitive-roze-rpc",
  ]);

  const bin = path.join(output, "bin");
  fs.mkdirSync(bin);
  fs.symlinkSync(
    path.resolve(goZeroSource),
    path.join(output, "go-zero-source"),
    process.platform === "win32" ? "junction" : "dir",
  );
  const goctl = path.join(bin, executable("goctl"));
  run("go", ["build", "-trimpath", "-o", goctl, "."], {
    cwd: path.join(goZeroSource, "tools", "goctl"),
    env: { ...process.env, GOTELEMETRY: "off" },
  });

  const contractDir = path.join(root, "benchmarks", "competitive", "contracts");
  const values = {
    restApi: path.join(contractDir, contract.inputs.restApi),
    rpcProto: path.join(contractDir, contract.inputs.rpcProto),
    restOut: path.join(workspace, "apps", "competitive-roze-rest"),
    rpcOut: path.join(workspace, "apps", "competitive-roze-rpc"),
  };
  runContractCommand(contract.generation.roze.rest, values);
  runContractCommand(contract.generation.roze.rpc, values);
  runContractCommand(contract.generation.roze.linkRestRpc, values);
  runContractCommand(contract.generation.roze.verifyRestRpcLink, values);

  const goZeroRest = path.join(output, "go-zero-rest");
  const goZeroRpc = path.join(output, "go-zero-rpc");
  runContractCommand(
    contract.generation["go-zero"].rest,
    { ...values, restOut: goZeroRest },
    { goctl },
  );
  runContractCommand(
    contract.generation["go-zero"].rpc,
    {
      ...values,
      rpcProto: contract.inputs.rpcProto,
      rpcOut: goZeroRpc,
    },
    { goctl },
    { cwd: contractDir },
  );
  const overlayManifest = installOverlays({
    "roze:rest": values.restOut,
    "roze:rpc": values.rpcOut,
    "go-zero:rest": goZeroRest,
    "go-zero:rpc": goZeroRpc,
  });

  const cargoEnvironment = {
    ...process.env,
    CARGO_TARGET_DIR:
      process.env.ROZE_COMPETITIVE_CARGO_TARGET_DIR ||
      path.join(root, "target", "competitive-source-cargo"),
  };
  run(
    "cargo",
    [
      "metadata",
      "--format-version",
      "1",
      "--manifest-path",
      path.join(workspace, "Cargo.toml"),
    ],
    { env: cargoEnvironment, capture: true },
  );
  for (const project of [values.restOut, values.rpcOut]) {
    run(
      "cargo",
      ["check", "--locked", "--manifest-path", path.join(project, "Cargo.toml")],
      { env: cargoEnvironment },
    );
  }
  const goZeroMod = fs.readFileSync(path.join(goZeroSource, "go.mod"), "utf8");
  const pinnedGoModules = {
    "google.golang.org/grpc": goModuleVersion(
      goZeroMod,
      "google.golang.org/grpc",
    ),
    "google.golang.org/protobuf": goModuleVersion(
      goZeroMod,
      "google.golang.org/protobuf",
    ),
  };
  for (const project of [goZeroRpc, goZeroRest]) {
    const goEnvironment = { ...process.env, GOTELEMETRY: "off" };
    run(
      "go",
      [
        "mod",
        "edit",
        "-replace=github.com/zeromicro/go-zero=../go-zero-source",
      ],
      { cwd: project, env: goEnvironment },
    );
    if (project === goZeroRest) {
      run("go", ["mod", "edit", "-require=go-zero-rpc@v0.0.0"], {
        cwd: project,
        env: goEnvironment,
      });
      run("go", ["mod", "edit", "-replace=go-zero-rpc=../go-zero-rpc"], {
        cwd: project,
        env: goEnvironment,
      });
    }
    if (project === goZeroRpc) {
      for (const [modulePath, version] of Object.entries(pinnedGoModules)) {
        run("go", ["mod", "edit", `-require=${modulePath}@${version}`], {
          cwd: project,
          env: goEnvironment,
        });
      }
    }
    run("go", ["mod", "tidy"], { cwd: project, env: goEnvironment });
    run("go", ["test", "./..."], { cwd: project, env: goEnvironment });
  }

  const manifest = artifactManifest(output, {
    schemaVersion: 1,
    evidenceEligible: false,
    semanticsReady: false,
    reason:
      "three application scenarios compile, but DB/cache, MQ persistence, context round-trip, fault and correctness probes are not installed",
    implementedScenarioOverlays: [
      ...new Set(overlayManifest.mappings.map((mapping) => mapping.scenario)),
    ].sort(),
    workloadDigest: competitiveInputDigest(),
    revisions: {
      roze: actualRozeRevision,
      goZero: goZeroRevision,
    },
    toolchains,
    host: { platform: os.platform(), arch: os.arch() },
  });
  fs.writeFileSync(
    path.join(output, "artifact-manifest.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
    "utf8",
  );
  console.log(`competitive structural source build passed: ${output}`);
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`competitive structural source build failed: ${error.message}`);
    process.exit(1);
  }
}

module.exports = { workspaceManifest };
