"use strict";

const fs = require("node:fs");
const path = require("node:path");
const {
  workloads,
  coefficientOfVariation,
  primaryField,
  readAndVerify,
} = require("./competitive-sample-verify.js");

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[middle - 1] + sorted[middle]) / 2
    : sorted[middle];
}

function sameJson(left, right) {
  return JSON.stringify(left) === JSON.stringify(right);
}

function lowerIsBetter(metric) {
  return metric === "p99_latency_ms" || metric === "recovery_time_ms";
}

function sampleStatistics(values) {
  const center = median(values);
  const deviations = values.map((value) => Math.abs(value - center));
  const mean = values.reduce((sum, value) => sum + value, 0) / values.length;
  const sampleVariance =
    values.length > 1
      ? values.reduce((sum, value) => sum + (value - mean) ** 2, 0) /
        (values.length - 1)
      : 0;
  const margin95 = 1.96 * Math.sqrt(sampleVariance / values.length);
  return {
    median: center,
    mad: median(deviations),
    coefficientOfVariation: coefficientOfVariation(values),
    confidenceInterval95: [mean - margin95, mean + margin95],
  };
}

function sampleInterval(sample, label) {
  return {
    label,
    startedAt: Date.parse(sample.startedAt),
    finishedAt: Date.parse(sample.finishedAt),
  };
}

function verifyCounterbalancedSchedule(roze, goZero) {
  const allIntervals = [];
  let rozeFirst = 0;
  let goZeroFirst = 0;
  for (const workload of workloads.scenarios) {
    const rozeScenario = roze.scenarios.find((scenario) => scenario.id === workload.id);
    const goZeroScenario = goZero.scenarios.find((scenario) => scenario.id === workload.id);
    if (rozeScenario.samples.length !== goZeroScenario.samples.length) {
      throw new Error(`${workload.id} paired samples have different counts`);
    }
    for (let index = 0; index < rozeScenario.samples.length; index += 1) {
      const rozeInterval = sampleInterval(
        rozeScenario.samples[index],
        `${workload.id}.roze[${index}]`,
      );
      const goZeroInterval = sampleInterval(
        goZeroScenario.samples[index],
        `${workload.id}.go-zero[${index}]`,
      );
      const first =
        rozeInterval.startedAt < goZeroInterval.startedAt ? rozeInterval : goZeroInterval;
      const second = first === rozeInterval ? goZeroInterval : rozeInterval;
      if (second.startedAt < first.finishedAt) {
        throw new Error(`${workload.id} paired samples overlap on the exclusive runner`);
      }
      const pairGapSeconds = (second.startedAt - first.finishedAt) / 1000;
      if (pairGapSeconds > 5) {
        throw new Error(`${workload.id} paired samples are not adjacent`);
      }
      if (first === rozeInterval) rozeFirst += 1;
      else goZeroFirst += 1;
      allIntervals.push(rozeInterval, goZeroInterval);
    }
  }
  allIntervals.sort((left, right) => left.startedAt - right.startedAt);
  for (let index = 1; index < allIntervals.length; index += 1) {
    if (allIntervals[index].startedAt < allIntervals[index - 1].finishedAt) {
      throw new Error(
        `${allIntervals[index].label} overlaps ${allIntervals[index - 1].label}`,
      );
    }
  }
  if (Math.abs(rozeFirst - goZeroFirst) > 1) {
    throw new Error("implementation execution order must be counterbalanced");
  }
}

function scenarioSummary(workload, rozeScenario, goZeroScenario) {
  const primary = primaryField(workload.primaryMetric);
  const rozeStatistics = sampleStatistics(
    rozeScenario.samples.map((sample) => sample[primary]),
  );
  const goZeroStatistics = sampleStatistics(
    goZeroScenario.samples.map((sample) => sample[primary]),
  );
  const rozePrimary = rozeStatistics.median;
  const goZeroPrimary = goZeroStatistics.median;
  const rozeP99 = median(rozeScenario.samples.map((sample) => sample.p99LatencyMs));
  const goZeroP99 = median(goZeroScenario.samples.map((sample) => sample.p99LatencyMs));
  const rozeErrors = median(
    rozeScenario.samples.map((sample) => sample.errorCount / sample.requestCount),
  );
  const goZeroErrors = median(
    goZeroScenario.samples.map((sample) => sample.errorCount / sample.requestCount),
  );
  const rozeMemory = median(rozeScenario.samples.map((sample) => sample.memoryPeakBytes));
  const goZeroMemory = median(goZeroScenario.samples.map((sample) => sample.memoryPeakBytes));
  const advantageRatio = lowerIsBetter(workload.primaryMetric)
    ? goZeroPrimary / rozePrimary
    : rozePrimary / goZeroPrimary;
  const won = advantageRatio >= 1;
  const regressions = [];
  if (rozeP99 > goZeroP99 * 1.1) regressions.push("p99_latency_ms");
  if (rozeErrors > goZeroErrors * 1.1 && rozeErrors > goZeroErrors) {
    regressions.push("error_ratio");
  }
  if (rozeMemory > goZeroMemory * 1.1) regressions.push("memory_peak_bytes");
  if (workload.recoveryObjectiveMs !== undefined) {
    const rozeRecovery = median(
      rozeScenario.samples.map((sample) => sample.recoveryTimeMs),
    );
    const goZeroRecovery = median(
      goZeroScenario.samples.map((sample) => sample.recoveryTimeMs),
    );
    if (rozeRecovery > goZeroRecovery * 1.1) regressions.push("recovery_time_ms");
  }
  return {
    id: workload.id,
    weight: workload.weight,
    primaryMetric: workload.primaryMetric,
    rozePrimary,
    goZeroPrimary,
    advantageRatio,
    rozeStatistics,
    goZeroStatistics,
    won,
    regressions,
  };
}

function verifyPair(rozeFile, goZeroFile) {
  const roze = readAndVerify(rozeFile, "roze");
  const goZero = readAndVerify(goZeroFile, "go-zero");
  if (roze.runId !== goZero.runId) throw new Error("runId mismatch");
  if (roze.environmentFingerprint !== goZero.environmentFingerprint) {
    throw new Error("environmentFingerprint mismatch");
  }
  if (roze.workloadDigest !== goZero.workloadDigest) {
    throw new Error("workloadDigest mismatch");
  }
  if (!sameJson(roze.runner, goZero.runner)) throw new Error("runner metadata mismatch");
  if (!sameJson(roze.dependencyDigests, goZero.dependencyDigests)) {
    throw new Error("dependency digest mismatch");
  }
  verifyCounterbalancedSchedule(roze, goZero);
  const rozeById = new Map(roze.scenarios.map((scenario) => [scenario.id, scenario]));
  const goZeroById = new Map(goZero.scenarios.map((scenario) => [scenario.id, scenario]));
  const scenarios = workloads.scenarios.map((workload) =>
    scenarioSummary(workload, rozeById.get(workload.id), goZeroById.get(workload.id)),
  );
  const weightedWins = scenarios
    .filter((scenario) => scenario.won)
    .reduce((sum, scenario) => sum + scenario.weight, 0);
  const weightedGeometricAdvantage = Math.exp(
    scenarios.reduce(
      (sum, scenario) =>
        sum + (scenario.weight / 100) * Math.log(scenario.advantageRatio),
      0,
    ),
  );
  const scenariosAtParity = scenarios.filter(
    (scenario) => scenario.advantageRatio >= 1,
  ).length;
  const regressions = scenarios.flatMap((scenario) =>
    scenario.regressions.map((metric) => `${scenario.id}:${metric}`),
  );
  return {
    schemaVersion: 1,
    runId: roze.runId,
    rozeRevision: roze.revision,
    goZeroRevision: goZero.revision,
    environmentFingerprint: roze.environmentFingerprint,
    weightedWins,
    weightedGeometricAdvantage,
    requiredWeightedGeometricAdvantage: 1.1,
    scenariosAtParity,
    requiredScenariosAtParity: 4,
    regressions,
    scenarios,
    verdict:
      weightedGeometricAdvantage >= 1.1 &&
      scenariosAtParity >= 4 &&
      regressions.length === 0
        ? "surpassed"
        : "not-surpassed",
  };
}

if (require.main === module) {
  const [rozeFile, goZeroFile, outputFile] = process.argv.slice(2);
  if (!rozeFile || !goZeroFile) {
    console.error(
      "usage: node competitive-report-verify.js <roze.json> <go-zero.json> [report.json]",
    );
    process.exit(2);
  }
  try {
    const report = verifyPair(path.resolve(rozeFile), path.resolve(goZeroFile));
    const encoded = `${JSON.stringify(report, null, 2)}\n`;
    if (outputFile) fs.writeFileSync(path.resolve(outputFile), encoded);
    process.stdout.write(encoded);
    if (report.verdict !== "surpassed") process.exit(1);
  } catch (error) {
    console.error(`competitive report invalid: ${error.message}`);
    process.exit(1);
  }
}

module.exports = { verifyPair };
