#!/usr/bin/env bash
# Regenerate the Dart protocol models from the Rust wire types (zero-drift codegen).
#   1. emit JSON Schema from the Rust `wire` types (single source of truth)
#   2. generate Dart models from the schema via quicktype
#
# CI guard: run this, then `git diff --exit-code protocol.schema.json app/lib/generated`.
# If a Rust wire type changed without regenerating, CI goes red.
set -euo pipefail
cd "$(dirname "$0")"

echo "1/2  emitting protocol.schema.json from Rust wire types..."
( cd server && cargo run --quiet --bin gen_schema ) > protocol.schema.json

echo "2/2  generating Dart models via quicktype..."
mkdir -p app/lib/generated
npx -y quicktype \
  --src protocol.schema.json --src-lang schema \
  --lang dart --top-level GraphState \
  --out app/lib/generated/protocol.dart

echo "done. generated app/lib/generated/protocol.dart"
