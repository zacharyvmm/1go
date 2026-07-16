#!/usr/bin/env bash
set -euo pipefail

# ── Configuration ───────────────────────────────────────────────────────────

BASE_REF="${BASE_REF:-origin/main}"
BENCH="${BENCH:-core_regression}"
PROFILE="${SCAH_BENCH_PROFILE:-full}"
BASELINE_NAME="${BASELINE_NAME:-main}"

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

# ── Dirty working tree check ────────────────────────────────────────────────

if ! git -C "$ROOT" diff --quiet ||
   ! git -C "$ROOT" diff --cached --quiet; then
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
    echo >&2
    echo "Alternatively, compare against a revision that already contains the harness:" >&2
    echo >&2
    echo "    BASE_REF=<commit> just bench-compare" >&2
    exit 1
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

DIRTY_COUNT="$(
    git -C "$ROOT" status --porcelain |
    wc -l |
    tr -d ' '
)"

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
date_utc=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
EOF

echo
echo "Benchmark comparison complete."
echo "Criterion report:"
echo "  $REPORT_DIR/report/index.html"
echo "Metadata:"
echo "  $REPORT_DIR/metadata.txt"
