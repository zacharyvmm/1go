#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo build -p scah-ffi --release

TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | python3 -c "import sys,json; print(json.load(sys.stdin)['target_directory'])")"
LIB_DIR="$TARGET_DIR/release"
INCLUDE="$ROOT/crates/scah-ffi/include"
TEST_DIR="$ROOT/crates/scah-ffi/tests"
OUT_DIR="$(mktemp -d)"
trap 'rm -rf "$OUT_DIR"' EXIT

if [[ ! -f "$INCLUDE/scah.h" ]]; then
  echo "missing $INCLUDE/scah.h — run: cargo run -p scah-ffi --features header-gen --bin generate_header" >&2
  exit 1
fi

if [[ ! -e "$LIB_DIR/libscah_ffi.so" && ! -e "$LIB_DIR/libscah_ffi.a" ]]; then
  echo "missing libscah_ffi in $LIB_DIR" >&2
  exit 1
fi

# Prefer the shared library when present (self-contained); fall back to static.
export LD_LIBRARY_PATH="$LIB_DIR:${LD_LIBRARY_PATH:-}"

cc -std=c11 -O2 -Wall -Wextra -Wpedantic -Werror \
  -I"$INCLUDE" "$TEST_DIR/c_smoke.c" \
  -L"$LIB_DIR" -lscah_ffi -lpthread -ldl -lm \
  -o "$OUT_DIR/c_smoke"
"$OUT_DIR/c_smoke"

c++ -std=c++17 -O2 -Wall -Wextra -Wpedantic -Werror \
  -I"$INCLUDE" "$TEST_DIR/cpp_smoke.cpp" \
  -L"$LIB_DIR" -lscah_ffi -lpthread -ldl -lm \
  -o "$OUT_DIR/cpp_smoke"
"$OUT_DIR/cpp_smoke"

echo "C/C++ FFI smoke tests passed"
