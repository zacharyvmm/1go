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
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if ! git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "error: not inside a Git repository" >&2
    exit 1
fi

# ── Helper: portable SHA-256 ────────────────────────────────────────────────

sha256_hash() {
    # Compute SHA-256 of a file. Tries sha256sum (Linux), shasum (macOS),
    # then falls back to Python's hashlib.
    local file="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -b "$file" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$file" | cut -d' ' -f1
    elif command -v python3 >/dev/null 2>&1; then
        python3 -c \
            "import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],'rb').read()).hexdigest())" \
            "$file"
    else
        echo "error: no SHA-256 tool found (need sha256sum, shasum, or python3)" >&2
        return 1
    fi
}

# ── Resolve revisions ───────────────────────────────────────────────────────

resolve_revisions() {
    if ! BASE_SHA="$(git -C "$ROOT" rev-parse "$BASE_REF" 2>/dev/null)"; then
        echo "error: cannot resolve base revision: $BASE_REF" >&2
        exit 1
    fi

    HEAD_SHA="$(git -C "$ROOT" rev-parse HEAD)"
}

# ── Capture current source state (before any Cargo invocation) ──────────────

capture_source_state() {
    local max_attempts=3
    local attempt=1
    local special_exhausted=0

    while [ "$attempt" -le "$max_attempts" ]; do
        SPECIAL_SCAN_BEFORE="$TEMP_ROOT/special-scan-before.txt"
        SPECIAL_SCAN_AFTER="$TEMP_ROOT/special-scan-after.txt"

        # Endpoint A: reject specials present at attempt start.
        if ! python3 "$SCRIPT_DIR/scan-special-files.py" \
            --root "$ROOT" \
            --manifest "$SPECIAL_SCAN_BEFORE"; then
            echo "error: unsupported special file present before source capture" >&2
            echo "  attempt: $attempt/$max_attempts" >&2
            exit 1
        fi

        # Working-tree status for dirty-file count and diagnostics.
        INITIAL_WORKTREE_STATUS="$(
            git -C "$ROOT" status --porcelain=v1 --untracked-files=all
        )"

        # Tracked changes (staged + unstaged) relative to HEAD.
        TRACKED_DIFF="$TEMP_ROOT/tracked-diff.patch"
        git -C "$ROOT" diff --binary HEAD > "$TRACKED_DIFF"

        # Untracked, non-ignored files (NUL-delimited).
        UNTRACKED_LIST="$TEMP_ROOT/untracked-files.txt"
        git -C "$ROOT" ls-files --others --exclude-standard -z \
            > "$UNTRACKED_LIST"

        # Test hook: block after untracked list so tests can mutate files.
        if [ -n "${SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_LIST:-}" ]; then
            printf '%s\n' "$attempt" \
                > "${SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_LIST}.started"
            rm -f "${SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_LIST}.released"
            while [ ! -e "${SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_LIST}.released" ]; do
                sleep 0.05
            done
            rm -f "${SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_LIST}.released"
        fi

        # Stage untracked entries into a temporary capture directory.
        CAPTURED_UNTRACKED="$TEMP_ROOT/captured-untracked"
        UNTRACKED_CAPTURE_MANIFEST="$TEMP_ROOT/untracked-capture-manifest.jsonl"

        if [ -d "$CAPTURED_UNTRACKED" ]; then
            rm -rf "$CAPTURED_UNTRACKED"
        fi
        mkdir -p "$CAPTURED_UNTRACKED"

        if ! python3 "$SCRIPT_DIR/capture-untracked.py" capture \
            --root "$ROOT" \
            --paths "$UNTRACKED_LIST" \
            --destination "$CAPTURED_UNTRACKED" \
            --manifest "$UNTRACKED_CAPTURE_MANIFEST"; then
            echo "error: failed to capture untracked entries" >&2
            exit 1
        fi

        # Test hook: block after capture so tests can mutate files post-capture.
        if [ -n "${SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_CAPTURE:-}" ]; then
            printf '%s\n' "$attempt" \
                > "${SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_CAPTURE}.started"
            rm -f "${SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_CAPTURE}.released"
            while [ ! -e "${SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_CAPTURE}.released" ]; do
                sleep 0.05
            done
            rm -f "${SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_CAPTURE}.released"
        fi

        # ── Independent verification ──────────────────────────────────────

        # Re-capture tracked diff.
        local tracked_diff_verify="$TEMP_ROOT/tracked-diff-verify.patch"
        git -C "$ROOT" diff --binary HEAD > "$tracked_diff_verify"

        # Re-capture untracked path inventory.
        local untracked_list_verify="$TEMP_ROOT/untracked-verify.txt"
        git -C "$ROOT" ls-files --others --exclude-standard -z \
            > "$untracked_list_verify"

        # Inspect live untracked entries without modifying the staging directory.
        UNTRACKED_LIVE_VERIFY_MANIFEST="$TEMP_ROOT/untracked-live-verify-manifest.jsonl"
        if ! python3 "$SCRIPT_DIR/capture-untracked.py" inspect \
            --root "$ROOT" \
            --paths "$UNTRACKED_LIST" \
            --manifest "$UNTRACKED_LIVE_VERIFY_MANIFEST"; then
            echo "error: failed to inspect live untracked entries" >&2
            exit 1
        fi

        # Endpoint B: scan again before accepting coherence.
        local special_after_rc=0
        if ! python3 "$SCRIPT_DIR/scan-special-files.py" \
            --root "$ROOT" \
            --manifest "$SPECIAL_SCAN_AFTER"; then
            special_after_rc=1
        fi

        # ── Coherence check ───────────────────────────────────────────────

        local coherent=1

        if ! cmp -s "$TRACKED_DIFF" "$tracked_diff_verify"; then
            coherent=0
        fi

        if ! cmp -s "$UNTRACKED_LIST" "$untracked_list_verify"; then
            coherent=0
        fi

        if [ -s "$UNTRACKED_CAPTURE_MANIFEST" ] || [ -s "$UNTRACKED_LIVE_VERIFY_MANIFEST" ]; then
            if ! cmp -s "$UNTRACKED_CAPTURE_MANIFEST" "$UNTRACKED_LIVE_VERIFY_MANIFEST"; then
                coherent=0
            fi
        fi

        if [ "$special_after_rc" != 0 ]; then
            coherent=0
            echo "  unsupported special file appeared during source capture (attempt $attempt/$max_attempts)" >&2
        fi

        if ! cmp -s "$SPECIAL_SCAN_BEFORE" "$SPECIAL_SCAN_AFTER"; then
            coherent=0
        fi

        if [ "$coherent" = 1 ]; then
            break  # Capture is coherent.
        fi

        if [ "$special_after_rc" != 0 ]; then
            special_exhausted=1
        fi

        echo "  working tree changed during snapshot capture (attempt $attempt/$max_attempts)"
        attempt=$((attempt + 1))

        if [ "$attempt" -gt "$max_attempts" ]; then
            if [ "$special_exhausted" = 1 ]; then
                echo "error: unsupported special file appeared during source capture" >&2
                if [ -s "$SPECIAL_SCAN_AFTER" ]; then
                    python3 - "$SPECIAL_SCAN_AFTER" <<'PY'
import os
import sys

data = open(sys.argv[1], "rb").read()
parts = [part for part in data.split(b"\0") if part]
for index in range(0, len(parts) - 1, 2):
    entry_type = parts[index].decode("utf-8")
    rel_path = os.fsdecode(parts[index + 1])
    print(f"  type: {entry_type}", file=sys.stderr)
    print(f"  path: {rel_path}", file=sys.stderr)
PY
                fi
                echo "  attempt: $max_attempts/$max_attempts" >&2
            else
                echo "error: working tree changed during snapshot capture" >&2
                echo "  untracked content, type, mode, or symlink target changed" >&2
                echo "  attempts: $max_attempts" >&2
            fi
            exit 1
        fi

        # Clean up and retry.
        rm -rf "$CAPTURED_UNTRACKED"
        rm -f "$UNTRACKED_CAPTURE_MANIFEST"
        rm -f "$UNTRACKED_LIVE_VERIFY_MANIFEST"
        rm -f "$TRACKED_DIFF"
        rm -f "$tracked_diff_verify"
        rm -f "$UNTRACKED_LIST"
        rm -f "$untracked_list_verify"
        rm -f "$SPECIAL_SCAN_BEFORE"
        rm -f "$SPECIAL_SCAN_AFTER"
    done

    # Determine dirty state.
    HEAD_DIRTY="false"
    if [ -s "$TRACKED_DIFF" ] || [ -s "$UNTRACKED_LIST" ]; then
        HEAD_DIRTY="true"
    fi

    if [ -n "$INITIAL_WORKTREE_STATUS" ]; then
        HEAD_DIRTY_FILES="$(
            printf '%s\n' "$INITIAL_WORKTREE_STATUS" | wc -l | tr -d ' '
        )"
    else
        HEAD_DIRTY_FILES=0
    fi
}

# ── Harness-integrity check ─────────────────────────────────────────────────

check_harness_integrity() {
    # Detects committed, dirty tracked, and untracked changes under the
    # benchmark harness path. Must run before any benchmark execution.

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
}

# ── Lock helpers ─────────────────────────────────────────────────────────────

acquire_report_lock() {
    # Derive lock from git common dir so linked worktrees share the same lock.
    local git_common_dir
    if ! git_common_dir="$(git -C "$ROOT" rev-parse --git-common-dir 2>/dev/null)"; then
        echo "error: cannot determine git common directory" >&2
        exit 1
    fi
    git_common_dir="$(cd "$ROOT" && cd "$git_common_dir" && pwd)"
    REPORT_LOCK="$git_common_dir/scah-bench-compare.lock"

    if ! mkdir "$REPORT_LOCK" 2>/dev/null; then
        echo "error: another benchmark comparison is already running" >&2

        if [ -f "$REPORT_LOCK/pid" ]; then
            local lock_pid
            lock_pid="$(cat "$REPORT_LOCK/pid" 2>/dev/null || true)"

            if [ -n "$lock_pid" ]; then
                echo "  lock owner PID: $lock_pid" >&2
            fi
        fi

        if [ -f "$REPORT_LOCK/worktree" ]; then
            echo "  lock owner worktree: $(cat "$REPORT_LOCK/worktree" 2>/dev/null || true)" >&2
        fi

        echo "  lock path: $REPORT_LOCK" >&2
        echo >&2
        echo "If no comparison is running, remove the stale lock directory manually." >&2
        return 1
    fi

    REPORT_LOCK_HELD=1

    printf '%s\n' "$$" > "$REPORT_LOCK/pid"
    printf '%s\n' "$HEAD_SHA" > "$REPORT_LOCK/head-sha"
    printf '%s\n' "$BASE_SHA" > "$REPORT_LOCK/base-sha"
    printf '%s\n' "$(date -u +"%Y-%m-%dT%H:%M:%SZ")" \
        > "$REPORT_LOCK/started-at"
    hostname > "$REPORT_LOCK/hostname" 2>/dev/null || true
    printf '%s\n' "$ROOT" > "$REPORT_LOCK/worktree"
}

# ── Create base worktree ────────────────────────────────────────────────────

create_base_worktree() {
    echo
    echo "Creating detached worktree for baseline..."

    if ! git -C "$ROOT" worktree add \
        --detach \
        "$BASE_WORKTREE" \
        "$BASE_SHA" >/dev/null 2>&1; then
        echo "error: failed to create worktree for $BASE_SHA" >&2
        exit 1
    fi

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
}

# ── Create current snapshot ─────────────────────────────────────────────────

create_current_snapshot() {
    echo
    echo "Creating isolated current-tree snapshot..."

    # Detached worktree at HEAD.
    if ! git -C "$ROOT" worktree add \
        --detach \
        "$CURRENT_WORKTREE" \
        "$HEAD_SHA" >/dev/null 2>&1; then
        echo "error: failed to create worktree for $HEAD_SHA" >&2
        exit 1
    fi

    # Apply tracked dirty changes to reconstruct the working tree.
    if [ -s "$TRACKED_DIFF" ]; then
        if ! git -C "$CURRENT_WORKTREE" apply --binary "$TRACKED_DIFF"; then
            echo "error: failed to apply tracked changes to current snapshot" >&2
            exit 1
        fi
    fi

    # Restore untracked entries from staged capture; never read from $ROOT.
    if [ ! -f "$UNTRACKED_LIST" ] || [ ! -f "$UNTRACKED_CAPTURE_MANIFEST" ]; then
        echo "error: missing untracked capture artifacts" >&2
        exit 1
    fi

    if [ -s "$UNTRACKED_LIST" ]; then
        if [ ! -d "$CAPTURED_UNTRACKED" ]; then
            echo "error: captured untracked directory missing" >&2
            exit 1
        fi
        if ! python3 "$SCRIPT_DIR/capture-untracked.py" restore \
            --root "$CAPTURED_UNTRACKED" \
            --paths "$UNTRACKED_LIST" \
            --destination "$CURRENT_WORKTREE"; then
            echo "error: failed to restore untracked capture into current worktree" >&2
            exit 1
        fi
    fi

    RECONSTRUCTED_UNTRACKED_MANIFEST="$TEMP_ROOT/untracked-reconstructed-manifest.jsonl"
    if ! python3 "$SCRIPT_DIR/capture-untracked.py" inspect \
        --root "$CURRENT_WORKTREE" \
        --paths "$UNTRACKED_LIST" \
        --manifest "$RECONSTRUCTED_UNTRACKED_MANIFEST"; then
        echo "error: failed to inspect reconstructed untracked entries" >&2
        exit 1
    fi

    if ! cmp -s \
        "$UNTRACKED_CAPTURE_MANIFEST" \
        "$RECONSTRUCTED_UNTRACKED_MANIFEST"; then
        echo "error: reconstructed current worktree does not match accepted untracked capture" >&2
        exit 1
    fi

    UNTRACKED_RECONSTRUCTION_VERIFIED=true
    UNTRACKED_CAPTURE_MANIFEST_SHA256="$(sha256_hash "$UNTRACKED_CAPTURE_MANIFEST")"
    UNTRACKED_RECONSTRUCTED_MANIFEST_SHA256="$(sha256_hash "$RECONSTRUCTED_UNTRACKED_MANIFEST")"
}

# ── Compute source fingerprint ──────────────────────────────────────────────

SOURCE_FINGERPRINT_PY="$SCRIPT_DIR/source-fingerprint.py"
run_source_fingerprint() {
    # Run the fingerprint helper. Args: --root <dir> --manifest <path>.
    # Prints SHA-256 to stdout. Exits nonzero on failure.
    local root_dir="$1"
    local manifest_path="$2"
    local label="${3:-}"

    if ! command -v python3 >/dev/null 2>&1; then
        echo "error: python3 is required to compute the source fingerprint" >&2
        exit 1
    fi

    local fp
    if ! fp="$(
        python3 "$SOURCE_FINGERPRINT_PY" \
            --root "$root_dir" \
            --manifest "$manifest_path" \
            --reject-escaping-symlinks
    )"; then
        echo "error: source fingerprint failed${label:+ for $label}" >&2
        exit 1
    fi

    printf '%s' "$fp"
}

# ── Per-phase fingerprint check ──────────────────────────────────────────────
_check_phase_fingerprint() {
    # Quick fingerprint of a worktree and compare to expected value.
    # Exits with a diagnostic if the fingerprint has changed.
    local worktree="$1"
    local expected="$2"
    local label="$3"

    local tmp_manifest
    tmp_manifest="$(mktemp)"

    local current
    if ! current="$(
        python3 "$SOURCE_FINGERPRINT_PY" \
            --root "$worktree" \
            --manifest "$tmp_manifest" \
            --reject-escaping-symlinks
    )"; then
        rm -f "$tmp_manifest"
        echo "error: phase fingerprint failed for $label" >&2
        exit 1
    fi

    rm -f "$tmp_manifest"

    if [ "$current" != "$expected" ]; then
        echo "error: source snapshot changed during $label phase" >&2
        echo "  expected: $expected" >&2
        echo "  actual:   $current" >&2
        exit 1
    fi
}

compute_lockfile_hash() {
    local lockfile="$1"
    if [ -f "$lockfile" ]; then
        sha256_hash "$lockfile"
    else
        printf 'missing'
    fi
}

# ── Cargo helper ────────────────────────────────────────────────────────────

run_cargo_bench() {
    cargo bench --locked "$@" || {
        echo >&2
        echo "error: cargo bench --locked failed." >&2
        echo "Inspect the Cargo output above for the actual cause." >&2
        echo "Cargo.lock was not updated automatically." >&2
        return 1
    }
}

# ── Compile ─────────────────────────────────────────────────────────────────

compile_base() {
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
}

compile_current() {
    echo
    echo "Compiling current benchmark..."

    (
        cd "$CURRENT_WORKTREE"
        CARGO_TARGET_DIR="$CURRENT_TARGET" \
            run_cargo_bench \
            -p scah-regression-benches \
            --bench "$BENCH" \
            --no-run
    )
}

# ── Measure baseline ────────────────────────────────────────────────────────

measure_base() {
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
}

# ── Measure current ─────────────────────────────────────────────────────────

measure_current() {
    echo
    echo "Measuring current working tree..."

    # Copy only named saved-baseline measurements (not report/, new/, change/).
    mkdir -p "$CURRENT_TARGET"
    rm -rf "$CURRENT_TARGET/criterion"

    CRITERION_BASELINE_INVENTORY="$TEMP_ROOT/criterion-baseline-inventory.jsonl"

    echo "Copying saved-baseline measurements..."
    if ! python3 "$SCRIPT_DIR/copy-criterion-baseline.py" \
        --source "$BASE_TARGET/criterion" \
        --destination "$CURRENT_TARGET/criterion" \
        --baseline "$BASELINE_NAME" \
        --inventory "$CRITERION_BASELINE_INVENTORY"; then
        echo "error: failed to copy baseline measurements" >&2
        exit 1
    fi

    # ── Baseline integrity: record manifest BEFORE current run ────────────

    BASELINE_MANIFEST_BEFORE="$TEMP_ROOT/criterion-baseline-manifest-before.bin"
    BASELINE_MANIFEST_AFTER="$TEMP_ROOT/criterion-baseline-manifest-after.bin"
    BASELINE_MANIFEST_SHA256_BEFORE=""
    BASELINE_MANIFEST_SHA256_AFTER=""
    BASELINE_INTEGRITY_OK=false

    if [ -s "$CRITERION_BASELINE_INVENTORY" ]; then
        BASELINE_MANIFEST_SHA256_BEFORE="$(
            python3 "$SCRIPT_DIR/criterion-baseline-manifest.py" \
                --criterion-root "$CURRENT_TARGET/criterion" \
                --inventory "$CRITERION_BASELINE_INVENTORY" \
                --manifest "$BASELINE_MANIFEST_BEFORE" \
                --baseline "$BASELINE_NAME"
        )"
    fi

    # Ensure no stale output exists before measuring.
    if [ -d "$CURRENT_TARGET/criterion/report" ]; then
        echo "error: stale Criterion report already present in current target" >&2
        exit 1
    fi

    (
        cd "$CURRENT_WORKTREE"
        CARGO_TARGET_DIR="$CURRENT_TARGET" \
            run_cargo_bench \
            -p scah-regression-benches \
            --bench "$BENCH" \
            -- \
            --baseline "$BASELINE_NAME"
    )

    # Fresh-output validation: the current run must produce a report.
    if [ ! -d "$CURRENT_TARGET/criterion/report" ]; then
        echo "error: Criterion did not produce a fresh report" >&2
        exit 1
    fi

    # Validate that every expected benchmark produced nested new/ and change/ data.
    echo "Validating Criterion comparison output..."
    if ! python3 "$SCRIPT_DIR/validate-criterion-comparison.py" \
        --criterion-root "$CURRENT_TARGET/criterion" \
        --inventory "$CRITERION_BASELINE_INVENTORY"; then
        echo "error: Criterion comparison output is incomplete" >&2
        exit 1
    fi

    # ── Baseline integrity: verify manifest AFTER current run ──────────────

    if [ -s "$CRITERION_BASELINE_INVENTORY" ]; then
        BASELINE_MANIFEST_SHA256_AFTER="$(
            python3 "$SCRIPT_DIR/criterion-baseline-manifest.py" \
                --criterion-root "$CURRENT_TARGET/criterion" \
                --inventory "$CRITERION_BASELINE_INVENTORY" \
                --manifest "$BASELINE_MANIFEST_AFTER" \
                --baseline "$BASELINE_NAME"
        )"

        if [ "$BASELINE_MANIFEST_SHA256_BEFORE" != "$BASELINE_MANIFEST_SHA256_AFTER" ] \
            || ! cmp -s "$BASELINE_MANIFEST_BEFORE" "$BASELINE_MANIFEST_AFTER"; then
            echo "error: copied baseline measurements were modified during current run" >&2
            exit 1
        fi

        BASELINE_INTEGRITY_OK=true
        echo "  baseline measurements unchanged — integrity verified"
    fi
}

# ── Build metadata ──────────────────────────────────────────────────────────

build_metadata() {
    rm -rf "$REPORT_STAGING" "$REPORT_BACKUP"
    mkdir -p "$REPORT_STAGING"

    cp -a "$CURRENT_TARGET/criterion/." "$REPORT_STAGING/"

    RUSTC_VERSION="$(rustc --version 2>/dev/null || echo "unknown")"
    CARGO_VERSION="$(cargo --version 2>/dev/null || echo "unknown")"
    HOST_TRIPLE="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p' || echo "unknown")"

    cat > "$REPORT_STAGING/metadata.txt" <<EOF
base_ref=$BASE_REF
base_sha=$BASE_SHA
head_sha=$HEAD_SHA
base_source_fingerprint_before=$BASE_SOURCE_FINGERPRINT_BEFORE
base_source_fingerprint_after=$BASE_SOURCE_FINGERPRINT_AFTER
current_source_fingerprint_before=$CURRENT_SOURCE_FINGERPRINT_BEFORE
current_source_fingerprint_after=$CURRENT_SOURCE_FINGERPRINT_AFTER
source_snapshot_endpoint_fingerprints_match=true
base_lockfile_sha256_before=$BASE_LOCKFILE_SHA256_BEFORE
base_lockfile_sha256_after=$BASE_LOCKFILE_SHA256_AFTER
current_lockfile_sha256_before=$CURRENT_LOCKFILE_SHA256_BEFORE
current_lockfile_sha256_after=$CURRENT_LOCKFILE_SHA256_AFTER
lockfile_endpoint_hashes_match=true
head_dirty_files=$HEAD_DIRTY_FILES
head_dirty=$HEAD_DIRTY
working_tree_snapshot=true
live_repository_used_for_measurement=false
cargo_locked=true
benchmark=$BENCH
profile=$PROFILE
rustc=$RUSTC_VERSION
cargo=$CARGO_VERSION
host=$HOST_TRIPLE
benchmark_harness_diff_present=$HARNESS_DIFF_PRESENT
benchmark_harness_diff_allowed=$ALLOW_BENCH_HARNESS_DIFF
criterion_baseline_manifest_sha256_before=${BASELINE_MANIFEST_SHA256_BEFORE:-}
criterion_baseline_manifest_sha256_after=${BASELINE_MANIFEST_SHA256_AFTER:-}
criterion_baseline_measurement_manifests_match=${BASELINE_INTEGRITY_OK:-false}
criterion_baseline_measurements_unchanged=${BASELINE_INTEGRITY_OK:-false}
untracked_capture_reconstruction_verified=${UNTRACKED_RECONSTRUCTION_VERIFIED:-false}
untracked_capture_manifest_sha256=${UNTRACKED_CAPTURE_MANIFEST_SHA256:-}
untracked_reconstructed_manifest_sha256=${UNTRACKED_RECONSTRUCTED_MANIFEST_SHA256:-}
date_utc=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
EOF

    # Copy supporting artifacts into the report directory.
    printf '%s\n' "$INITIAL_WORKTREE_STATUS" \
        > "$REPORT_STAGING/working-tree-status.txt"

    if [ -f "$TRACKED_DIFF" ]; then
        cp "$TRACKED_DIFF" "$REPORT_STAGING/tracked-diff.patch"
    fi

    if [ -f "$UNTRACKED_LIST" ]; then
        cp "$UNTRACKED_LIST" "$REPORT_STAGING/untracked-files.txt"
    fi

    # Retain the untracked capture manifest (type and hash of each captured entry).
    if [ -f "$UNTRACKED_CAPTURE_MANIFEST" ]; then
        cp "$UNTRACKED_CAPTURE_MANIFEST" \
            "$REPORT_STAGING/untracked-capture-manifest.jsonl"
    fi

    # Retain the current source manifest and its hash.
    if [ -f "$CURRENT_MANIFEST" ]; then
        cp "$CURRENT_MANIFEST" "$REPORT_STAGING/current-source-manifest.bin"
    fi

    if [ -f "$CURRENT_MANIFEST_HASH_FILE" ]; then
        cp "$CURRENT_MANIFEST_HASH_FILE" \
            "$REPORT_STAGING/current-source-manifest.sha256"
    fi

    # Retain Criterion baseline inventory and verified measurement manifest.
    if [ -f "$CRITERION_BASELINE_INVENTORY" ]; then
        cp "$CRITERION_BASELINE_INVENTORY" \
            "$REPORT_STAGING/criterion-baseline-inventory.jsonl"
    fi

    if [ -f "$BASELINE_MANIFEST_BEFORE" ]; then
        cp "$BASELINE_MANIFEST_BEFORE" \
            "$REPORT_STAGING/criterion-baseline-manifest.bin"
        printf '%s\n' "$BASELINE_MANIFEST_SHA256_BEFORE" \
            > "$REPORT_STAGING/criterion-baseline-manifest.sha256"
    fi

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
}

# ── Validate staged report ──────────────────────────────────────────────────

validate_staged_report() {
    if [ ! -f "$REPORT_STAGING/report/index.html" ]; then
        echo "error: Criterion report index was not generated" >&2
        exit 1
    fi

    if [ ! -f "$REPORT_STAGING/metadata.txt" ]; then
        echo "error: benchmark metadata was not generated" >&2
        exit 1
    fi

    if [ -z "${CURRENT_SOURCE_FINGERPRINT_BEFORE:-}" ]; then
        echo "error: source fingerprint was not computed" >&2
        exit 1
    fi

    if [ ! -f "$REPORT_STAGING/criterion-baseline-inventory.jsonl" ]; then
        echo "error: Criterion baseline inventory was not published" >&2
        exit 1
    fi

    if [ ! -f "$REPORT_STAGING/criterion-baseline-manifest.bin" ]; then
        echo "error: Criterion baseline manifest was not published" >&2
        exit 1
    fi

    if [ ! -f "$REPORT_STAGING/criterion-baseline-manifest.sha256" ]; then
        echo "error: Criterion baseline manifest hash was not published" >&2
        exit 1
    fi

    local published_manifest_hash
    published_manifest_hash="$(tr -d '[:space:]' < "$REPORT_STAGING/criterion-baseline-manifest.sha256")"
    local actual_manifest_hash
    actual_manifest_hash="$(sha256_hash "$REPORT_STAGING/criterion-baseline-manifest.bin")"

    if [ "$published_manifest_hash" != "$actual_manifest_hash" ]; then
        echo "error: Criterion baseline manifest hash does not match published manifest" >&2
        exit 1
    fi

    if ! grep -q '^criterion_baseline_measurements_unchanged=true$' \
        "$REPORT_STAGING/metadata.txt"; then
        echo "error: Criterion baseline integrity metadata missing or false" >&2
        exit 1
    fi

    if ! grep -q '^criterion_baseline_measurement_manifests_match=true$' \
        "$REPORT_STAGING/metadata.txt"; then
        echo "error: Criterion baseline manifest match metadata missing or false" >&2
        exit 1
    fi
}

# ── Publish report ──────────────────────────────────────────────────────────

publish_report() {
    if [ -e "$REPORT_DIR" ]; then
        mv "$REPORT_DIR" "$REPORT_BACKUP"
    fi

    if mv "$REPORT_STAGING" "$REPORT_DIR"; then
        # Test hook: block BEFORE backup deletion so tests can verify
        # cleanup of stale backups after publication interruption.
        if [ -n "${SCAH_BENCH_TEST_BLOCK_AFTER_PUBLISH:-}" ]; then
            touch "${SCAH_BENCH_TEST_BLOCK_AFTER_PUBLISH}.started"
            while [ ! -e "${SCAH_BENCH_TEST_BLOCK_AFTER_PUBLISH}.released" ]; do
                sleep 0.05
            done
        fi
        rm -rf "$REPORT_BACKUP"
    else
        echo "error: failed to publish benchmark report" >&2

        if [ -e "$REPORT_BACKUP" ]; then
            mv "$REPORT_BACKUP" "$REPORT_DIR" || true
        fi

        exit 1
    fi
}

# ── Cleanup ─────────────────────────────────────────────────────────────────

REPORT_LOCK_HELD=0
cleanup_done=0

cleanup() {
    if [ "${cleanup_done:-0}" = "1" ]; then
        return
    fi

    cleanup_done=1

    # Remove temporary worktrees.
    if [ -n "${BASE_WORKTREE:-}" ]; then
        git -C "$ROOT" worktree remove --force "$BASE_WORKTREE" \
            >/dev/null 2>&1 || true
    fi

    if [ -n "${CURRENT_WORKTREE:-}" ]; then
        git -C "$ROOT" worktree remove --force "$CURRENT_WORKTREE" \
            >/dev/null 2>&1 || true
    fi

    # Remove temp root (includes base/current worktree dirs if removal failed).
    rm -rf "$TEMP_ROOT"

    # Always remove staging.
    if [ -n "${REPORT_STAGING:-}" ]; then
        rm -rf "$REPORT_STAGING"
    fi

    # Backup cleanup: if both backup and report exist, publication succeeded —
    # delete the stale backup. If only backup exists (no report), restore it.
    if [ -n "${REPORT_BACKUP:-}" ] && [ -e "$REPORT_BACKUP" ]; then
        if [ -n "${REPORT_DIR:-}" ] && [ -e "$REPORT_DIR" ]; then
            rm -rf "$REPORT_BACKUP"
        else
            mv "$REPORT_BACKUP" "$REPORT_DIR" >/dev/null 2>&1 || true
        fi
    fi

    # Release lock last, after all other cleanup.
    if [ "${REPORT_LOCK_HELD:-0}" = "1" ]; then
        rm -rf "$REPORT_LOCK"
        REPORT_LOCK_HELD=0
    fi
}

# ── Main ────────────────────────────────────────────────────────────────────

main() {
    resolve_revisions

    echo "Benchmark comparison"
    echo "  base ref:  $BASE_REF"
    echo "  base SHA:  $BASE_SHA"
    echo "  head SHA:  $HEAD_SHA"
    echo "  benchmark: $BENCH"
    echo "  profile:   $PROFILE"

    # Pre-flight: baseline must contain the regression package.
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

    # ── Temporary directories ────────────────────────────────────────────

    TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/scah-bench-compare.XXXXXX")"
    BASE_WORKTREE="$TEMP_ROOT/base"
    CURRENT_WORKTREE="$TEMP_ROOT/current"
    BASE_TARGET="$TEMP_ROOT/base-target"
    CURRENT_TARGET="$TEMP_ROOT/current-target"
    UNTRACKED_RECONSTRUCTION_VERIFIED=false

    REPORT_ROOT="$ROOT/target/bench-compare"
    REPORT_DIR="$REPORT_ROOT/latest"
    REPORT_STAGING="$REPORT_ROOT/.latest-staging-$$"
    REPORT_BACKUP="$REPORT_ROOT/.latest-backup-$$"

    # Install signal handlers and cleanup trap early — before any
    # temporary resource that can be left behind.
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    # Capture source state before any Cargo invocation.
    capture_source_state

    # Harness-integrity check.
    check_harness_integrity

    # Dirty working tree notice.
    if [ "$HEAD_DIRTY" = "true" ]; then
        echo "  working tree: dirty; current measurements include local changes"
    else
        echo "  working tree: clean"
    fi

    # ── Acquire benchmark lock ───────────────────────────────────────────

    echo
    echo "Acquiring benchmark lock..."

    if ! acquire_report_lock; then
        exit 1
    fi

    # ── Build worktrees ─────────────────────────────────────────────────

    create_base_worktree
    create_current_snapshot

    # ── Manifest paths ──────────────────────────────────────────────────

    BASE_MANIFEST="$TEMP_ROOT/base-source-manifest.bin"
    CURRENT_MANIFEST="$TEMP_ROOT/current-source-manifest.bin"
    CURRENT_MANIFEST_HASH_FILE="$TEMP_ROOT/current-source-manifest.sha256"

    # ── Fingerprint and hash BEFORE any Cargo invocation ─────────────────

    echo
    echo "Fingerprinting snapshots before Cargo..."

    BASE_SOURCE_FINGERPRINT_BEFORE="$(
        run_source_fingerprint \
            "$BASE_WORKTREE" "$BASE_MANIFEST" "baseline"
    )"
    echo "  baseline fingerprint: $BASE_SOURCE_FINGERPRINT_BEFORE"

    CURRENT_SOURCE_FINGERPRINT_BEFORE="$(
        run_source_fingerprint \
            "$CURRENT_WORKTREE" "$CURRENT_MANIFEST" "current"
    )"
    echo "  current fingerprint: $CURRENT_SOURCE_FINGERPRINT_BEFORE"

    # Write manifest hash file.
    printf '%s\n' "$CURRENT_SOURCE_FINGERPRINT_BEFORE" \
        > "$CURRENT_MANIFEST_HASH_FILE"

    BASE_LOCKFILE_SHA256_BEFORE="$(
        compute_lockfile_hash "$BASE_WORKTREE/Cargo.lock"
    )"
    CURRENT_LOCKFILE_SHA256_BEFORE="$(
        compute_lockfile_hash "$CURRENT_WORKTREE/Cargo.lock"
    )"
    echo "  baseline lockfile: $BASE_LOCKFILE_SHA256_BEFORE"
    echo "  current lockfile:  $CURRENT_LOCKFILE_SHA256_BEFORE"

    # ── Environment ──────────────────────────────────────────────────────

    export CARGO_INCREMENTAL=0
    export SCAH_BENCH_PROFILE="$PROFILE"

    # ── Compile ──────────────────────────────────────────────────────────

    compile_base
    compile_current

    # Quick per-phase check: fingerprints after compile.
    _check_phase_fingerprint "$BASE_WORKTREE" "$BASE_SOURCE_FINGERPRINT_BEFORE" "baseline (after compile)"
    _check_phase_fingerprint "$CURRENT_WORKTREE" "$CURRENT_SOURCE_FINGERPRINT_BEFORE" "current (after compile)"

    # ── Measure ──────────────────────────────────────────────────────────

    measure_base
    measure_current

    # Quick per-phase check: fingerprints after measurement.
    _check_phase_fingerprint "$BASE_WORKTREE" "$BASE_SOURCE_FINGERPRINT_BEFORE" "baseline (after measure)"
    _check_phase_fingerprint "$CURRENT_WORKTREE" "$CURRENT_SOURCE_FINGERPRINT_BEFORE" "current (after measure)"

    # ── Fingerprint and hash AFTER all Cargo commands ────────────────────

    echo
    echo "Verifying snapshot integrity after Cargo..."

    BASE_SOURCE_FINGERPRINT_AFTER="$(
        run_source_fingerprint \
            "$BASE_WORKTREE" "$BASE_MANIFEST" "baseline (after)"
    )"

    CURRENT_SOURCE_FINGERPRINT_AFTER="$(
        run_source_fingerprint \
            "$CURRENT_WORKTREE" "$CURRENT_MANIFEST" "current (after)"
    )"

    BASE_LOCKFILE_SHA256_AFTER="$(
        compute_lockfile_hash "$BASE_WORKTREE/Cargo.lock"
    )"
    CURRENT_LOCKFILE_SHA256_AFTER="$(
        compute_lockfile_hash "$CURRENT_WORKTREE/Cargo.lock"
    )"

    # ── Integrity checks ─────────────────────────────────────────────────

    local integrity_ok=1

    if [ "$BASE_SOURCE_FINGERPRINT_BEFORE" != "$BASE_SOURCE_FINGERPRINT_AFTER" ]; then
        echo "error: baseline source snapshot changed during benchmarking" >&2
        integrity_ok=0
    fi

    if [ "$CURRENT_SOURCE_FINGERPRINT_BEFORE" != "$CURRENT_SOURCE_FINGERPRINT_AFTER" ]; then
        echo "error: current source snapshot changed during benchmarking" >&2
        integrity_ok=0
    fi

    if [ "$BASE_LOCKFILE_SHA256_BEFORE" != "$BASE_LOCKFILE_SHA256_AFTER" ]; then
        echo "error: baseline Cargo.lock changed during benchmarking" >&2
        integrity_ok=0
    fi

    if [ "$CURRENT_LOCKFILE_SHA256_BEFORE" != "$CURRENT_LOCKFILE_SHA256_AFTER" ]; then
        echo "error: current Cargo.lock changed during benchmarking" >&2
        integrity_ok=0
    fi

    if [ "$integrity_ok" = 0 ]; then
        echo "error: snapshot integrity verification failed; no report published" >&2
        exit 1
    fi

    echo "  snapshots unchanged — integrity verified"

    # ── Build and publish report ─────────────────────────────────────────

    build_metadata
    validate_staged_report
    publish_report

    echo
    echo "Benchmark comparison complete."
    echo "Criterion report:"
    echo "  $REPORT_DIR/report/index.html"
    echo "Metadata:"
    echo "  $REPORT_DIR/metadata.txt"
    echo "Source fingerprint: $CURRENT_SOURCE_FINGERPRINT_BEFORE"
}

main "$@"
