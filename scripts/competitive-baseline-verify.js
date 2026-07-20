const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const baselinePath = path.join(root, "benchmarks", "competitive", "baseline.yaml");
const workloadPath = path.join(root, "benchmarks", "competitive", "workloads.json");

function fail(message) {
  console.error(`competitive baseline invalid: ${message}`);
  process.exit(1);
}

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch (error) {
    fail(`${path.relative(root, file)}: ${error.message}`);
  }
}

function requiredObject(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    fail(`${label} must be an object`);
  }
}

function requiredString(value, label) {
  if (typeof value !== "string" || value.trim() === "") {
    fail(`${label} must be a non-empty string`);
  }
}

const baseline = readJson(baselinePath);
const workloads = readJson(workloadPath);
requiredObject(baseline, "baseline");
requiredObject(workloads, "workloads");

if (baseline.schemaVersion !== 1 || workloads.schemaVersion !== 1) {
  fail("unsupported schemaVersion");
}

for (const [name, source] of [["roze", baseline.roze], ["goZero", baseline.goZero]]) {
  requiredObject(source, name);
  requiredString(source.repository, `${name}.repository`);
  if (!/^[0-9a-f]{40}$/.test(source.revision || "")) {
    fail(`${name}.revision must be a full lowercase Git revision`);
  }
}

requiredObject(baseline.toolchains, "toolchains");
for (const name of ["rust", "cargo", "go", "node", "protoc"]) {
  requiredString(baseline.toolchains[name], `toolchains.${name}`);
}

requiredObject(baseline.runner, "runner");
if (baseline.runner.os !== "linux" || baseline.runner.arch !== "x86_64") {
  fail("the authoritative runner must be linux/x86_64");
}
if (baseline.runner.exclusive !== true || baseline.runner.cpuGovernor !== "performance") {
  fail("the authoritative runner must be exclusive with the performance CPU governor");
}
if (!Number.isInteger(baseline.runner.memoryLimitBytes) || baseline.runner.memoryLimitBytes < 1073741824) {
  fail("runner.memoryLimitBytes must be an integer of at least 1 GiB");
}

requiredObject(baseline.measurement, "measurement");
if (!Number.isInteger(baseline.measurement.samples) || baseline.measurement.samples < 5) {
  fail("measurement.samples must be at least 5");
}
if (!Number.isInteger(baseline.measurement.warmups) || baseline.measurement.warmups < 1) {
  fail("measurement.warmups must be at least 1");
}
if (
  !Number.isInteger(baseline.measurement.sampleSeconds) ||
  baseline.measurement.sampleSeconds < 60
) {
  fail("measurement.sampleSeconds must be at least 60");
}
if (baseline.measurement.maxCoefficientOfVariation > 0.1) {
  fail("measurement.maxCoefficientOfVariation must not exceed 0.1");
}

if (!Array.isArray(baseline.dependencies) || baseline.dependencies.length === 0) {
  fail("dependencies must be non-empty");
}
const requireDigests = process.env.ROZE_COMPETITIVE_REQUIRE_DIGESTS === "1";
const dependencyNames = new Set();
const digestEnvironments = new Set();
for (const dependency of baseline.dependencies) {
  requiredString(dependency.name, "dependency.name");
  requiredString(dependency.image, `${dependency.name}.image`);
  requiredString(dependency.digestEnvironment, `${dependency.name}.digestEnvironment`);
  if (dependencyNames.has(dependency.name)) {
    fail(`duplicate dependency name ${dependency.name}`);
  }
  if (digestEnvironments.has(dependency.digestEnvironment)) {
    fail(`duplicate digest environment ${dependency.digestEnvironment}`);
  }
  dependencyNames.add(dependency.name);
  digestEnvironments.add(dependency.digestEnvironment);
  if (requireDigests) {
    const digest = process.env[dependency.digestEnvironment] || "";
    if (!/^sha256:[0-9a-f]{64}$/.test(digest)) {
      fail(`${dependency.digestEnvironment} must contain a sha256 image digest`);
    }
  }
}

if (!Array.isArray(workloads.requiredImplementations)
    || workloads.requiredImplementations.join(",") !== "roze,go-zero") {
  fail("requiredImplementations must be exactly roze and go-zero");
}
if (!Array.isArray(workloads.scenarios) || workloads.scenarios.length !== 6) {
  fail("exactly six competitive scenarios are required");
}
requiredObject(workloads.global, "workloads.global");
for (const field of ["requestTimeoutMs", "connectTimeoutMs", "warmupSeconds", "payloadSeed"]) {
  if (!Number.isInteger(workloads.global[field]) || workloads.global[field] <= 0) {
    fail(`workloads.global.${field} must be a positive integer`);
  }
}
if (
  !Array.isArray(workloads.global.concurrencySteps) ||
  workloads.global.concurrencySteps.length < 3 ||
  workloads.global.concurrencySteps.some((value) => !Number.isInteger(value) || value <= 0)
) {
  fail("workloads.global.concurrencySteps must contain at least three positive integers");
}
if (!(workloads.global.errorBudgetRatio >= 0 && workloads.global.errorBudgetRatio < 1)) {
  fail("workloads.global.errorBudgetRatio must be in [0, 1)");
}
const ids = new Set();
let weight = 0;
for (const scenario of workloads.scenarios) {
  requiredString(scenario.id, "scenario.id");
  if (ids.has(scenario.id)) {
    fail(`duplicate scenario id ${scenario.id}`);
  }
  ids.add(scenario.id);
  if (!Number.isInteger(scenario.weight) || scenario.weight <= 0) {
    fail(`${scenario.id}.weight must be a positive integer`);
  }
  weight += scenario.weight;
  requiredString(scenario.primaryMetric, `${scenario.id}.primaryMetric`);
  requiredString(scenario.protocol, `${scenario.id}.protocol`);
  if (!(scenario.sloP99Ms > 0)) {
    fail(`${scenario.id}.sloP99Ms must be positive`);
  }
}
if (weight !== 100) {
  fail(`scenario weights must total 100, got ${weight}`);
}

console.log(`competitive baseline valid: ${workloads.scenarios.length} scenarios, weight=${weight}`);
