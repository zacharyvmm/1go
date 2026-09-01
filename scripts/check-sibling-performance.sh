#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -m)" != "x86_64" ]]; then
  echo "The sibling performance gate must run on x86-64."
  exit 1
fi

repo_root="$(git rev-parse --show-toplevel)"
base_ref="${1:-origin/main}"
gate_root="$(mktemp -d -t scah-sibling-performance.XXXXXX)"
base_tree="$gate_root/base"

cleanup() {
  if git -C "$repo_root" worktree list --porcelain | grep -Fqx "worktree $base_tree"; then
    git -C "$repo_root" worktree remove --force "$base_tree"
  fi
  rm -rf "$gate_root"
}
trap cleanup EXIT

git -C "$repo_root" worktree add --detach "$base_tree" "$base_ref"
mkdir -p "$base_tree/benches/ordinary_gate"
cp "$repo_root/benches/ordinary_gate/speed_bench.rs" \
  "$base_tree/benches/ordinary_gate/speed_bench.rs"

if ! grep -Fq 'name = "speed_bench_ordinary_gate"' "$base_tree/benches/Cargo.toml"; then
  printf '\n[[bench]]\nname = "speed_bench_ordinary_gate"\npath = "ordinary_gate/speed_bench.rs"\nharness = false\n' \
    >> "$base_tree/benches/Cargo.toml"
fi

build_benchmark() {
  local source_root="$1"
  local destination="$2"
  local build_target="$3"
  CARGO_TARGET_DIR="$build_target" cargo bench \
    --manifest-path "$source_root/Cargo.toml" \
    -p scah-benches \
    --bench speed_bench_ordinary_gate \
    --no-run
  local executable
  executable="$(find "$build_target/release/deps" -maxdepth 1 -type f \
    -name 'speed_bench_ordinary_gate-*' -perm -111 | head -n 1)"
  test -n "$executable"
  cp "$executable" "$destination"
}

build_benchmark "$base_tree" "$gate_root/base-benchmark" "$gate_root/base-target"
build_benchmark \
  "$repo_root" "$gate_root/candidate-benchmark" "$gate_root/candidate-target"

cd "$repo_root"
run_round() {
  local executable="$1"
  local baseline_name="$2"
  "$executable" --bench ordinary_parser_gate \
    --warm-up-time 1 --measurement-time 2 --noplot \
    --save-baseline "$baseline_name"
}

for round in 1 2 3; do
  if ((round % 2 == 0)); then
    run_round "$gate_root/candidate-benchmark" "sibling-candidate-$round"
    run_round "$gate_root/base-benchmark" "sibling-main-$round"
  else
    run_round "$gate_root/base-benchmark" "sibling-main-$round"
    run_round "$gate_root/candidate-benchmark" "sibling-candidate-$round"
  fi
done

python3 - "$repo_root/target/criterion/ordinary_parser_gate" <<'PY'
import json
import pathlib
import statistics
import sys

root = pathlib.Path(sys.argv[1])
limit = 1.05
failed = False

for workload in ("no_match", "match"):
    case = root / workload / "10000"

    def estimate(prefix: str, round_number: int) -> float:
        estimates = case / f"{prefix}-{round_number}" / "estimates.json"
        with estimates.open(encoding="utf-8") as handle:
            return json.load(handle)["slope"]["point_estimate"]

    ratios = [
        estimate("sibling-candidate", round_number)
        / estimate("sibling-main", round_number)
        for round_number in (1, 2, 3)
    ]
    ratio = statistics.median(ratios)
    delta = (ratio - 1.0) * 100.0
    print(f"{workload}: {delta:+.2f}% vs main")
    failed |= ratio > limit

if failed:
    print("ordinary parser regression exceeds the 5% limit", file=sys.stderr)
    raise SystemExit(1)
PY
