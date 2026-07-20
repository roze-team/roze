"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const {
  baseline,
  workloads,
  primaryField,
  verifySampleDocument,
} = require("./competitive-sample-verify.js");
const { competitiveInputDigest } = require("./competitive-input-verify.js");
const { verifyPair } = require("./competitive-report-verify.js");

function rawDocument(implementation, multiplier = 1) {
  const revision =
    implementation === "roze" ? baseline.roze.revision : baseline.goZero.revision;
  const dependencyDigests = Object.fromEntries(
    baseline.dependencies.map((dependency, index) => [
      dependency.name,
      `sha256:${String(index + 1).padStart(64, "0")}`,
    ]),
  );
  const runStartedAt = Date.parse("2026-07-18T00:00:00.000Z");
  const sampleSeconds = baseline.measurement.sampleSeconds;
  const scenarioBlockSeconds =
    2 * workloads.global.warmupSeconds +
    2 * baseline.measurement.samples * sampleSeconds;
  const scenarios = workloads.scenarios.map((workload, scenarioIndex) => {
    const primary = primaryField(workload.primaryMetric);
    const lower =
      workload.primaryMetric === "p99_latency_ms" ||
      workload.primaryMetric === "recovery_time_ms";
    const primaryValue = lower ? 10 * multiplier : 200 / multiplier;
    const samples = Array.from({ length: baseline.measurement.samples }, (_, sampleIndex) => {
      const pairStartSeconds =
        scenarioIndex * scenarioBlockSeconds +
        2 * workloads.global.warmupSeconds +
        sampleIndex * 2 * sampleSeconds;
      const rozeRunsFirst = (sampleIndex + scenarioIndex) % 2 === 0;
      const implementationRunsFirst =
        (implementation === "roze" && rozeRunsFirst) ||
        (implementation === "go-zero" && !rozeRunsFirst);
      const sampleStart =
        runStartedAt +
        (pairStartSeconds + (implementationRunsFirst ? 0 : sampleSeconds)) * 1000;
      const startedAt = new Date(sampleStart).toISOString();
      const sample = {
        startedAt,
        finishedAt: new Date(sampleStart + sampleSeconds * 1000).toISOString(),
        durationSeconds: sampleSeconds,
        requestCount: 40000,
        errorCount: 0,
        cpuCoreSeconds: 100,
        p50LatencyMs: 3 * multiplier,
        p95LatencyMs: 7 * multiplier,
        p99LatencyMs: 10 * multiplier,
        memoryPeakBytes: 100000000,
        duplicateEffects: 0,
        recoveryTimeMs: 100 * multiplier,
        [primary]: primaryValue,
      };
      if (workload.primaryMetric === "throughput_per_core") {
        sample.cpuCoreSeconds = sample.requestCount / primaryValue;
      }
      if (workload.primaryMetric === "confirmed_throughput") {
        sample.confirmedCount = Math.round(primaryValue * sample.durationSeconds);
        sample[primary] = sample.confirmedCount / sample.durationSeconds;
      }
      return sample;
    });
    return {
      id: workload.id,
      samples,
    };
  });
  return {
    schemaVersion: 1,
    implementation,
    revision,
    sourceDirty: false,
    artifactDigest: `sha256:${"d".repeat(64)}`,
    toolchains: { ...baseline.toolchains },
    startedAt: "2026-07-18T00:00:00.000Z",
    finishedAt: "2026-07-18T02:00:00.000Z",
    runId: "competitive-test-run",
    environmentFingerprint: "a".repeat(64),
    workloadDigest: competitiveInputDigest(),
    runner: {
      os: "linux",
      arch: "x86_64",
      exclusive: true,
      cpuGovernor: "performance",
      cpuModel: "fixed-test-cpu",
      kernel: "test-kernel",
      logicalCpus: 8,
      memoryBytes: baseline.runner.memoryLimitBytes,
    },
    dependencyDigests,
    scenarios,
  };
}

function writePair(roze, goZero) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "roze-competitive-"));
  const rozeFile = path.join(directory, "roze.json");
  const goZeroFile = path.join(directory, "go-zero.json");
  fs.writeFileSync(rozeFile, JSON.stringify(roze));
  fs.writeFileSync(goZeroFile, JSON.stringify(goZero));
  return { rozeFile, goZeroFile };
}

test("accepts complete stable raw samples", () => {
  assert.equal(verifySampleDocument(rawDocument("roze"), "roze").implementation, "roze");
});

test("rejects unstable primary samples", () => {
  const document = rawDocument("roze");
  const workload = workloads.scenarios[0];
  const primary = primaryField(workload.primaryMetric);
  document.scenarios[0].samples[0][primary] *= 10;
  document.scenarios[0].samples[0].cpuCoreSeconds =
    document.scenarios[0].samples[0].requestCount /
    document.scenarios[0].samples[0][primary];
  assert.throws(
    () => verifySampleDocument(document, "roze"),
    /primary metric CV/,
  );
});

test("rejects producer-forged derived throughput", () => {
  const document = rawDocument("roze");
  document.scenarios[0].samples[0].throughputPerCore *= 2;
  assert.throws(
    () => verifySampleDocument(document, "roze"),
    /must be derived from counts and cpuCoreSeconds/,
  );
});

test("rejects overlapping exclusive samples", () => {
  const document = rawDocument("roze");
  document.scenarios[0].samples[1].startedAt =
    document.scenarios[0].samples[0].startedAt;
  document.scenarios[0].samples[1].finishedAt =
    document.scenarios[0].samples[0].finishedAt;
  assert.throws(
    () => verifySampleDocument(document, "roze"),
    /competitive samples must run exclusively/,
  );
});

test("emits surpassed only for matched evidence with weighted wins", () => {
  const files = writePair(rawDocument("roze"), rawDocument("go-zero", 2));
  const report = verifyPair(files.rozeFile, files.goZeroFile);
  assert.equal(report.weightedWins, 100);
  assert.equal(report.scenariosAtParity, 6);
  assert.ok(report.weightedGeometricAdvantage >= 1.1);
  assert.equal(report.verdict, "surpassed");
  assert.deepEqual(report.regressions, []);
});

test("rejects a nominal sweep without the required geometric advantage", () => {
  const files = writePair(rawDocument("roze"), rawDocument("go-zero", 1.05));
  const report = verifyPair(files.rozeFile, files.goZeroFile);
  assert.equal(report.weightedWins, 100);
  assert.ok(report.weightedGeometricAdvantage < 1.1);
  assert.equal(report.verdict, "not-surpassed");
});

test("rejects inconsistent latency percentiles", () => {
  const document = rawDocument("roze");
  document.scenarios[0].samples[0].p95LatencyMs =
    document.scenarios[0].samples[0].p99LatencyMs + 1;
  assert.throws(
    () => verifySampleDocument(document, "roze"),
    /p50 <= p95 <= p99/,
  );
});

test("rejects environment drift between implementations", () => {
  const roze = rawDocument("roze");
  const goZero = rawDocument("go-zero", 2);
  goZero.environmentFingerprint = "c".repeat(64);
  const files = writePair(roze, goZero);
  assert.throws(
    () => verifyPair(files.rozeFile, files.goZeroFile),
    /environmentFingerprint mismatch/,
  );
});

test("rejects samples with CPUs outside the pinned affinity", () => {
  const document = rawDocument("roze");
  document.runner.logicalCpus = 16;
  assert.throws(
    () => verifySampleDocument(document, "roze"),
    /pinned cpu affinity/,
  );
});

test("rejects samples with a different memory limit", () => {
  const document = rawDocument("roze");
  document.runner.memoryBytes = baseline.runner.memoryLimitBytes * 2;
  assert.throws(
    () => verifySampleDocument(document, "roze"),
    /pinned memory limit/,
  );
});

test("rejects overlapping paired implementation samples", () => {
  const roze = rawDocument("roze");
  const goZero = rawDocument("go-zero", 2);
  goZero.scenarios[0].samples[0].startedAt = roze.scenarios[0].samples[0].startedAt;
  goZero.scenarios[0].samples[0].finishedAt = roze.scenarios[0].samples[0].finishedAt;
  const files = writePair(roze, goZero);
  assert.throws(
    () => verifyPair(files.rozeFile, files.goZeroFile),
    /paired samples overlap/,
  );
});

test("reports a material memory regression as not surpassed", () => {
  const roze = rawDocument("roze");
  for (const sample of roze.scenarios[0].samples) {
    sample.memoryPeakBytes = 120000000;
  }
  const files = writePair(roze, rawDocument("go-zero", 2));
  const report = verifyPair(files.rozeFile, files.goZeroFile);
  assert.equal(report.verdict, "not-surpassed");
  assert.ok(report.regressions.includes("rest-json:memory_peak_bytes"));
});
