#!/usr/bin/env bash
set -euo pipefail

mode="${1:-local}"
evidence_dir="${ROZE_MODEL_PARITY_EVIDENCE_DIR:-target/model-parity-evidence}"

write_evidence() {
  local backend="$1"
  local revision rust_version generated_at runner_os
  revision="${GITHUB_SHA:-$(git rev-parse HEAD)}"
  rust_version="$(rustc --version)"
  generated_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  runner_os="${RUNNER_OS:-local}"
  mkdir -p "$evidence_dir"
  printf '{"schema_version":1,"backend":"%s","status":"passed","revision":"%s","rust":"%s","runner_os":"%s","generated_at":"%s"}\n' \
    "$backend" "$revision" "$rust_version" "$runner_os" "$generated_at" \
    > "$evidence_dir/$backend.json"
}

run_local_gate() {
  cargo fmt --all -- --check
  cargo test -p roze-orm
  cargo test -p roze-migration -- --skip postgres_live --skip mysql_live
  cargo test -p rozectl generator::model::tests -- --skip generated_sea_orm_model_crate_compiles --skip generated_toasty_model_crate_compiles
  cargo test -p rozectl generated_sea_orm_model_crate_compiles -- --ignored --nocapture
  cargo test -p rozectl generated_sea_orm_sqlite_runtime_evidence -- --ignored --nocapture
  cargo test -p rozectl generated_toasty_model_crate_compiles -- --ignored --nocapture
  cargo test -p rozectl generated_toasty_runtime_fixture_compiles -- --ignored --nocapture
  cargo clippy -p roze-orm -p roze-migration -p rozectl --all-targets -- -D warnings
}

if grep -Eq '^\| .*\| missing \|' docs/model-ent-parity.md; then
  echo "model parity matrix still contains missing capabilities" >&2
  exit 1
fi

case "$mode" in
  local)
    run_local_gate
    write_evidence sqlite
    ;;
  postgres)
    : "${ROZECTL_TEST_POSTGRES_URL:?ROZECTL_TEST_POSTGRES_URL is required}"
    cargo test -p roze-migration postgres_live_apply_and_rollback_evidence -- --nocapture
    cargo test -p rozectl postgres -- --nocapture
    write_evidence postgres
    ;;
  mysql)
    : "${ROZECTL_TEST_MYSQL_URL:?ROZECTL_TEST_MYSQL_URL is required}"
    cargo test -p roze-migration mysql_live_apply_and_rollback_evidence -- --nocapture
    cargo test -p rozectl mysql -- --nocapture
    write_evidence mysql
    ;;
  all)
    : "${ROZECTL_TEST_POSTGRES_URL:?ROZECTL_TEST_POSTGRES_URL is required}"
    : "${ROZECTL_TEST_MYSQL_URL:?ROZECTL_TEST_MYSQL_URL is required}"
    run_local_gate
    cargo test -p roze-migration postgres_live_apply_and_rollback_evidence -- --nocapture
    cargo test -p roze-migration mysql_live_apply_and_rollback_evidence -- --nocapture
    cargo test -p rozectl postgres -- --nocapture
    cargo test -p rozectl mysql -- --nocapture
    write_evidence sqlite
    write_evidence postgres
    write_evidence mysql
    ;;
  *)
    echo "usage: $0 [local|postgres|mysql|all]" >&2
    exit 2
    ;;
esac
