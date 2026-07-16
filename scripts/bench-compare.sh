#!/usr/bin/env bash
set -euo pipefail

# ── Configuration ───────────────────────────────────────────────────────────

BASE_REF="${BASE_REF:-origin/main}"
BENCH="${BENCH:-core_regression}"
PROFILE="${SCAH_BENCH_PROFILE:-full}"
BASELINE_NAME="${BASELINE_NAME:-main}"
ALLOW_BENCH_HARNESS_DIFF="${ALLOW_BENCH_HARNESS_DIFF:-0}"
BENCH_HARNESS_PATH="benches/regression"
# ── Resolve repository root ─────────────────────────────────────────────────

ROOT="$(git rev-parse --show-toplevel)"

if ! git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "error: not inside a Git repository" >&2
    exit 1
fi

# ── Resolve revisions ───────────────────────────────────────────────────────

if ! BASE_SHA="$(git -C "$ROOT" rev-parse "$BASE_REF" 2>/dev/null)"; then
    echo "error: cannot resolve base revision: $BASE_REF" >&2
    exit 1
fi

HEAD_SHA="$(git -C "$ROOT" rev-parse HEAD)"

echo "Benchmark comparison"
echo "  base ref:  $BASE_REF"
echo "  base SHA:  $BASE_SHA"
echo "  head SHA:  $HEAD_SHA"
echo "  benchmark: $BENCH"
echo "  profile:   $PROFILE"

# ── Pre-flight: baseline must contain the regression package ────────────────

if ! git -C "$ROOT" cat-file \
    -e "$BASE_SHA:benches/regression/Cargo.toml" \
    2>/dev/null; then
    echo
    echo "error: The baseline revision does not contain the SCaH regression benchmark package." >&2
    echo >&2
    echo "This is expected while introducing the benchmark infrastructure." >&2
    echo "Merge the infrastructure first, then use it as the baseline for subsequent performance changes." >&2
    echo >&2
    echo "Alternatively, compare against a revision that already contains the harness:" >&2
    echo >&2
    echo "    just bench-compare <commit>" >&2
    echo >&2
    echo "For direct script invocation:" >&2
    echo >&2
    echo "    BASE_REF=<commit> ./scripts/bench-compare.sh" >&2
fi

# ── Harness-integrity check ─────────────────────────────────────────────────

TRACKED_HARNESS_DIFF="$(
    git -C "$ROOT" diff \
        --name-only \
        "$BASE_SHA" \
        -- "$BENCH_HARNESS_PATH"
)"

UNTRACKED_HARNESS_FILES="$(
    git -C "$ROOT" ls-files \
        --others \
        --exclude-standard \
        -- "$BENCH_HARNESS_PATH"
)"

if [ -n "$TRACKED_HARNESS_DIFF" ] ||
   [ -n "$UNTRACKED_HARNESS_FILES" ]; then
    HARNESS_DIFF_PRESENT=1
else
    HARNESS_DIFF_PRESENT=0
fi

if [ "$HARNESS_DIFF_PRESENT" = 1 ]; then
    echo >&2
    echo "error: regression benchmark harness differs from the baseline revision" >&2
    echo >&2

    if [ -n "$TRACKED_HARNESS_DIFF" ]; then
        echo "Tracked benchmark differences:" >&2
        printf '%s\n' "$TRACKED_HARNESS_DIFF" |
            sed 's/^/  /' >&2
    fi

    if [ -n "$UNTRACKED_HARNESS_FILES" ]; then
        echo "Untracked benchmark files:" >&2
        printf '%s\n' "$UNTRACKED_HARNESS_FILES" |
            sed 's/^/  /' >&2
    fi

    echo >&2
    echo "Criterion can compare only equivalent workloads." >&2
    echo "Merge benchmark-harness changes separately before benchmarking production changes." >&2

    if [ "$ALLOW_BENCH_HARNESS_DIFF" != "1" ]; then
        echo >&2
        echo "For benchmark-infrastructure development only, override with:" >&2
        echo >&2
        echo "  ALLOW_BENCH_HARNESS_DIFF=1 just bench-compare <base>" >&2
        exit 1
    fi

    echo >&2
    echo "WARNING: ALLOW_BENCH_HARNESS_DIFF=1 is set." >&2
    echo "The resulting performance comparison may compare different workloads." >&2
fi

# ── Dirty working tree check ────────────────────────────────────────────────

WORKTREE_STATUS="$(git -C "$ROOT" status --porcelain)"

if [ -n "$WORKTREE_STATUS" ]; then
    echo "  working tree: dirty; current measurements include local changes"
else
    echo "  working tree: clean"
fi

# ── Temporary directories ───────────────────────────────────────────────────

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/scah-bench-compare.XXXXXX")"
BASE_WORKTREE="$TEMP_ROOT/base"
BASE_TARGET="$TEMP_ROOT/base-target"
HEAD_TARGET="$TEMP_ROOT/head-target"

cleanup() {
    git -C "$ROOT" worktree remove --force "$BASE_WORKTREE" \
        >/dev/null 2>&1 || true
    rm -rf "$TEMP_ROOT"
}

trap cleanup EXIT INT TERM

# ── Create detached worktree for baseline ───────────────────────────────────

echo
echo "Creating detached worktree for baseline..."

if ! git -C "$ROOT" worktree add \
    --detach \
    "$BASE_WORKTREE" \
    "$BASE_SHA" >/dev/null 2>&1; then
    echo "error: failed to create worktree for $BASE_SHA" >&2
    exit 1
fi

# ── Verify regression bench package exists at baseline ──────────────────────

if [ ! -f "$BASE_WORKTREE/benches/regression/Cargo.toml" ]; then
    echo
    echo "error: The baseline revision does not contain the SCaH regression benchmark package." >&2
    echo >&2
    echo "This is expected while introducing the benchmark infrastructure." >&2
    echo "Merge the infrastructure first, then use it as the baseline for subsequent performance changes." >&2
    echo "Alternatively, compare against a revision that already contains the harness:" >&2
    echo >&2
    echo "    just bench-compare <commit>" >&2
    echo >&2
    echo "For direct script invocation:" >&2
    echo >&2
    echo "    BASE_REF=<commit> ./scripts/bench-compare.sh" >&2
fi

# ── Environment ─────────────────────────────────────────────────────────────

export CARGO_INCREMENTAL=0
export SCAH_BENCH_PROFILE="$PROFILE"

# ── Compile both revisions before measuring ─────────────────────────────────

echo
echo "Compiling baseline benchmark..."

(
    cd "$BASE_WORKTREE"
    CARGO_TARGET_DIR="$BASE_TARGET" \
        cargo bench \
        -p scah-regression-benches \
        --bench "$BENCH" \
        --no-run
)

echo
echo "Compiling current benchmark..."

(
    cd "$ROOT"
    CARGO_TARGET_DIR="$HEAD_TARGET" \
        cargo bench \
        -p scah-regression-benches \
        --bench "$BENCH" \
        --no-run
)

# ── Measure baseline ────────────────────────────────────────────────────────

echo
echo "Measuring baseline..."

(
    cd "$BASE_WORKTREE"
    CARGO_TARGET_DIR="$BASE_TARGET" \
        cargo bench \
        -p scah-regression-benches \
        --bench "$BENCH" \
        -- \
        --save-baseline "$BASELINE_NAME" \
        --noplot
)

if [ ! -d "$BASE_TARGET/criterion" ]; then
    echo "error: Criterion did not produce a baseline directory at $BASE_TARGET/criterion" >&2
    exit 1
fi

# ── Copy Criterion measurement data ─────────────────────────────────────────

mkdir -p "$HEAD_TARGET"
rm -rf "$HEAD_TARGET/criterion"
cp -a "$BASE_TARGET/criterion" "$HEAD_TARGET/criterion"

# ── Measure current working tree ────────────────────────────────────────────

echo
echo "Measuring current working tree..."

(
    cd "$ROOT"
    CARGO_TARGET_DIR="$HEAD_TARGET" \
        cargo bench \
        -p scah-regression-benches \
        --bench "$BENCH" \
        -- \
        --baseline "$BASELINE_NAME"
)

# ── Copy final report ───────────────────────────────────────────────────────

REPORT_ROOT="$ROOT/target/bench-compare"
REPORT_DIR="$REPORT_ROOT/latest"

mkdir -p "$REPORT_ROOT"
rm -rf "$REPORT_DIR"
mkdir -p "$REPORT_DIR"

cp -a "$HEAD_TARGET/criterion/." "$REPORT_DIR/"
# ── Metadata ────────────────────────────────────────────────────────────────

if [ -n "$WORKTREE_STATUS" ]; then
    DIRTY_COUNT="$(
        printf '%s\n' "$WORKTREE_STATUS" |
        wc -l |
        tr -d ' '
    )"
else
    DIRTY_COUNT=0
fi

RUSTC_VERSION="$(rustc --version 2>/dev/null || echo "unknown")"
CARGO_VERSION="$(cargo --version 2>/dev/null || echo "unknown")"
HOST_TRIPLE="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p' || echo "unknown")"

cat > "$REPORT_DIR/metadata.txt" <<EOF
base_ref=$BASE_REF
base_sha=$BASE_SHA
head_sha=$HEAD_SHA
head_dirty_files=$DIRTY_COUNT
benchmark=$BENCH
profile=$PROFILE
rustc=$RUSTC_VERSION
cargo=$CARGO_VERSION
host=$HOST_TRIPLE
benchmark_harness_diff_present=$HARNESS_DIFF_PRESENT
benchmark_harness_diff_allowed=$ALLOW_BENCH_HARNESS_DIFF
date_utc=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
EOF

if [ "$HARNESS_DIFF_PRESENT" = 1 ]; then
    {
        if [ -n "$TRACKED_HARNESS_DIFF" ]; then
            echo "[tracked]"
            printf '%s\n' "$TRACKED_HARNESS_DIFF"
        fi

        if [ -n "$UNTRACKED_HARNESS_FILES" ]; then
            echo "[untracked]"
            printf '%s\n' "$UNTRACKED_HARNESS_FILES"
        fi
    } > "$REPORT_DIR/harness-diff.txt"
fi

echo
echo "Benchmark comparison complete."
echo "Criterion report:"
echo "  $REPORT_DIR/report/index.html"
echo "Metadata:"
echo "  $REPORT_DIR/metadata.txt"
