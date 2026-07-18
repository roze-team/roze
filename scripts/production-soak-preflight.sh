#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fail() {
  echo "production soak preflight: $*" >&2
  exit 1
}

[[ "$(uname -s)" == "Linux" ]] || fail "the fixed runner must be Linux"

for command_name in cargo rustc node docker; do
  command -v "$command_name" >/dev/null 2>&1 ||
    fail "required command is missing: $command_name"
done

docker info >/dev/null 2>&1 || fail "Docker daemon is unavailable"
docker compose version >/dev/null 2>&1 || fail "Docker Compose v2 is unavailable"

[[ -r /proc/stat && -r /proc/meminfo ]] ||
  fail "Linux procfs resource counters are unavailable"

REVISION="$(git rev-parse HEAD 2>/dev/null)" ||
  fail "runner is not executing inside a Git worktree"
[[ "$REVISION" =~ ^[0-9a-f]{40}$ ]] ||
  fail "runner revision is not a full commit SHA"

echo "production soak preflight passed: revision=$REVISION"
