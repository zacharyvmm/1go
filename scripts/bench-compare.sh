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
    # Working-tree status for dirty-file count and diagnostics.
    INITIAL_WORKTREE_STATUS="$(
        git -C "$ROOT" status --porcelain=v1 --untracked-files=all
    )"

    # Tracked changes (staged + unstaged) relative to HEAD.
    # git diff --binary HEAD captures the combined working-tree diff
    # including additions, deletions, renames, binary files, and mode changes.
    TRACKED_DIFF="$TEMP_ROOT/tracked-diff.patch"
    git -C "$ROOT" diff --binary HEAD > "$TRACKED_DIFF"

    # Untracked, non-ignored files (NUL-delimited).
    UNTRACKED_LIST="$TEMP_ROOT/untracked-files.txt"
    git -C "$ROOT" ls-files --others --exclude-standard -z \
        > "$UNTRACKED_LIST"

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
    mkdir -p "$REPORT_ROOT"

    if ! mkdir "$REPORT_LOCK" 2>/dev/null; then
        echo "error: another benchmark comparison is already running" >&2

        if [ -f "$REPORT_LOCK/pid" ]; then
            local lock_pid
            lock_pid="$(cat "$REPORT_LOCK/pid" 2>/dev/null || true)"

            if [ -n "$lock_pid" ]; then
                echo "  lock owner PID: $lock_pid" >&2
            fi
        fi

        echo "  lock path: $REPORT_LOCK" >&2
        echo >&2
        echo "If no comparison is running, remove the stale lock directory manually." >&2
        return 1
    fi

    REPORT_LOCK_HELD=1

    printf '%s\n' "$$" > "$REPORT_LOCK/pid"
    printf '%s\n' "$HEAD_SHA" > "$REPORT_LOCK/head-sha"
    printf '%s\n' "$(date -u +"%Y-%m-%dT%H:%M:%SZ")" \
        > "$REPORT_LOCK/started-at"
    hostname > "$REPORT_LOCK/hostname" 2>/dev/null || true
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
    echo "Creating immutable current-tree snapshot..."

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

    # Copy untracked, non-ignored files into the snapshot.
    if [ -s "$UNTRACKED_LIST" ]; then
        while IFS= read -r -d '' file; do
            if [ -z "$file" ]; then
                continue
            fi
            local src="$ROOT/$file"
            local dst="$CURRENT_WORKTREE/$file"
            mkdir -p "$(dirname "$dst")"

            if [ -L "$src" ]; then
                # Preserve symlink.
                local target
                target="$(readlink "$src")"
                ln -s "$target" "$dst"
            else
                cp -p "$src" "$dst"
            fi
        done < "$UNTRACKED_LIST"
    fi
}

# ── Compute source fingerprint ──────────────────────────────────────────────

compute_source_fingerprint() {
    # Generate a sorted manifest of every file in the current snapshot
    # (excluding .git) and hash the manifest. Uses Python for portability
    # across Linux and macOS. Python is already required by the test suite.

    local manifest="$TEMP_ROOT/source-manifest.txt"
    local manifest_hash_file="$TEMP_ROOT/source-manifest.sha256"

    if ! command -v python3 >/dev/null 2>&1; then
        echo "error: python3 is required to compute the source fingerprint" >&2
        exit 1
    fi

    python3 -c "
import os, sys, hashlib

root = sys.argv[1]
manifest_path = sys.argv[2]

entries = []
for dirpath, dirnames, filenames in os.walk(root):
    if '.git' in dirnames:
        dirnames.remove('.git')
    for fn in filenames:
        full = os.path.join(dirpath, fn)
        rel = os.path.relpath(full, root)
        try:
            st = os.lstat(full)
            mode = oct(st.st_mode)[-3:]
            with open(full, 'rb') as f:
                h = hashlib.sha256(f.read()).hexdigest()
            entries.append((rel, mode, h))
        except OSError:
            # Skip unreadable files.
            pass

entries.sort(key=lambda x: x[0])

with open(manifest_path, 'w') as mf:
    for rel, mode, h in entries:
        mf.write(f'{mode}\t{h}\t{rel}\n')
" "$CURRENT_WORKTREE" "$manifest"

    CURRENT_SOURCE_FINGERPRINT="$(sha256_hash "$manifest")"
    echo "  source fingerprint: $CURRENT_SOURCE_FINGERPRINT"
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

    # Copy Criterion baseline data so the current run can compare against it.
    mkdir -p "$CURRENT_TARGET"
    rm -rf "$CURRENT_TARGET/criterion"
    cp -a "$BASE_TARGET/criterion" "$CURRENT_TARGET/criterion"

    (
        cd "$CURRENT_WORKTREE"
        CARGO_TARGET_DIR="$CURRENT_TARGET" \
            run_cargo_bench \
            -p scah-regression-benches \
            --bench "$BENCH" \
            -- \
            --baseline "$BASELINE_NAME"
    )
}

# ── Build metadata ──────────────────────────────────────────────────────────

build_metadata() {
    rm -rf "$REPORT_STAGING" "$REPORT_BACKUP"
    mkdir -p "$REPORT_STAGING"

    cp -a "$CURRENT_TARGET/criterion/." "$REPORT_STAGING/"

    # Lockfile hashes (compute before and after snapshot is valid).
    if [ -f "$BASE_WORKTREE/Cargo.lock" ]; then
        BASE_LOCKFILE_SHA256="$(sha256_hash "$BASE_WORKTREE/Cargo.lock")"
    else
        BASE_LOCKFILE_SHA256="missing"
    fi

    if [ -f "$CURRENT_WORKTREE/Cargo.lock" ]; then
        CURRENT_LOCKFILE_SHA256="$(sha256_hash "$CURRENT_WORKTREE/Cargo.lock")"
    else
        CURRENT_LOCKFILE_SHA256="missing"
    fi

    RUSTC_VERSION="$(rustc --version 2>/dev/null || echo "unknown")"
    CARGO_VERSION="$(cargo --version 2>/dev/null || echo "unknown")"
    HOST_TRIPLE="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p' || echo "unknown")"

    cat > "$REPORT_STAGING/metadata.txt" <<EOF
base_ref=$BASE_REF
base_sha=$BASE_SHA
head_sha=$HEAD_SHA
current_source_fingerprint=$CURRENT_SOURCE_FINGERPRINT
base_lockfile_sha256=$BASE_LOCKFILE_SHA256
current_lockfile_sha256=$CURRENT_LOCKFILE_SHA256
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

    if [ -f "$TEMP_ROOT/source-manifest.txt" ]; then
        cp "$TEMP_ROOT/source-manifest.txt" "$REPORT_STAGING/source-manifest.txt"
    fi

    if [ -f "$TEMP_ROOT/source-manifest.sha256" ]; then
        cp "$TEMP_ROOT/source-manifest.sha256" \
            "$REPORT_STAGING/source-manifest.sha256"
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

    if [ -z "${CURRENT_SOURCE_FINGERPRINT:-}" ]; then
        echo "error: source fingerprint was not computed" >&2
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

    REPORT_ROOT="$ROOT/target/bench-compare"
    REPORT_DIR="$REPORT_ROOT/latest"
    REPORT_STAGING="$REPORT_ROOT/.latest-staging-$$"
    REPORT_BACKUP="$REPORT_ROOT/.latest-backup-$$"
    REPORT_LOCK="$REPORT_ROOT/.bench-compare-lock"

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

    # ── Build immutable worktrees ────────────────────────────────────────

    create_base_worktree
    create_current_snapshot

    # Compute source fingerprint (before compilation, after snapshot is ready).
    compute_source_fingerprint

    # ── Environment ──────────────────────────────────────────────────────

    export CARGO_INCREMENTAL=0
    export SCAH_BENCH_PROFILE="$PROFILE"

    # ── Compile ──────────────────────────────────────────────────────────

    compile_base
    compile_current

    # ── Measure ──────────────────────────────────────────────────────────

    measure_base
    measure_current

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
    echo "Source fingerprint: $CURRENT_SOURCE_FINGERPRINT"
}

main "$@"
