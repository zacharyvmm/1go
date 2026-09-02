#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -m)" != "x86_64" && "${SCAH_ALLOW_UNSUPPORTED_PERF_HOST:-0}" != "1" ]]; then
  echo "The text performance gate must run on x86-64."
  exit 1
fi

repo_root="$(git rev-parse --show-toplevel)"
base_ref="${1:-origin/main}"
gate_root="$(mktemp -d -t scah-text-performance.XXXXXX)"
base_tree="$gate_root/base"
rounds="${SCAH_PERF_GATE_ROUNDS:-3}"
warm_up_time="${SCAH_PERF_GATE_WARM_UP_TIME:-1}"
measurement_time="${SCAH_PERF_GATE_MEASUREMENT_TIME:-2}"

cleanup() {
  if git -C "$repo_root" worktree list --porcelain | grep -Fqx "worktree $base_tree"; then
    git -C "$repo_root" worktree remove --force "$base_tree"
  fi
  rm -rf "$gate_root"
}
trap cleanup EXIT

git -C "$repo_root" worktree add --detach "$base_tree" "$base_ref"
mkdir -p "$base_tree/benches/text_extraction"
if grep -Fq 'pub raw_text: bool' \
  "$base_tree/crates/scah-query-ir/src/query/compiler/builder.rs"; then
  legacy_base=0
  cp "$repo_root/benches/text_extraction/gate.rs" \
    "$base_tree/benches/text_extraction/gate.rs"
else
  legacy_base=1
  cp "$repo_root/benches/text_extraction/gate_legacy.rs" \
    "$base_tree/benches/text_extraction/gate.rs"
fi

if ! grep -Fq 'name = "speed_bench_text_gate"' "$base_tree/benches/Cargo.toml"; then
  printf '\n[[bench]]\nname = "speed_bench_text_gate"\npath = "text_extraction/gate.rs"\nharness = false\n' \
    >> "$base_tree/benches/Cargo.toml"
fi

build_benchmark() {
  local source_root="$1"
  local destination="$2"
  local build_target="$3"
  CARGO_TARGET_DIR="$build_target" cargo bench \
    --manifest-path "$source_root/Cargo.toml" \
    -p scah-benches \
    --bench speed_bench_text_gate \
    --no-run
  local executable
  executable="$(find "$build_target/release/deps" -maxdepth 1 -type f \
    -name 'speed_bench_text_gate-*' -perm -111 | head -n 1)"
  test -n "$executable"
  cp "$executable" "$destination"
}

build_benchmark "$base_tree" "$gate_root/base-benchmark" "$gate_root/base-target"
build_benchmark "$repo_root" "$gate_root/candidate-benchmark" "$gate_root/candidate-target"

cd "$repo_root"
run_round() {
  local executable="$1"
  local baseline_name="$2"
  "$executable" --bench text_extraction_gate \
    --warm-up-time "$warm_up_time" --measurement-time "$measurement_time" --noplot \
    --save-baseline "$baseline_name"
}

for ((round = 1; round <= rounds; round++)); do
  if ((round % 2 == 0)); then
    run_round "$gate_root/candidate-benchmark" "text-candidate-$round"
    run_round "$gate_root/base-benchmark" "text-main-$round"
  else
    run_round "$gate_root/base-benchmark" "text-main-$round"
    run_round "$gate_root/candidate-benchmark" "text-candidate-$round"
  fi
done

python3 - "$repo_root/target/criterion/text_extraction_gate" "$rounds" "$legacy_base" <<'PY'
import json
import pathlib
import statistics
import sys

root = pathlib.Path(sys.argv[1])
rounds = int(sys.argv[2])
legacy_base = bool(int(sys.argv[3]))
failed = False
workloads = (
    "no_content",
    "inner_html_only",
    "text_only_no_matches",
    "text_only_sparse_matches",
    "text_only_prose",
    "raw_only",
)

for workload in workloads:
    case = root / workload / "1000"

    def estimate(prefix: str, round_number: int) -> float:
        estimates = case / f"{prefix}-{round_number}" / "estimates.json"
        with estimates.open(encoding="utf-8") as handle:
            return json.load(handle)["slope"]["point_estimate"]

    ratios = [
        estimate("text-candidate", round_number)
        / estimate("text-main", round_number)
        for round_number in range(1, rounds + 1)
    ]
    ratio = statistics.median(ratios)
    # Legacy main performs simple text accumulation. The new normalized-text
    # contract also decodes entities and handles block, hidden, and preformatted
    # content. Keep a one-time compatibility budget for that non-equivalent case.
    # Once main has the raw-text API, every workload uses the strict limit.
    limit = 2.30 if legacy_base and workload == "text_only_prose" else 1.10
    delta = (ratio - 1.0) * 100.0
    print(f"{workload}: {delta:+.2f}% vs main (limit {(limit - 1.0) * 100:.0f}%)")
    if ratio > limit:
        print(f"{workload} exceeds its performance limit", file=sys.stderr)
        failed = True

if failed:
    raise SystemExit(1)
PY
