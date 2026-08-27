#!/usr/bin/env bash
# Build and run the direct C ABI result-access benchmark.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo build -p scah-ffi --release

TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | python3 -c "import sys,json; print(json.load(sys.stdin)['target_directory'])")"
LIB_DIR="$TARGET_DIR/release"
INCLUDE="$ROOT/crates/scah-ffi/include"
SRC="$ROOT/crates/scah-ffi/benches/c_abi_bench.c"
OUT_DIR="${TMPDIR:-/tmp}/scah-c-abi-bench"
mkdir -p "$OUT_DIR"

export LD_LIBRARY_PATH="$LIB_DIR:${LD_LIBRARY_PATH:-}"

cc -std=c11 -O2 -Wall -Wextra -Wpedantic -Werror \
  -I"$INCLUDE" "$SRC" \
  -L"$LIB_DIR" -lscah_ffi -lpthread -ldl -lm \
  -o "$OUT_DIR/c_abi_bench"

"$OUT_DIR/c_abi_bench" "$@"
