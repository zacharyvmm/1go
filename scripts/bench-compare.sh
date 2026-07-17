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

# Capture the exact working-tree state before any Cargo invocation so we can
# detect mutations introduced by this script (for example a lockfile rewrite).
INITIAL_WORKTREE_STATUS="$(
    git -C "$ROOT" status --porcelain=v1 --untracked-files=all
)"

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
    exit 1
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

if [ -n "$INITIAL_WORKTREE_STATUS" ]; then
    echo "  working tree: dirty; current measurements include local changes"
else
    echo "  working tree: clean"
fi

# ── Temporary directories ───────────────────────────────────────────────────

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/scah-bench-compare.XXXXXX")"
BASE_WORKTREE="$TEMP_ROOT/base"
BASE_TARGET="$TEMP_ROOT/base-target"
HEAD_TARGET="$TEMP_ROOT/head-target"

REPORT_ROOT="$ROOT/target/bench-compare"
REPORT_DIR="$REPORT_ROOT/latest"
REPORT_STAGING="$REPORT_ROOT/.latest-staging-$$"
REPORT_BACKUP="$REPORT_ROOT/.latest-backup-$$"

cleanup() {
    git -C "$ROOT" worktree remove --force "$BASE_WORKTREE" \
        >/dev/null 2>&1 || true

    rm -rf "$TEMP_ROOT"

    if [ -n "${REPORT_STAGING:-}" ]; then
        rm -rf "$REPORT_STAGING"
    fi

    if [ -n "${REPORT_BACKUP:-}" ] &&
       [ -e "$REPORT_BACKUP" ] &&
       [ ! -e "${REPORT_DIR:-}" ]; then
        mv "$REPORT_BACKUP" "$REPORT_DIR" >/dev/null 2>&1 || true
    fi
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
    exit 1
fi

# ── Environment ─────────────────────────────────────────────────────────────

export CARGO_INCREMENTAL=0
export SCAH_BENCH_PROFILE="$PROFILE"

run_cargo_bench() {
    cargo bench --locked "$@" || {
        echo >&2
        echo "error: cargo bench --locked failed." >&2
        echo "Inspect the Cargo output above for the actual cause." >&2
        echo "Cargo.lock was not updated automatically." >&2
        return 1
    }
}

# ── Compile both revisions before measuring ─────────────────────────────────

echo
echo "Compiling baseline benchmark..."

(
    cd "$BASE_WORKTREE"
    CARGO_TARGET_DIR="$BASE_TARGET" \
        run_cargo_bench \
        -p scah-regression-benches \
        --bench "$BENCH" \
        --no-run
)

echo
echo "Compiling current benchmark..."

(
    cd "$ROOT"
    CARGO_TARGET_DIR="$HEAD_TARGET" \
        run_cargo_bench \
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
        run_cargo_bench \
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
        run_cargo_bench \
        -p scah-regression-benches \
        --bench "$BENCH" \
        -- \
        --baseline "$BASELINE_NAME"
)

# ── Post-run mutation detection ─────────────────────────────────────────────

FINAL_WORKTREE_STATUS="$(
    git -C "$ROOT" status --porcelain=v1 --untracked-files=all
)"

if [ "$FINAL_WORKTREE_STATUS" != "$INITIAL_WORKTREE_STATUS" ]; then
    echo "error: benchmark comparison modified the working tree" >&2
    echo >&2
    echo "Initial status:" >&2
    if [ -n "$INITIAL_WORKTREE_STATUS" ]; then
        printf '%s\n' "$INITIAL_WORKTREE_STATUS" | sed 's/^/  /' >&2
    else
        echo "  (clean)" >&2
    fi
    echo >&2
    echo "Final status:" >&2
    if [ -n "$FINAL_WORKTREE_STATUS" ]; then
        printf '%s\n' "$FINAL_WORKTREE_STATUS" | sed 's/^/  /' >&2
    else
        echo "  (clean)" >&2
    fi
    exit 1
fi

# ── Build staged report ─────────────────────────────────────────────────────

mkdir -p "$REPORT_ROOT"
rm -rf "$REPORT_STAGING" "$REPORT_BACKUP"
mkdir -p "$REPORT_STAGING"

cp -a "$HEAD_TARGET/criterion/." "$REPORT_STAGING/"

# ── Metadata ────────────────────────────────────────────────────────────────

if [ -n "$INITIAL_WORKTREE_STATUS" ]; then
    DIRTY_COUNT="$(
        printf '%s\n' "$INITIAL_WORKTREE_STATUS" |
        wc -l |
        tr -d ' '
    )"
else
    DIRTY_COUNT=0
fi

RUSTC_VERSION="$(rustc --version 2>/dev/null || echo "unknown")"
CARGO_VERSION="$(cargo --version 2>/dev/null || echo "unknown")"
HOST_TRIPLE="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p' || echo "unknown")"

cat > "$REPORT_STAGING/metadata.txt" <<EOF
base_ref=$BASE_REF
base_sha=$BASE_SHA
head_sha=$HEAD_SHA
head_dirty_files=$DIRTY_COUNT
working_tree_unchanged=true
cargo_locked=true
benchmark=$BENCH
profile=$PROFILE
rustc=$RUSTC_VERSION
cargo=$CARGO_VERSION
host=$HOST_TRIPLE
benchmark_harness_diff_present=$HARNESS_DIFF_PRESENT
benchmark_harness_diff_allowed=$ALLOW_BENCH_HARNESS_DIFF
date_utc=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
EOF

printf '%s\n' "$INITIAL_WORKTREE_STATUS" > "$REPORT_STAGING/working-tree-status.txt"

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
    } > "$REPORT_STAGING/harness-diff.txt"
fi

# ── Validate staged report ──────────────────────────────────────────────────

if [ ! -f "$REPORT_STAGING/report/index.html" ]; then
    echo "error: Criterion report index was not generated" >&2
    exit 1
fi

if [ ! -f "$REPORT_STAGING/metadata.txt" ]; then
    echo "error: benchmark metadata was not generated" >&2
    exit 1
fi

# ── Publish staged report ───────────────────────────────────────────────────

if [ -e "$REPORT_DIR" ]; then
    mv "$REPORT_DIR" "$REPORT_BACKUP"
fi

if mv "$REPORT_STAGING" "$REPORT_DIR"; then
    rm -rf "$REPORT_BACKUP"
else
    echo "error: failed to publish benchmark report" >&2

    if [ -e "$REPORT_BACKUP" ]; then
        mv "$REPORT_BACKUP" "$REPORT_DIR" || true
    fi

    exit 1
fi

echo
echo "Benchmark comparison complete."
echo "Criterion report:"
echo "  $REPORT_DIR/report/index.html"
echo "Metadata:"
echo "  $REPORT_DIR/metadata.txt"
