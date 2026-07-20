"use strict";

const fs = require("node:fs");
const path = require("node:path");
const { competitiveInputDigest } = require("./competitive-input-verify.js");

const root = path.resolve(__dirname, "..");
const baseline = JSON.parse(
  fs.readFileSync(path.join(root, "benchmarks", "competitive", "baseline.yaml"), "utf8"),
);
const workloads = JSON.parse(
  fs.readFileSync(path.join(root, "benchmarks", "competitive", "workloads.json"), "utf8"),
);

function reject(message) {
  throw new Error(message);
}

function finite(value, label, minimum = 0) {
  if (!Number.isFinite(value) || value < minimum) {
    reject(`${label} must be a finite number >= ${minimum}`);
  }
}

function approximatelyEqual(actual, expected) {
  const scale = Math.max(1, Math.abs(actual), Math.abs(expected));
  return Math.abs(actual - expected) <= scale * 1e-6;
}

function coefficientOfVariation(values) {
  const mean = values.reduce((sum, value) => sum + value, 0) / values.length;
  if (mean === 0) {
    return values.every((value) => value === 0) ? 0 : Number.POSITIVE_INFINITY;
  }
  const variance =
    values.reduce((sum, value) => sum + (value - mean) ** 2, 0) / values.length;
  return Math.sqrt(variance) / mean;
}

function primaryField(metric) {
  switch (metric) {
    case "throughput_per_core":
      return "throughputPerCore";
    case "p99_latency_ms":
      return "p99LatencyMs";
    case "confirmed_throughput":
      return "confirmedThroughput";
    case "recovery_time_ms":
      return "recoveryTimeMs";
    default:
      reject(`unsupported primary metric ${metric}`);
  }
}

function expectedRevision(implementation) {
  if (implementation === "roze") return baseline.roze.revision;
  if (implementation === "go-zero") return baseline.goZero.revision;
  reject(`unknown implementation ${implementation}`);
}

function cpuAffinityCount(value) {
  const cpus = new Set();
  for (const token of String(value).split(",")) {
    const [startText, endText] = token.split("-");
    const start = Number(startText);
    const end = endText === undefined ? start : Number(endText);
    if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end < start) {
      reject("baseline runner.cpuAffinity is invalid");
    }
    for (let cpu = start; cpu <= end; cpu += 1) cpus.add(cpu);
  }
  return cpus.size;
}

function verifySampleDocument(document, expectedImplementation) {
  if (!document || typeof document !== "object" || Array.isArray(document)) {
    reject("sample document must be an object");
  }
  if (document.schemaVersion !== 1) reject("schemaVersion must be 1");
  if (document.implementation !== expectedImplementation) {
    reject(`implementation must be ${expectedImplementation}`);
  }
  if (document.revision !== expectedRevision(expectedImplementation)) {
    reject(`${expectedImplementation} revision does not match the pinned baseline`);
  }
  if (document.sourceDirty !== false) {
    reject("sourceDirty must be false");
  }
  if (!/^sha256:[0-9a-f]{64}$/.test(document.artifactDigest || "")) {
    reject("artifactDigest must be a sha256 digest");
  }
  for (const [name, version] of Object.entries(baseline.toolchains)) {
    if (document.toolchains?.[name] !== version) {
      reject(`toolchains.${name} must match the baseline`);
    }
  }
  const startedAt = Date.parse(document.startedAt);
  const finishedAt = Date.parse(document.finishedAt);
  if (!Number.isFinite(startedAt) || !Number.isFinite(finishedAt) || finishedAt <= startedAt) {
    reject("startedAt and finishedAt must define a positive UTC interval");
  }
  if (typeof document.runId !== "string" || document.runId.length < 8) {
    reject("runId must be a non-empty stable identifier");
  }
  if (
    typeof document.environmentFingerprint !== "string" ||
    !/^[0-9a-f]{64}$/.test(document.environmentFingerprint)
  ) {
    reject("environmentFingerprint must be a lowercase SHA-256 hex digest");
  }
  if (
    typeof document.workloadDigest !== "string" ||
    !/^sha256:[0-9a-f]{64}$/.test(document.workloadDigest)
  ) {
    reject("workloadDigest must be a sha256 digest");
  }
  if (document.workloadDigest !== competitiveInputDigest()) {
    reject("workloadDigest does not match the checked-in competitive inputs");
  }
  if (!document.runner || document.runner.os !== "linux" || document.runner.arch !== "x86_64") {
    reject("runner must record linux/x86_64");
  }
  if (document.runner.exclusive !== true || document.runner.cpuGovernor !== "performance") {
    reject("runner must be exclusive with performance CPU governor");
  }
  for (const field of ["cpuModel", "kernel"]) {
    if (typeof document.runner[field] !== "string" || document.runner[field].trim() === "") {
      reject(`runner.${field} must be recorded`);
    }
  }
  for (const field of ["logicalCpus", "memoryBytes"]) {
    finite(document.runner[field], `runner.${field}`, 1);
  }
  const expectedLogicalCpus = cpuAffinityCount(baseline.runner.cpuAffinity);
  if (document.runner.logicalCpus !== expectedLogicalCpus) {
    reject(`runner.logicalCpus must equal the pinned cpu affinity (${expectedLogicalCpus})`);
  }
  if (document.runner.memoryBytes !== baseline.runner.memoryLimitBytes) {
    reject("runner.memoryBytes must equal the pinned memory limit");
  }
  if (!document.dependencyDigests || typeof document.dependencyDigests !== "object") {
    reject("dependencyDigests must be an object");
  }
  for (const dependency of baseline.dependencies) {
    const digest = document.dependencyDigests[dependency.name];
    if (!/^sha256:[0-9a-f]{64}$/.test(digest || "")) {
      reject(`dependencyDigests.${dependency.name} must be a sha256 digest`);
    }
  }
  if (!Array.isArray(document.scenarios)) reject("scenarios must be an array");
  const byId = new Map(document.scenarios.map((scenario) => [scenario.id, scenario]));
  if (byId.size !== workloads.scenarios.length) {
    reject("sample must contain each scenario exactly once");
  }
  const sampleWindows = [];
  let declaredSampleSeconds = 0;
  for (const workload of workloads.scenarios) {
    const scenario = byId.get(workload.id);
    if (!scenario) reject(`missing scenario ${workload.id}`);
    if (!Array.isArray(scenario.samples) || scenario.samples.length < baseline.measurement.samples) {
      reject(`${workload.id} requires at least ${baseline.measurement.samples} samples`);
    }
    const primary = primaryField(workload.primaryMetric);
    for (const [index, sample] of scenario.samples.entries()) {
      const label = `${workload.id}.samples[${index}]`;
      finite(sample.durationSeconds, `${label}.durationSeconds`, baseline.measurement.sampleSeconds);
      finite(sample.requestCount, `${label}.requestCount`, 1);
      finite(sample.errorCount, `${label}.errorCount`);
      finite(sample.cpuCoreSeconds, `${label}.cpuCoreSeconds`, Number.EPSILON);
      if (!Number.isInteger(sample.requestCount) || !Number.isInteger(sample.errorCount)) {
        reject(`${label} request/error counts must be integers`);
      }
      if (sample.errorCount > sample.requestCount) {
        reject(`${label}.errorCount cannot exceed requestCount`);
      }
      finite(sample.p99LatencyMs, `${label}.p99LatencyMs`);
      finite(sample.memoryPeakBytes, `${label}.memoryPeakBytes`, 1);
      finite(sample.p50LatencyMs, `${label}.p50LatencyMs`);
      finite(sample.p95LatencyMs, `${label}.p95LatencyMs`);
      finite(sample[primary], `${label}.${primary}`, Number.EPSILON);
      if (
        sample.p50LatencyMs > sample.p95LatencyMs ||
        sample.p95LatencyMs > sample.p99LatencyMs
      ) {
        reject(`${label} latency percentiles must satisfy p50 <= p95 <= p99`);
      }
      if (
        sample.cpuCoreSeconds >
        sample.durationSeconds * document.runner.logicalCpus * 1.05
      ) {
        reject(`${label}.cpuCoreSeconds exceeds available runner CPU time`);
      }
      const sampleStartedAt = Date.parse(sample.startedAt);
      const sampleFinishedAt = Date.parse(sample.finishedAt);
      if (
        !Number.isFinite(sampleStartedAt) ||
        !Number.isFinite(sampleFinishedAt) ||
        sampleStartedAt < startedAt ||
        sampleFinishedAt > finishedAt ||
        sampleFinishedAt <= sampleStartedAt
      ) {
        reject(`${label} timestamps must define an interval inside the document run`);
      }
      const measuredSeconds = (sampleFinishedAt - sampleStartedAt) / 1000;
      if (
        measuredSeconds < sample.durationSeconds ||
        measuredSeconds > sample.durationSeconds + 5
      ) {
        reject(`${label} timestamp interval must match durationSeconds`);
      }
      sampleWindows.push({ label, startedAt: sampleStartedAt, finishedAt: sampleFinishedAt });
      declaredSampleSeconds += sample.durationSeconds;
      const errorRatio = sample.errorCount / sample.requestCount;
      if (errorRatio > workloads.global.errorBudgetRatio) {
        reject(`${label} exceeds the error budget`);
      }
      if (sample.p99LatencyMs > workload.sloP99Ms) {
        reject(`${label} exceeds the p99 SLO`);
      }
      if (workload.requiresZeroDuplicateEffects && sample.duplicateEffects !== 0) {
        reject(`${label}.duplicateEffects must be zero`);
      }
      if (
        workload.recoveryObjectiveMs !== undefined &&
        sample.recoveryTimeMs > workload.recoveryObjectiveMs
      ) {
        reject(`${label} exceeds the recovery objective`);
      }
      if (workload.primaryMetric === "throughput_per_core") {
        const derived = (sample.requestCount - sample.errorCount) / sample.cpuCoreSeconds;
        if (!approximatelyEqual(sample.throughputPerCore, derived)) {
          reject(`${label}.throughputPerCore must be derived from counts and cpuCoreSeconds`);
        }
      }
      if (workload.primaryMetric === "confirmed_throughput") {
        finite(sample.confirmedCount, `${label}.confirmedCount`, 1);
        if (
          !Number.isInteger(sample.confirmedCount) ||
          sample.confirmedCount > sample.requestCount - sample.errorCount
        ) {
          reject(`${label}.confirmedCount must be an integer within successful requests`);
        }
        const derived = sample.confirmedCount / sample.durationSeconds;
        if (!approximatelyEqual(sample.confirmedThroughput, derived)) {
          reject(`${label}.confirmedThroughput must be derived from confirmedCount and duration`);
        }
      }
    }
    const cv = coefficientOfVariation(scenario.samples.map((sample) => sample[primary]));
    if (cv > baseline.measurement.maxCoefficientOfVariation) {
      reject(
        `${workload.id} primary metric CV ${cv.toFixed(6)} exceeds ` +
          baseline.measurement.maxCoefficientOfVariation,
      );
    }
  }
  sampleWindows.sort((left, right) => left.startedAt - right.startedAt);
  for (let index = 1; index < sampleWindows.length; index += 1) {
    if (sampleWindows[index].startedAt < sampleWindows[index - 1].finishedAt) {
      reject(
        `${sampleWindows[index].label} overlaps ${sampleWindows[index - 1].label}; ` +
          "competitive samples must run exclusively",
      );
    }
  }
  const requiredRunSeconds =
    declaredSampleSeconds + workloads.scenarios.length * workloads.global.warmupSeconds;
  if ((finishedAt - startedAt) / 1000 < requiredRunSeconds) {
    reject("document run interval cannot contain all declared samples and warmups");
  }
  return document;
}

function readAndVerify(file, implementation) {
  const document = JSON.parse(fs.readFileSync(file, "utf8"));
  return verifySampleDocument(document, implementation);
}

if (require.main === module) {
  const [file, implementation] = process.argv.slice(2);
  if (!file || !implementation) {
    console.error("usage: node competitive-sample-verify.js <sample.json> <roze|go-zero>");
    process.exit(2);
  }
  try {
    readAndVerify(path.resolve(file), implementation);
    console.log(`competitive sample valid: ${implementation}`);
  } catch (error) {
    console.error(`competitive sample invalid: ${error.message}`);
    process.exit(1);
  }
}

module.exports = {
  baseline,
  workloads,
  coefficientOfVariation,
  primaryField,
  readAndVerify,
  verifySampleDocument,
};
