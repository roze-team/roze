#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

found=0
while IFS= read -r -d '' manifest; do
  found=$((found + 1))
  project="$(dirname "$manifest")"
  cargo run --quiet -p rozectl -- service sync --project "$project" --check
done < <(find . \
  \( -type d \( -name .git -o -name target -o -name 'target-*' \) -prune \) -o \
  \( -type f -name roze-service.yaml -print0 \))

printf 'service dependency check passed: %s manifest(s)\n' "$found"
