#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
node scripts/text-utf8-verify.js
node scripts/text-utf8-verifier.test.js
node scripts/competitive-baseline-verify.js
node scripts/competitive-input-verify.js
node scripts/competitive-input-verifier.test.js
node --check scripts/competitive-source-build.js
node --check scripts/competitive-echo-smoke.js
node --check scripts/competitive-runner-preflight.js
node --check scripts/competitive-schedule-verify.js
node --check scripts/competitive-schedule-bind.js
node --check scripts/competitive-report-verify.js
node --check scripts/reference-systems-evidence-verify.js
node scripts/competitive-verifier.test.js
node scripts/competitive-schedule-verifier.test.js
node scripts/competitive-schedule-binding.test.js
node scripts/reference-systems-evidence-verifier.test.js

bash -n scripts/production-evidence.sh
bash -n scripts/production-evidence-smoke.sh
bash -n scripts/production-evidence-promotion-smoke.sh
bash -n scripts/production-evidence-report-verify.sh
bash -n scripts/production-soak-mq.sh
bash -n scripts/production-soak-gateway.sh
bash -n scripts/production-soak-config-center.sh
bash -n scripts/production-soak-lifecycle.sh
bash -n scripts/production-soak-generated-systems.sh
bash -n scripts/production-soak-ci.sh
bash -n scripts/production-soak-preflight.sh
bash -n scripts/production-evidence-promote.sh
bash -n scripts/production-evidence-gate.sh
bash -n scripts/production-release-audit.sh
bash -n scripts/generated-target-matrix.sh
bash -n scripts/generated-reference-systems.sh
bash -n scripts/reference-systems-integration.sh
bash -n scripts/reference-systems-preflight.sh
bash -n scripts/service-dependency-check.sh
bash -n scripts/competitive-benchmark.sh
bash -n benchmarks/competitive/runners/implementation.sh
bash -n benchmarks/competitive/runners/roze.sh
bash -n benchmarks/competitive/runners/go-zero.sh
bash -n benchmarks/competitive/runners/pair.sh
bash scripts/production-evidence-smoke.sh
bash scripts/production-evidence-promotion-smoke.sh
bash scripts/production-evidence-gate.sh
bash scripts/production-release-audit.sh --json-out target/production-release-audit.json
bash scripts/service-dependency-check.sh

cargo test -p roze-gateway
bash scripts/gateway-smoke.sh
cargo test -p roze-config config_center
cargo test -p roze-mq
cargo check -p roze-kafka --no-default-features --features rdkafka
cargo check -p roze-kafka --no-default-features --features rskafka
cargo test -p roze-service -p roze-bootstrap -p roze-shutdown
cargo test -p roze-job
cargo test -p roze-report

bash scripts/rozectl-smoke.sh

cargo run -p rozectl -- gate check \
  --manifest roze-gate.yaml \
  --report target/roze-gate.json \
  --markdown target/roze-gate.md

bash scripts/generated-target-matrix.sh

bash scripts/production-smoke.sh --skip-generated

echo "release gate passed"
