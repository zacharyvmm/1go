#!/usr/bin/env bash
# Compare binding performance between two git revisions.
#
# Always runs harness scripts from the invoking checkout. Each revision builds
# its own bindings. C ABI benchmarks run only when the revision provides a
# compatible scah-ffi.
#
# Usage:
#   ./scripts/compare-binding-performance.sh [baseline-ref] [candidate-ref]
#
# Defaults: origin/main vs HEAD.
# Pass candidate-ref as WORKTREE to measure the current dirty working tree.
#
# Env:
#   THRESHOLD  max allowed median regression percent (default 5)
#   OUT_ROOT   output directory (default temp dir)
#   SMOKE=1    minimal iterations for CI pipeline correctness
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BASE_REF="${1:-origin/main}"
CAND_REF="${2:-HEAD}"
THRESHOLD="${THRESHOLD:-5}"
OUT_ROOT="${OUT_ROOT:-$(mktemp -d /tmp/scah-bench-compare.XXXXXX)}"
SMOKE="${SMOKE:-0}"

mkdir -p "$OUT_ROOT"

if [[ "$CAND_REF" == "WORKTREE" ]]; then
  CAND_SHA="$(git rev-parse HEAD)-worktree"
  CAND_LABEL="worktree"
else
  CAND_SHA="$(git rev-parse "$CAND_REF")"
  CAND_LABEL="$CAND_REF"
fi
BASE_SHA="$(git rev-parse "$BASE_REF")"

echo "output directory: $OUT_ROOT"
echo "baseline: $BASE_REF -> $BASE_SHA"
echo "candidate: $CAND_LABEL -> $CAND_SHA"

cat >"$OUT_ROOT/meta.json" <<EOF
{
  "baseline_ref": "$BASE_REF",
  "baseline_sha": "$BASE_SHA",
  "candidate_ref": "$CAND_LABEL",
  "candidate_sha": "$CAND_SHA",
  "threshold_pct": $THRESHOLD,
  "machine": "$(uname -n)",
  "os": "$(uname -srm)",
  "date": "$(date -Iseconds)"
}
EOF

run_node_bench() {
  local node_root="$1"
  local out_json="$2"
  cp "$ROOT/scripts/bench-result-access.ts" "$node_root/bench-result-access.ts"
  python3 - <<PY
from pathlib import Path
p = Path("$node_root/bench-result-access.ts")
text = p.read_text()
text = text.replace(
    "from '../crates/bindings/scah-node/index.js'",
    "from './index.js'",
)
p.write_text(text)
PY
  (
    cd "$node_root"
    if [[ "$SMOKE" == "1" ]]; then
      bun bench-result-access.ts --smoke --output "$out_json"
    else
      bun bench-result-access.ts --samples 15 --output "$out_json"
    fi
  )
}

run_python_bench() {
  local worktree="$1"
  local out_json="$2"
  (
    cd "$worktree/crates/bindings/scah-python"
    uv sync -q
    # Force a fresh extension build; editable installs can otherwise leave a stale .so.
    uv pip install -e . --reinstall --no-cache -q
    if [[ "$SMOKE" == "1" ]]; then
      .venv/bin/python "$ROOT/scripts/bench-result-access.py" --smoke --output "$out_json"
    else
      .venv/bin/python "$ROOT/scripts/bench-result-access.py" --samples 15 --output "$out_json"
    fi
  )
}

run_c_abi_bench() {
  local worktree="$1"
  local out="$2"
  if [[ ! -f "$worktree/crates/scah-ffi/Cargo.toml" ]]; then
    echo "{\"language\":\"c-abi\",\"results\":[],\"skipped\":\"no scah-ffi\"}" >"$out/c-abi.json"
    return 0
  fi
  (
    cd "$worktree"
    cargo build -p scah-ffi --release
    TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | python3 -c "import sys,json; print(json.load(sys.stdin)['target_directory'])")"
    LIB_DIR="$TARGET_DIR/release"
    export LD_LIBRARY_PATH="$LIB_DIR:${LD_LIBRARY_PATH:-}"
    INCLUDE="$worktree/crates/scah-ffi/include"
    if [[ ! -f "$INCLUDE/scah.h" ]]; then
      INCLUDE="$ROOT/crates/scah-ffi/include"
    fi
    if ! cc -std=c11 -O2 -Wall -Wextra -Wpedantic \
      -I"$INCLUDE" \
      "$ROOT/crates/scah-ffi/benches/c_abi_bench.c" \
      -L"$LIB_DIR" -lscah_ffi -lpthread -ldl -lm \
      -o "$out/c_abi_bench" 2>"$out/c-abi-build.log"; then
      echo "{\"language\":\"c-abi\",\"results\":[],\"skipped\":\"compile failed\"}" >"$out/c-abi.json"
      echo "C ABI benchmark compilation failed (see $out/c-abi-build.log)" >&2
      return 1
    fi
    if [[ "$SMOKE" == "1" ]]; then
      "$out/c_abi_bench" --smoke >"$out/c-abi.json"
    else
      "$out/c_abi_bench" --samples 15 >"$out/c-abi.json"
    fi
  )
}

run_in_dir() {
  local label="$1"
  local worktree="$2"
  local out="$OUT_ROOT/$label"
  mkdir -p "$out"

  (
    cd "$worktree"
    run_python_bench "$worktree" "$out/python.json"
    (
      cd crates/bindings/scah-node
      bun install
      bun run build
    )
    run_node_bench "$worktree/crates/bindings/scah-node" "$out/node.json"
    run_c_abi_bench "$worktree" "$out"
  )
}

run_label() {
  local label="$1"
  local sha="$2"
  local worktree="$OUT_ROOT/wt-$label"
  mkdir -p "$OUT_ROOT/$label"

  if git -C "$ROOT" worktree list | grep -q "$worktree"; then
    git worktree remove --force "$worktree" || true
  fi
  rm -rf "$worktree"
  git worktree add --detach "$worktree" "$sha"
  run_in_dir "$label" "$worktree"
  git worktree remove --force "$worktree"
}

run_label baseline "$BASE_SHA"

if [[ "$CAND_REF" == "WORKTREE" ]]; then
  echo "measuring candidate from current working tree"
  run_in_dir candidate "$ROOT"
else
  run_label candidate "$CAND_SHA"
fi

REQUIRED_PY=(
  lookup_only_100
  lookup_only_1000
  field_name_1k_from_1000
  attrs_1k_from_1000
  nested_lookup
)
REQUIRED_NODE=(
  lookup_only_100
  lookup_only_1000
  field_name_1k_from_1000
  attrs_1k_from_1000
  toJson_1k_from_1000
  nested_lookup
)
if [[ "$SMOKE" != "1" ]]; then
  REQUIRED_PY+=(lookup_only_10000 lookup_only_100000)
  REQUIRED_NODE+=(lookup_only_10000 lookup_only_100000)
fi

status=0
for kind in python node; do
  base="$OUT_ROOT/baseline/$kind.json"
  cand="$OUT_ROOT/candidate/$kind.json"
  if [[ ! -f "$base" || ! -f "$cand" ]]; then
    echo "missing $kind results" >&2
    status=1
    continue
  fi
  req_args=()
  if [[ "$kind" == "python" ]]; then
    for name in "${REQUIRED_PY[@]}"; do
      req_args+=(--required "$name")
    done
  else
    for name in "${REQUIRED_NODE[@]}"; do
      req_args+=(--required "$name")
    done
  fi
  if ! python3 "$ROOT/scripts/compare_bench_results.py" \
    "$base" "$cand" \
    --threshold "$THRESHOLD" \
    "${req_args[@]}" \
    --output "$OUT_ROOT/compare-$kind.json"; then
    status=1
  fi
done

if [[ -f "$OUT_ROOT/baseline/c-abi.json" && -f "$OUT_ROOT/candidate/c-abi.json" ]]; then
  base_count="$(python3 -c "import json;print(len(json.load(open('$OUT_ROOT/baseline/c-abi.json')).get('results',[])))")"
  cand_count="$(python3 -c "import json;print(len(json.load(open('$OUT_ROOT/candidate/c-abi.json')).get('results',[])))")"
  if [[ "$base_count" == "0" && "$cand_count" != "0" ]]; then
    echo "C ABI present only on candidate — reporting absolute measurements (no main baseline)"
  elif [[ "$base_count" != "0" && "$cand_count" != "0" ]]; then
    if ! python3 "$ROOT/scripts/compare_bench_results.py" \
      "$OUT_ROOT/baseline/c-abi.json" \
      "$OUT_ROOT/candidate/c-abi.json" \
      --threshold "$THRESHOLD" \
      --output "$OUT_ROOT/compare-c-abi.json"; then
      status=1
    fi
  else
    echo "skipping C ABI threshold gate (baseline or candidate lacked comparable ABI)"
  fi
fi

echo "comparison artifacts in $OUT_ROOT"
exit "$status"
