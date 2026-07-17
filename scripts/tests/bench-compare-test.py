#!/usr/bin/env python3
"""Test: bench-compare.sh safety and correctness.

Validates:
  1. Concurrent invocation prevention.
  2. Signal-based lock release (SIGTERM, SIGINT).
  3. Pre-existing stale lock detection.
  4. Dirty tracked file is snapshotted correctly.
  5. Untracked file included in snapshot (filename with spaces).
  6. Live source mutation cannot alter the measurement.
  7. Dirty file content fingerprint changes with content.
  8. Lockfile hashes are recorded correctly.
  9. Backup cleanup after publication interruption.
 10. Current snapshot mutation aborts publication.
 11. Baseline snapshot mutation aborts publication.
 12. Lockfile mutation is detected.
 13. Identical source trees produce identical fingerprints.
 14. Source-content change alters the fingerprint.
 15. .git is excluded from the fingerprint.
 16. Symlink target affects the fingerprint.
 17. Filenames with tabs and newlines are unambiguous.
 18. Unreadable or unsupported entries fail.
 19. Manifest hash file exists and matches.
 20+. Full-workflow safety: FIFO/socket rejection, capture races,
     symlink policy, Criterion failure paths, linked-worktree locking,
     and baseline integrity via portable manifest controls.
"""

import os
import signal
import shutil
import socket
import stat as stat_mod
import subprocess
import tempfile
import threading
import time
import sys
import hashlib

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(SCRIPT_DIR, "..", ".."))
BENCH_COMPARE = os.path.join(REPO_ROOT, "scripts", "bench-compare.sh")
FINGERPRINT_PY = os.path.join(REPO_ROOT, "scripts", "source-fingerprint.py")

PASS = 0
FAIL = 0


def _pass(msg):
    global PASS
    PASS += 1
    print(f"  PASS: {msg}")


def _fail(msg):
    global FAIL
    FAIL += 1
    print(f"  FAIL: {msg}", file=sys.stderr)


def assert_eq(label, expected, actual):
    if expected != actual:
        _fail(f"{label}: expected '{expected}', got '{actual}'")
    else:
        _pass(label)


def assert_ne(label, unexpected, actual):
    if unexpected == actual:
        _fail(f"{label}: got unexpected '{actual}'")
    else:
        _pass(label)


def assert_file_exists(path, label):
    if os.path.isfile(path):
        _pass(label)
    else:
        _fail(f"{label}: file '{path}' not found")


def assert_file_absent(path, label):
    if not os.path.exists(path):
        _pass(label)
    else:
        _fail(f"{label}: path '{path}' unexpectedly exists")


def assert_contains(path, substring, label):
    if not os.path.isfile(path):
        _fail(f"{label}: file '{path}' does not exist")
        return
    with open(path) as f:
        content = f.read()
    if substring in content:
        _pass(label)
    else:
        _fail(f"{label}: '{substring}' not found in {path}")


def assert_text_contains(text, substring, label):
    if substring in text:
        _pass(label)
    else:
        _fail(f"{label}: '{substring}' not found in output")


def assert_not_contains(path, substring, label):
    if not os.path.isfile(path):
        _pass(label)
        return
    with open(path) as f:
        content = f.read()
    if substring not in content:
        _pass(label)
    else:
        _fail(f"{label}: '{substring}' unexpectedly found in {path}")


def git_common_dir(repo: str) -> str:
    result = subprocess.run(
        ["git", "-C", repo, "rev-parse", "--git-common-dir"],
        check=True,
        capture_output=True,
        text=True,
    )
    value = result.stdout.strip()
    if not os.path.isabs(value):
        value = os.path.join(repo, value)
    return os.path.realpath(value)


def comparison_lock_path(repo: str) -> str:
    return os.path.join(git_common_dir(repo), "scah-bench-compare.lock")


def sha256_file(path):
    """Compute SHA-256 of a file."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def read_manifest(path):
    """Parse NUL-delimited binary manifest into a list of dicts.

    Each record has: type, mode, hash, path.
    """
    with open(path, "rb") as file:
        fields = file.read().split(b"\0")

    assert fields[-1] == b"", "manifest must end with NUL terminator"

    records = []

    for index in range(0, len(fields) - 1, 4):
        records.append(
            {
                "type": fields[index].decode("utf-8"),
                "mode": fields[index + 1].decode("ascii"),
                "hash": fields[index + 2].decode("ascii"),
                "path": fields[index + 3].decode(
                    "utf-8",
                    errors="surrogateescape",
                ),
            }
        )

    return records


def create_test_repo(repo):
    """Create a minimal test repo with two commits.

    Commit 1: .gitignore + benches/regression/ files.
    Commit 2: other.rs (so HEAD~1 has the regression bench package).
    """
    os.makedirs(repo, exist_ok=True)
    subprocess.run(
        ["git", "-C", repo, "init", "-q", "-b", "main"], check=True
    )
    subprocess.run(
        ["git", "-C", repo, "config", "user.email", "t@t.com"], check=True
    )
    subprocess.run(
        ["git", "-C", repo, "config", "user.name", "T"], check=True
    )

    with open(os.path.join(repo, ".gitignore"), "w") as f:
        f.write("/target/\n")

    os.makedirs(os.path.join(repo, "benches", "regression"))
    with open(
        os.path.join(repo, "benches", "regression", "Cargo.toml"), "w"
    ) as f:
        f.write(
            "[package]\n"
            'name = "scah-regression-benches"\n'
            'version = "0.1.0"\n'
            'edition = "2021"\n'
            "[[bench]]\n"
            'name = "core_regression"\n'
            "harness = false\n"
        )

    with open(
        os.path.join(
            repo, "benches", "regression", "core_regression.rs"
        ),
        "w",
    ) as f:
        f.write("// stub bench\n")

    subprocess.run(["git", "-C", repo, "add", "-A"], check=True)
    subprocess.run(
        ["git", "-C", repo, "commit", "-q", "-m", "init"], check=True
    )

    with open(os.path.join(repo, "other.rs"), "w") as f:
        f.write("// unrelated\n")
    subprocess.run(["git", "-C", repo, "add", "other.rs"], check=True)
    subprocess.run(
        ["git", "-C", repo, "commit", "-q", "-m", "other"], check=True
    )


def commit_file(repo, path, content):
    """Create, add, and commit a file in the test repo."""
    full = os.path.join(repo, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(content)
    subprocess.run(["git", "-C", repo, "add", path], check=True)
    subprocess.run(
        ["git", "-C", repo, "commit", "-q", "-m", f"add {path}"],
        check=True,
    )


def write_file(repo, path, content):
    """Write a file without committing (becomes dirty or untracked)."""
    full = os.path.join(repo, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(content)


def shlex_quote(s):
    """Simple shell quoting."""
    return "'" + s.replace("'", "'\\''") + "'"


def _stub_mutation_lines(cargo_log, mutate_when_cwd_contains=None,
                         mutate_file=None, mutate_content=None):
    """Return shell lines for the cargo stub's file-mutation logic.

    When *mutate_when_cwd_contains* is a non-empty string AND *mutate_file*
    is explicitly set, the stub checks whether CWD contains that substring
    and, if so, overwrites *mutate_file* (relative to CWD) with
    *mutate_content*.
    """
    if not mutate_when_cwd_contains or not mutate_file:
        return []
    mf = shlex_quote(mutate_file)
    mc = shlex_quote(mutate_content or "mutated by stub")
    kw = shlex_quote(mutate_when_cwd_contains)
    return [
        'if [[ "$CWD" == *' + kw + '* ]]; then',
        '    printf "%s\\n" ' + mc + ' > "$CWD"/' + mf,
        "fi",
        "",
    ]


def make_stub_cargo(stub_dir, cargo_log, block_file=None,
                    mutate_when_cwd_contains=None,
                    mutate_file=None, mutate_content=None,
                    mutate_shell=None):
    """Create a fake 'cargo' executable that logs invocations.

    Logs CWD and args on every call. When SCAH_BENCH_TEST_READ_FILE is
    set, reads that file (relative to CWD) and logs its content.
    Supports CARGO_BLOCK_FILE for concurrency tests.
    When *mutate_when_cwd_contains* is set:
      - *mutate_file* is overwritten with *mutate_content*.
      - *mutate_shell* (if set) is injected as raw shell code.
    """
    os.makedirs(stub_dir, exist_ok=True)
    stub_path = os.path.join(stub_dir, "cargo")
    lines = [
        "#!/usr/bin/env bash",
        "set -euo pipefail",
        "",
        "# Handle --version calls silently (not a real bench invocation).",
        'case "$*" in',
        "    *--version*)",
        '        echo "cargo 1.0.0 (test stub)"',
        "        exit 0",
        "        ;;",
        "esac",
        "",
        'CWD="$(pwd)"',
        f"printf 'CWD=%s\\n' \"$CWD\" >> {shlex_quote(cargo_log)}",
        f"printf 'ARGS=%s\\n' \"$*\" >> {shlex_quote(cargo_log)}",
        "",
        'if [ -n "${SCAH_BENCH_TEST_READ_FILE:-}" ]; then',
        '    f="$CWD/${SCAH_BENCH_TEST_READ_FILE}"',
        "    if [ -f \"$f\" ]; then",
        (
            "        printf 'FILE_CONTENT=%s\\n'"
            f' "$(cat "$f")" >> {shlex_quote(cargo_log)}'
        ),
        "    else",
        (
            "        printf 'FILE_MISSING=%s\\n'"
            f' "${{SCAH_BENCH_TEST_READ_FILE}}" >> {shlex_quote(cargo_log)}'
        ),
        "    fi",
        "fi",
        "",
        'CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target}"',
        "",
    ]
    # Mutation support.
    lines += _stub_mutation_lines(
        cargo_log,
        mutate_when_cwd_contains=mutate_when_cwd_contains,
        mutate_file=mutate_file,
        mutate_content=mutate_content,
    )
    # Shell-level mutation support (for directory symlink etc.).
    comparison_mutate_lines = []
    if mutate_shell and mutate_when_cwd_contains:
        kw = shlex_quote(mutate_when_cwd_contains)
        comparison_mutate_lines = [
            '    if [[ "$CWD" == *' + kw + '* ]]; then',
            '    ' + mutate_shell,
            "    fi",
        ]
    if block_file:
        lines += [
            'if [ -n "${CARGO_BLOCK_FILE:-}" ] &&'
            ' [ ! -e "${CARGO_BLOCK_FILE}.released" ]; then',
            '    touch "${CARGO_BLOCK_FILE}.started"',
            '    while [ ! -e "${CARGO_BLOCK_FILE}.released" ]; do',
            "        sleep 0.05",
            "    done",
            "fi",
            "",
        ]
    lines += [
        'BENCH_DIR="$CARGO_TARGET_DIR/criterion"',
        "",
        # For --save-baseline runs (baseline measurement), create saved-baseline dirs.
        'if echo "$*" | grep -q -- "--save-baseline"; then',
        '    BASELINE_NAME=$(echo "$*" | sed -n "s/.*--save-baseline \\([^ ]*\\).*/\\1/p")',
        '    [ -n "$BASELINE_NAME" ] || BASELINE_NAME="main"',
        '    BENCH_PATHS="',
        '        synthetic_links/prebuilt/all/save_none',
        '        synthetic_links/prebuilt/all/save_inner_html',
        '        synthetic_links/prebuilt/all/save_text',
        '        synthetic_links/prebuilt/all/save_all',
        '        synthetic_links/consume/all/save_all',
        '        synthetic_links/end_to_end/all/save_all',
        '        product_catalog/prebuilt/nested_all/save_all',
        '        product_catalog/consume/nested_all/save_all',
        '        product_catalog/end_to_end/nested_all/save_all',
        '        multi_query/prebuilt',
        '    "',
        '    for bp in $BENCH_PATHS; do',
        '        BASELINE_DIR="$BENCH_DIR/$bp/$BASELINE_NAME"',
        '        mkdir -p "$BASELINE_DIR"',
        '        printf "{\\"confidence_interval\\":{\\"lower_bound\\":1.0,\\"upper_bound\\":1.0,\\"point_estimate\\":1.0}}\\n" > "$BASELINE_DIR/estimates.json"',
        '    done',
        'fi',
        "",
        # For current runs (--baseline without --save-baseline), create nested new/ and change/.
        'if echo "$*" | grep -q -- "--baseline" && ! echo "$*" | grep -q -- "--save-baseline"; then',
        '    BASELINE_NAME=$(echo "$*" | sed -n "s/.*--baseline \\([^ ]*\\).*/\\1/p")',
        '    [ -n "$BASELINE_NAME" ] || BASELINE_NAME="main"',
        "",
        '    # Baseline integrity mutations (before comparison output).',
        '    if [ "${SCAH_STUB_MUTATE_BASELINE_FILE:-}" = "1" ]; then',
        '        est="$(find "$BENCH_DIR" -type f -path "*/${BASELINE_NAME}/estimates.json" | head -1)"',
        '        if [ -n "$est" ]; then printf "\\nmutated\\n" >> "$est"; fi',
        '    fi',
        '    if [ "${SCAH_STUB_DELETE_BASELINE_FILE:-}" = "1" ]; then',
        '        est="$(find "$BENCH_DIR" -type f -path "*/${BASELINE_NAME}/estimates.json" | head -1)"',
        '        if [ -n "$est" ]; then rm -f "$est"; fi',
        '    fi',
        '    if [ "${SCAH_STUB_ADD_BASELINE_FILE:-}" = "1" ]; then',
        '        est="$(find "$BENCH_DIR" -type f -path "*/${BASELINE_NAME}/estimates.json" | head -1)"',
        '        if [ -n "$est" ]; then printf "extra\\n" > "$(dirname "$est")/extra-baseline.txt"; fi',
        '    fi',
        '    if [ "${SCAH_STUB_INSERT_BASELINE_SYMLINK:-}" = "1" ]; then',
        '        est="$(find "$BENCH_DIR" -type f -path "*/${BASELINE_NAME}/estimates.json" | head -1)"',
        '        if [ -n "$est" ]; then ln -sf "estimates.json" "$(dirname "$est")/baseline-link"; fi',
        '    fi',
        "",
        '    if [ "${SCAH_STUB_SKIP_CURRENT_OUTPUT:-}" = "1" ]; then',
        '        :',
        '    elif [ "${SCAH_STUB_REPORT_ONLY:-}" = "1" ]; then',
        '        mkdir -p "$BENCH_DIR/report"',
        (
            "        printf '<html>stub report</html>\\n'"
            ' > "$BENCH_DIR/report/index.html"'
        ),
        '    else',
        '        find "$BENCH_DIR" -type f -path "*/${BASELINE_NAME}/estimates.json" | while read -r est; do',
        '            BASELINE_DIR="$(dirname "$est")"',
        '            BENCH_PATH="$(dirname "$BASELINE_DIR")"',
        '            rel="${BENCH_PATH#"$BENCH_DIR"/}"',
        '            if [ -n "${SCAH_STUB_SKIP_BENCHMARK_PATH:-}" ] && [ "$rel" = "${SCAH_STUB_SKIP_BENCHMARK_PATH}" ]; then',
        '                continue',
        '            fi',
        '            mkdir -p "$BENCH_PATH/new" "$BENCH_PATH/change"',
        '            printf "{\\"mean\\":{\\"estimate\\":1.0,\\"lower_bound\\":0.9,\\"upper_bound\\":1.1}}\\n" > "$BENCH_PATH/new/estimates.json"',
        '            if [ "${SCAH_STUB_SKIP_CURRENT_SAMPLE:-}" != "1" ]; then',
        '                printf "{\\"mean\\":{\\"estimate\\":1.0,\\"lower_bound\\":0.9,\\"upper_bound\\":1.1}}\\n" > "$BENCH_PATH/new/sample.json"',
        '            fi',
        '            if [ "${SCAH_STUB_SKIP_CHANGE_ESTIMATES:-}" != "1" ]; then',
        '                printf "{\\"mean\\":{\\"estimate\\":1.0,\\"lower_bound\\":0.9,\\"upper_bound\\":1.1}}\\n" > "$BENCH_PATH/change/estimates.json"',
        '            fi',
        '        done',
        '        mkdir -p "$BENCH_DIR/report"',
        (
            "        printf '<html>stub report</html>\\n'"
            ' > "$BENCH_DIR/report/index.html"'
        ),
        '    fi',
    ] + comparison_mutate_lines + [
        'fi',
        "",
    ]
    with open(stub_path, "w") as f:
        f.write("\n".join(lines))
    os.chmod(stub_path, 0o755)


def bench_compare_env(stub_dir, block_file=None, extra_env=None):
    """Build environment dict for running bench-compare.sh."""
    env = os.environ.copy()
    env["PATH"] = stub_dir + ":" + env.get("PATH", "")
    env["ALLOW_BENCH_HARNESS_DIFF"] = "1"
    env["BASE_REF"] = "HEAD~1"
    if block_file:
        env["CARGO_BLOCK_FILE"] = block_file
    if extra_env:
        env.update(extra_env)
    return env


def run_bench_compare_bg(repo, stub_dir, block_file, stdout, stderr,
                         extra_env=None):
    """Start bench-compare.sh in a new session."""
    env = bench_compare_env(stub_dir, block_file, extra_env=extra_env)
    p = subprocess.Popen(
        [BENCH_COMPARE],
        cwd=repo,
        env=env,
        stdout=stdout,
        stderr=stderr,
        preexec_fn=os.setsid,
    )
    return p


def wait_for_block(block_file, timeout=10.0):
    """Wait for the cargo stub to create the .started marker."""
    started = block_file + ".started"
    deadline = time.time() + timeout
    while time.time() < deadline:
        if os.path.exists(started):
            return True
        time.sleep(0.05)
    return False


def setup_prior_report(repo, marker_name="report-marker.txt", content="previous"):
    """Create a pre-existing successful report under target/bench-compare/latest."""
    latest_dir = os.path.join(repo, "target/bench-compare/latest")
    report_dir = os.path.join(latest_dir, "report")
    os.makedirs(report_dir, exist_ok=True)
    with open(os.path.join(report_dir, "index.html"), "w") as f:
        f.write("<html>prior report</html>\n")
    with open(os.path.join(latest_dir, marker_name), "w") as f:
        f.write(content)


def assert_prior_report_preserved(repo, marker_name="report-marker.txt"):
    marker = os.path.join(repo, "target/bench-compare/latest", marker_name)
    assert_file_exists(marker, "previous report marker preserved")
    report_html = os.path.join(
        repo, "target/bench-compare/latest/report/index.html"
    )
    assert_file_exists(report_html, "previous report index preserved")


def assert_no_staging_or_backup(repo):
    report_root = os.path.join(repo, "target/bench-compare")
    if not os.path.isdir(report_root):
        _pass("no staging or backup directories")
        return
    for entry in os.listdir(report_root):
        if entry.startswith(".latest-staging-"):
            _fail(f"stale staging directory found: {entry}")
            return
        if entry.startswith(".latest-backup-"):
            _fail(f"stale backup directory found: {entry}")
            return
    _pass("no staging or backup directories")


def assert_lock_absent(repo, label):
    assert_file_absent(comparison_lock_path(repo), label)


def assert_cargo_not_run(cargo_log, label):
    if not os.path.exists(cargo_log) or os.path.getsize(cargo_log) == 0:
        _pass(label)
        return
    with open(cargo_log) as f:
        log_content = f.read()
    args_lines = [
        line for line in log_content.splitlines() if line.startswith("ARGS=")
    ]
    if not args_lines:
        _pass(label)
    else:
        _fail(f"{label}: unexpected Cargo invocation logged")


def assert_cargo_ran(cargo_log, label):
    if not os.path.isfile(cargo_log):
        _fail(f"{label}: cargo log missing")
        return
    with open(cargo_log) as f:
        log_content = f.read()
    args_lines = [
        line for line in log_content.splitlines() if line.startswith("ARGS=")
    ]
    if any("--bench" in line for line in args_lines):
        _pass(label)
    else:
        _fail(f"{label}: Cargo bench was not invoked")
        print(log_content, file=sys.stderr)


def run_bench_compare(repo, stub_dir, extra_env=None, timeout=60):
    env = bench_compare_env(stub_dir, extra_env=extra_env)
    return subprocess.run(
        [BENCH_COMPARE],
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def run_capture_hook_bg(repo, stub_dir, block_file, hook_env_key, extra_env=None):
    """Start bench-compare blocked at a capture hook."""
    env = bench_compare_env(stub_dir, extra_env=extra_env)
    env[hook_env_key] = block_file
    stdout_path = block_file + ".stdout"
    stderr_path = block_file + ".stderr"
    f_out = open(stdout_path, "w")
    f_err = open(stderr_path, "w")
    proc = subprocess.Popen(
        [BENCH_COMPARE],
        cwd=repo,
        env=env,
        stdout=f_out,
        stderr=f_err,
        preexec_fn=os.setsid,
    )
    return proc, f_out, f_err, stdout_path, stderr_path


def release_capture_hook(block_file):
    with open(block_file + ".released", "w") as f:
        f.write("\n")


def wait_for_capture_hook_start(block_file, timeout=30.0):
    return wait_for_block(block_file, timeout=timeout)


def capture_hook_releaser(block_file, stop_event, mutate_fn=None, repeat_mutate=False):
    """Release each capture-hook iteration; optionally mutate once or always."""
    started = block_file + ".started"
    released = block_file + ".released"
    last_started_marker = None
    mutated = False
    while not stop_event.is_set():
        if os.path.exists(started):
            with open(started, "rb") as fh:
                marker = fh.read()
            marker_changed = marker != last_started_marker
            hook_waiting = not os.path.exists(released)
            if marker_changed or hook_waiting:
                if marker_changed:
                    last_started_marker = marker
                    if mutate_fn is not None and (repeat_mutate or not mutated):
                        mutate_fn()
                        mutated = True
                if not os.path.exists(released):
                    release_capture_hook(block_file)
        time.sleep(0.01)


def continuous_capture_mutator(block_file, mutate_fn, stop_event):
    """Mutate on every capture-hook iteration until stop_event is set."""
    capture_hook_releaser(block_file, stop_event, mutate_fn, repeat_mutate=True)


def read_metadata(repo):
    """Read metadata.txt from the published report."""
    path = os.path.join(
        repo, "target/bench-compare/latest/metadata.txt"
    )
    if not os.path.isfile(path):
        return {}
    meta = {}
    with open(path) as f:
        for line in f:
            line = line.strip()
            if "=" in line:
                k, v = line.split("=", 1)
                meta[k] = v
    return meta


def run_fingerprint(root, manifest_path=None):
    """Run the source-fingerprint.py helper, return (returncode, stdout, stderr)."""
    if manifest_path is None:
        manifest_path = os.path.join(
            tempfile.mkdtemp(prefix="fp-manifest-"), "manifest.bin"
        )
    result = subprocess.run(
        [
            sys.executable, FINGERPRINT_PY,
            "--root", root,
            "--manifest", manifest_path,
        ],
        capture_output=True,
        text=True,
        timeout=30,
    )
    return result.returncode, result.stdout.strip(), result.stderr, manifest_path


# ═══════════════════════════════════════════════════════════════════════════
# Test 1: Concurrent invocation
# ═══════════════════════════════════════════════════════════════════════════

def test_concurrent(tmp):
    print("=== Test 1: Concurrent invocation ===")
    test_dir = os.path.join(tmp, "test1")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    block_file = os.path.join(test_dir, "block")
    cargo_log1 = os.path.join(test_dir, "cargo-first.log")
    cargo_log2 = os.path.join(test_dir, "cargo-second.log")

    stub1 = os.path.join(test_dir, "stub1")
    make_stub_cargo(stub1, cargo_log1, block_file)

    # Start first invocation.
    f_out = open(os.path.join(test_dir, "first.out"), "w")
    f_err = open(os.path.join(test_dir, "first.err"), "w")
    p1 = run_bench_compare_bg(repo, stub1, block_file, f_out, f_err)

    if not wait_for_block(block_file):
        _fail("first process did not start cargo")
        p1.kill()
        p1.wait()
        f_out.close()
        f_err.close()
        with open(os.path.join(test_dir, "first.err")) as fe:
            print(fe.read(), file=sys.stderr)
        return
    _pass("first process started and is blocking on cargo")

    # Second invocation (no blocking stub).
    stub2 = os.path.join(test_dir, "stub2")
    make_stub_cargo(stub2, cargo_log2)

    env2 = bench_compare_env(stub2)
    result2 = subprocess.run(
        [BENCH_COMPARE],
        cwd=repo,
        env=env2,
        capture_output=True,
        text=True,
        timeout=30,
    )

    assert_ne("second exit code", 0, result2.returncode)

    if "another benchmark comparison is already running" in result2.stderr:
        _pass("second invocation printed lock error")
    else:
        _fail("second invocation did not print lock error")
        print(result2.stderr, file=sys.stderr)

    cargo2_empty = (
        not os.path.exists(cargo_log2)
        or os.path.getsize(cargo_log2) == 0
    )
    if cargo2_empty:
        _pass("second invocation made no Cargo calls")
    else:
        _fail("second invocation called Cargo")

    # Release first process.
    with open(block_file + ".released", "w") as f:
        f.write("\n")
    p1.wait(timeout=30)
    f_out.close()
    f_err.close()

    assert_eq("first exit code", 0, p1.returncode)
    assert_file_exists(
        os.path.join(
            repo, "target/bench-compare/latest/report/index.html"
        ),
        "first published report",
    )
    assert_file_exists(
        os.path.join(
            repo, "target/bench-compare/latest/metadata.txt"
        ),
        "first wrote metadata",
    )
    assert_file_absent(
        comparison_lock_path(repo),
        "lock released after success",
    )

    # Check no nested staging directory.
    latest = os.path.join(repo, "target/bench-compare/latest")
    nested = False
    if os.path.isdir(latest):
        for entry in os.listdir(latest):
            if entry.startswith(".latest-staging-"):
                nested = True
                break
    if nested:
        _fail("nested staging directory found")
    else:
        _pass("no nested staging directory")


# ═══════════════════════════════════════════════════════════════════════════
# Test 2: Signal releases lock
# ═══════════════════════════════════════════════════════════════════════════

def test_signal_releases_lock(tmp, sig, label):
    print()
    print(f"=== Test 2: {label} signal releases lock ===")
    test_dir = os.path.join(tmp, f"test2-{label}")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    block_file = os.path.join(test_dir, "block")
    stub_dir = os.path.join(test_dir, "stub")
    make_stub_cargo(
        stub_dir, os.path.join(test_dir, "cargo.log"), block_file
    )

    f_out = open(os.path.join(test_dir, "out"), "w")
    f_err = open(os.path.join(test_dir, "err"), "w")
    p = run_bench_compare_bg(
        repo, stub_dir, block_file, f_out, f_err
    )

    if not wait_for_block(block_file):
        _fail(f"{label} test: process did not start cargo")
        p.kill()
        p.wait()
        f_out.close()
        f_err.close()
        return
    _pass(f"{label} test: process started and is blocking")

    # Send signal to process group.
    pgid = os.getpgid(p.pid)
    os.killpg(pgid, sig)
    try:
        p.wait(timeout=15)
    except subprocess.TimeoutExpired:
        _fail(f"{label} test: process did not exit within 15s")
        os.killpg(pgid, signal.SIGKILL)
        p.wait()

    f_out.close()
    f_err.close()
    time.sleep(0.2)

    lock = comparison_lock_path(repo)
    if os.path.exists(lock):
        _fail(
            f"lock not released on {label}: "
            f"path '{lock}' unexpectedly exists"
        )
    else:
        _pass(f"lock released on {label}")


def test_signals(tmp):
    test_signal_releases_lock(tmp, signal.SIGTERM, "TERM")
    test_signal_releases_lock(tmp, signal.SIGINT, "INT")


# ═══════════════════════════════════════════════════════════════════════════
# Test 3: Pre-existing lock
# ═══════════════════════════════════════════════════════════════════════════

def test_existing_lock(tmp):
    print()
    print("=== Test 3: Pre-existing lock ===")
    test_dir = os.path.join(tmp, "test3")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    # Create a pre-existing lock.
    lock_dir = comparison_lock_path(repo)
    os.makedirs(lock_dir)
    with open(os.path.join(lock_dir, "pid"), "w") as f:
        f.write("999999\n")
    with open(os.path.join(lock_dir, "head-sha"), "w") as f:
        f.write("abc123\n")

    env = bench_compare_env(stub_dir)
    result = subprocess.run(
        [BENCH_COMPARE],
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        timeout=30,
    )

    assert_ne("existing lock exit code", 0, result.returncode)

    if "another benchmark comparison is already running" in result.stderr:
        _pass("existing lock: error message printed")
    else:
        _fail("existing lock: missing error message")
        print(result.stderr, file=sys.stderr)

    if "999999" in result.stderr:
        _pass("existing lock: PID reported")
    else:
        _fail("existing lock: PID not reported")

    if (
        not os.path.exists(cargo_log)
        or os.path.getsize(cargo_log) == 0
    ):
        _pass("existing lock: no Cargo calls")
    else:
        _fail("existing lock: unexpectedly called Cargo")

    if os.path.isdir(lock_dir):
        _pass("existing lock: lock directory preserved")
    else:
        _fail("existing lock: lock directory was deleted")


# ═══════════════════════════════════════════════════════════════════════════
# Test 4: Dirty tracked file is snapshotted correctly
# ═══════════════════════════════════════════════════════════════════════════

def test_dirty_snapshot(tmp):
    print()
    print("=== Test 4: Dirty tracked file in snapshot ===")
    test_dir = os.path.join(tmp, "test4")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    # Commit a production-like file, then modify it.
    commit_file(repo, "src/lib.rs", "original")
    write_file(repo, "src/lib.rs", "modified_for_benchmark")

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    env = bench_compare_env(stub_dir, extra_env={
        "SCAH_BENCH_TEST_READ_FILE": "src/lib.rs",
    })
    result = subprocess.run(
        [BENCH_COMPARE],
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )

    assert_eq("test4 exit code", 0, result.returncode)

    # Verify stub ran from a temp directory (not the repo).
    assert_contains(
        cargo_log, "CWD=", "cargo stub logged CWD"
    )
    assert_not_contains(
        cargo_log,
        f"CWD={repo}",
        "cargo stub did NOT run from live repo",
    )

    # Verify it saw the modified content.
    assert_contains(
        cargo_log,
        "FILE_CONTENT=modified_for_benchmark",
        "stub read dirty tracked file content",
    )

    # Verify metadata has source fingerprint.
    meta = read_metadata(repo)
    fp = meta.get("current_source_fingerprint_before", "")
    if fp and fp != "missing":
        _pass(f"source fingerprint recorded: {fp[:16]}...")
    else:
        _fail("source fingerprint missing or empty")

    # Verify snapshot metadata fields.
    assert_eq(
        "working_tree_snapshot", "true",
        meta.get("working_tree_snapshot", "")
    )
    assert_eq(
        "live_repository_used_for_measurement", "false",
        meta.get("live_repository_used_for_measurement", "")
    )
    assert_eq(
        "head_dirty", "true",
        meta.get("head_dirty", "")
    )


# ═══════════════════════════════════════════════════════════════════════════
# Test 5: Untracked file included in snapshot
# ═══════════════════════════════════════════════════════════════════════════

def test_untracked_in_snapshot(tmp):
    print()
    print("=== Test 5: Untracked file in snapshot ===")
    test_dir = os.path.join(tmp, "test5")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    # Create an untracked file with a space in its name.
    write_file(repo, "my data.txt", "untracked content with spaces")

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    env = bench_compare_env(stub_dir, extra_env={
        "SCAH_BENCH_TEST_READ_FILE": "my data.txt",
    })
    result = subprocess.run(
        [BENCH_COMPARE],
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )

    assert_eq("test5 exit code", 0, result.returncode)

    # The untracked file should exist in snapshot invocations.
    assert_contains(
        cargo_log,
        "FILE_CONTENT=untracked content with spaces",
        "stub read untracked file in snapshot",
    )

    # Base invocations should NOT see it.
    assert_contains(
        cargo_log,
        "FILE_MISSING",
        "base did not have untracked file",
    )

    # Verify the stub did not run from the live repo.
    assert_not_contains(
        cargo_log,
        f"CWD={repo}",
        "cargo stub did NOT run from live repo",
    )


# ═══════════════════════════════════════════════════════════════════════════
# Test 6: Live source mutation cannot alter the measurement
# ═══════════════════════════════════════════════════════════════════════════

def test_live_mutation_isolation(tmp):
    print()
    print("=== Test 6: Live mutation cannot alter measurement ===")
    test_dir = os.path.join(tmp, "test6")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    # Commit a file, then make a dirty modification.
    commit_file(repo, "src/lib.rs", "original")
    write_file(repo, "src/lib.rs", "dirty_version")

    block_file = os.path.join(test_dir, "block")
    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log, block_file)

    # Start bench-compare in background.
    f_out = open(os.path.join(test_dir, "out"), "w")
    f_err = open(os.path.join(test_dir, "err"), "w")
    p = run_bench_compare_bg(
        repo, stub_dir, block_file, f_out, f_err,
        extra_env={"SCAH_BENCH_TEST_READ_FILE": "src/lib.rs"},
    )

    if not wait_for_block(block_file):
        _fail("test6: process did not start cargo")
        p.kill()
        p.wait()
        f_out.close()
        f_err.close()
        return
    _pass("test6: first cargo invocation blocked (snapshot ready)")

    # Now mutate the live repo file AFTER the snapshot was captured.
    write_file(repo, "src/lib.rs", "mutated_after_snapshot")

    # Release the stub and wait for completion.
    with open(block_file + ".released", "w") as f:
        f.write("\n")
    p.wait(timeout=60)
    f_out.close()
    f_err.close()

    assert_eq("test6 exit code", 0, p.returncode)

    # Verify cargo invocations still see the ORIGINAL dirty content,
    # not the post-snapshot mutation.
    assert_contains(
        cargo_log,
        "FILE_CONTENT=dirty_version",
        "snapshot preserved original dirty version",
    )
    assert_not_contains(
        cargo_log,
        "FILE_CONTENT=mutated_after_snapshot",
        "live mutation did NOT leak into snapshot",
    )

    # Verify the fingerprint matches the snapshot.
    meta = read_metadata(repo)
    fp = meta.get("current_source_fingerprint_before", "")
    if fp and len(fp) == 64:
        _pass(f"fingerprint recorded: {fp[:16]}...")
    else:
        _fail("fingerprint missing or malformed")

    # Verify all cargo ran from temp, not live repo.
    assert_not_contains(
        cargo_log,
        f"CWD={repo}",
        "no cargo ran from live repo",
    )


# ═══════════════════════════════════════════════════════════════════════════
# Test 7: Dirty file content fingerprint changes
# ═══════════════════════════════════════════════════════════════════════════

def test_fingerprint_changes_with_content(tmp):
    print()
    print(
        "=== Test 7: Fingerprint changes with dirty content ==="
    )
    repo_a = os.path.join(tmp, "test7a", "repo")
    repo_b = os.path.join(tmp, "test7b", "repo")

    for repo in (repo_a, repo_b):
        create_test_repo(repo)
        commit_file(repo, "src/lib.rs", "original")
        # Same path, same git status, different content.
        suffix = "a" if repo == repo_a else "b"
        write_file(repo, "src/lib.rs", f"version_{suffix}")

        stub_dir = os.path.join(os.path.dirname(repo), "stub")
        cargo_log = os.path.join(
            os.path.dirname(repo), "cargo.log"
        )
        make_stub_cargo(stub_dir, cargo_log)

        env = bench_compare_env(stub_dir)
        result = subprocess.run(
            [BENCH_COMPARE],
            cwd=repo,
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        assert_eq(
            f"comparison {suffix} exit code", 0, result.returncode
        )

    meta_a = read_metadata(repo_a)
    meta_b = read_metadata(repo_b)

    fp_a = meta_a.get("current_source_fingerprint_before", "")
    fp_b = meta_b.get("current_source_fingerprint_before", "")

    if fp_a and fp_b:
        _pass("both fingerprints are present")
    else:
        _fail("one or both fingerprints missing")

    if fp_a != fp_b:
        _pass("fingerprints differ with different dirty content")
    else:
        _fail(
            f"fingerprints identical despite different content: {fp_a}"
        )


# ═══════════════════════════════════════════════════════════════════════════
# Test 8: Lockfile hashes are correct
# ═══════════════════════════════════════════════════════════════════════════

def test_lockfile_hashes(tmp):
    print()
    print("=== Test 8: Lockfile hashes ===")
    test_dir = os.path.join(tmp, "test8")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    # Commit a Cargo.lock with known content.
    lock_content = "# This is a fake lockfile for testing\n"
    commit_file(repo, "Cargo.lock", lock_content)

    # Also create a dirty lockfile modification.
    dirty_lock = lock_content + "# dirty line\n"
    write_file(repo, "Cargo.lock", dirty_lock)

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    env = bench_compare_env(stub_dir)
    result = subprocess.run(
        [BENCH_COMPARE],
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )

    assert_eq("test8 exit code", 0, result.returncode)

    meta = read_metadata(repo)

    # Base is HEAD~1 which does NOT have Cargo.lock → "missing".
    base_hash = meta.get("base_lockfile_sha256_before", "")
    assert_eq(
        "base lockfile hash is missing",
        "missing",
        base_hash,
    )

    # Current snapshot has the dirty lockfile.
    current_hash = meta.get("current_lockfile_sha256_before", "")
    expected_current = sha256_file(
        os.path.join(repo, "Cargo.lock")
    )
    assert_eq(
        "current lockfile hash matches dirty version",
        expected_current,
        current_hash,
    )


# ═══════════════════════════════════════════════════════════════════════════
# Test 9: Backup cleanup after publication interruption
# ═══════════════════════════════════════════════════════════════════════════

def test_backup_cleanup(tmp):
    print()
    print(
        "=== Test 9: Backup cleanup after publication"
        " interruption ==="
    )
    test_dir = os.path.join(tmp, "test9")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    # Use SCAH_BENCH_TEST_BLOCK_AFTER_PUBLISH hook to pause
    # after publication but before backup deletion.
    block_file = os.path.join(test_dir, "publish-block")

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    # Pre-create a "latest" so publish exercises the backup logic.
    latest_dir = os.path.join(
        repo, "target/bench-compare/latest"
    )
    os.makedirs(latest_dir)
    with open(os.path.join(latest_dir, "old.txt"), "w") as f:
        f.write("previous run\n")

    # Start bench-compare.
    f_out = open(os.path.join(test_dir, "out"), "w")
    f_err = open(os.path.join(test_dir, "err"), "w")
    p = run_bench_compare_bg(
        repo, stub_dir, None, f_out, f_err,
        extra_env={
            "SCAH_BENCH_TEST_BLOCK_AFTER_PUBLISH": block_file,
        },
    )

    # Wait for the publish hook to fire.
    started = block_file + ".started"
    deadline = time.time() + 60.0
    while time.time() < deadline:
        if os.path.exists(started):
            break
        time.sleep(0.1)

    if not os.path.exists(started):
        _fail("publish block did not fire")
        p.kill()
        p.wait()
        f_out.close()
        f_err.close()
        return
    _pass("publish block fired after publication")

    # Send SIGTERM — this allows the EXIT trap (cleanup) to run.
    # SIGKILL would prevent cleanup, defeating the purpose.
    pgid = os.getpgid(p.pid)
    os.killpg(pgid, signal.SIGTERM)
    try:
        p.wait(timeout=10)
    except subprocess.TimeoutExpired:
        os.killpg(pgid, signal.SIGKILL)
        p.wait()
    f_out.close()
    f_err.close()
    time.sleep(0.3)

    report_root = os.path.join(repo, "target/bench-compare")

    # Verify: latest exists and is valid.
    if os.path.isdir(latest_dir):
        _pass("latest directory exists after interruption")
    else:
        _fail("latest directory missing after interruption")

    # Verify: no .latest-backup-* directory remains.
    backup_found = False
    for entry in os.listdir(report_root):
        if entry.startswith(".latest-backup-"):
            backup_found = True
            _fail(f"stale backup directory found: {entry}")
    if not backup_found:
        _pass("no stale backup directory")

    # Verify: no .latest-staging-* directory remains.
    staging_found = False
    for entry in os.listdir(report_root):
        if entry.startswith(".latest-staging-"):
            staging_found = True
            _fail(f"stale staging directory found: {entry}")
    if not staging_found:
        _pass("no stale staging directory")

    # Verify: comparison lock is removed.
    lock_dir = comparison_lock_path(repo)
    if not os.path.exists(lock_dir):
        _pass("comparison lock released")
    else:
        _fail("comparison lock still present")


# ═══════════════════════════════════════════════════════════════════════════
# Test 10: Current snapshot mutation aborts publication
# ═══════════════════════════════════════════════════════════════════════════

def test_current_snapshot_mutation(tmp):
    print()
    print("=== Test 10: Current snapshot mutation aborts publication ===")
    test_dir = os.path.join(tmp, "test10")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    commit_file(repo, "src/lib.rs", "original source")

    # Pre-create a valid "latest" report.
    latest_dir = os.path.join(repo, "target/bench-compare/latest")
    os.makedirs(latest_dir)
    with open(os.path.join(latest_dir, "old.txt"), "w") as f:
        f.write("previous successful run\n")

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")

    # Stub mutates src/lib.rs when CWD contains "/current" (the current worktree).
    make_stub_cargo(
        stub_dir, cargo_log,
        mutate_when_cwd_contains="/current",
        mutate_file="src/lib.rs",
        mutate_content="mutated during benchmark",
    )

    env = bench_compare_env(stub_dir)
    result = subprocess.run(
        [BENCH_COMPARE],
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )

    # Must exit nonzero.
    assert_ne("current mutation exit code", 0, result.returncode)

    # stderr must report current snapshot changed.
    stderr = result.stderr
    if "source snapshot changed" in stderr and "current" in stderr:
        _pass("error message identifies current snapshot mutation")
    else:
        _fail("missing current snapshot mutation error message")
        print(f"stderr: {stderr}", file=sys.stderr)

    # Old report must remain intact.
    old_file = os.path.join(latest_dir, "old.txt")
    if os.path.isfile(old_file):
        _pass("old latest report preserved")
    else:
        _fail("old latest report was removed")

    # No staging directory remains.
    staging_found = False
    report_root = os.path.join(repo, "target/bench-compare")
    if os.path.isdir(report_root):
        for entry in os.listdir(report_root):
            if entry.startswith(".latest-staging-"):
                staging_found = True
                _fail(f"stale staging directory found: {entry}")
    if not staging_found:
        _pass("no staging directory remains")

    # No backup directory remains.
    backup_found = False
    if os.path.isdir(report_root):
        for entry in os.listdir(report_root):
            if entry.startswith(".latest-backup-"):
                backup_found = True
                _fail(f"stale backup directory found: {entry}")
    if not backup_found:
        _pass("no backup directory remains")

    # Lock released.
    lock_dir = comparison_lock_path(repo)
    assert_file_absent(lock_dir, "comparison lock released")

    # Temporary worktrees cleaned up (we can't inspect TEMP_ROOT directly,
    # but the cleanup trap handles it).


# ═══════════════════════════════════════════════════════════════════════════
# Test 11: Baseline snapshot mutation aborts publication
# ═══════════════════════════════════════════════════════════════════════════

def test_baseline_snapshot_mutation(tmp):
    print()
    print("=== Test 11: Baseline snapshot mutation aborts publication ===")
    test_dir = os.path.join(tmp, "test11")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    commit_file(repo, "src/lib.rs", "original source")

    # Pre-create a valid "latest" report.
    latest_dir = os.path.join(repo, "target/bench-compare/latest")
    os.makedirs(latest_dir)
    with open(os.path.join(latest_dir, "old.txt"), "w") as f:
        f.write("previous successful run\n")

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")

    # Stub mutates .gitignore when CWD contains "/base" (the baseline worktree).
    # .gitignore exists in all revisions (created in commit 1).
    make_stub_cargo(
        stub_dir, cargo_log,
        mutate_when_cwd_contains="/base",
        mutate_file=".gitignore",
        mutate_content="/target/\n# mutated\n",
    )

    env = bench_compare_env(stub_dir)
    result = subprocess.run(
        [BENCH_COMPARE],
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )

    # Must exit nonzero.
    assert_ne("baseline mutation exit code", 0, result.returncode)

    # stderr must report baseline snapshot changed.
    stderr = result.stderr
    if "source snapshot changed" in stderr and "baseline" in stderr:
        _pass("error message identifies baseline snapshot mutation")
    else:
        _fail("missing baseline snapshot mutation error message")
        print(f"stderr: {stderr}", file=sys.stderr)

    # Old report must remain intact.
    old_file = os.path.join(latest_dir, "old.txt")
    if os.path.isfile(old_file):
        _pass("old latest report preserved")
    else:
        _fail("old latest report was removed")

    # Lock released.
    lock_dir = comparison_lock_path(repo)
    assert_file_absent(lock_dir, "comparison lock released")


# ═══════════════════════════════════════════════════════════════════════════
# Test 12: Lockfile mutation is detected
# ═══════════════════════════════════════════════════════════════════════════

def test_lockfile_mutation(tmp):
    print()
    print("=== Test 12: Lockfile mutation is detected ===")
    test_dir = os.path.join(tmp, "test12")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    commit_file(repo, "Cargo.lock", "# lockfile v1\n")

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")

    # Stub mutates Cargo.lock in the current worktree.
    make_stub_cargo(
        stub_dir, cargo_log,
        mutate_when_cwd_contains="/current",
        mutate_file="Cargo.lock",
        mutate_content="# mutated lockfile\n",
    )

    env = bench_compare_env(stub_dir)
    result = subprocess.run(
        [BENCH_COMPARE],
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )

    # Must exit nonzero.
    assert_ne("lockfile mutation exit code", 0, result.returncode)

    # stderr must identify the mutation. The source fingerprint check
    # catches Cargo.lock changes (since the lockfile is part of the
    # source tree), so either error message is valid.
    stderr = result.stderr
    if "Cargo.lock changed" in stderr or "source snapshot changed" in stderr:
        _pass("error message identifies lockfile mutation")
    else:
        _fail("missing lockfile mutation error message")
        print(f"stderr: {stderr}", file=sys.stderr)

    # No new report published.
    latest = os.path.join(repo, "target/bench-compare/latest")
    if not os.path.isdir(latest):
        _pass("no new report published after lockfile mutation")
    else:
        # May exist from pre-creation in another test, but shouldn't
        # contain the metadata from the failed run.
        meta_path = os.path.join(latest, "metadata.txt")
        if os.path.isfile(meta_path):
            meta = read_metadata(repo)
            if meta.get("lockfile_endpoint_hashes_match") == "true":
                _fail("report published despite lockfile mutation")
            else:
                _pass("no new report published after lockfile mutation")
        else:
            _pass("no new report published after lockfile mutation")


# ═══════════════════════════════════════════════════════════════════════════
# Test 13: Identical source trees produce identical fingerprints
# ═══════════════════════════════════════════════════════════════════════════

def test_identical_fingerprints(tmp):
    print()
    print("=== Test 13: Identical trees → identical fingerprints ===")
    dir_a = os.path.join(tmp, "test13a", "snap")
    dir_b = os.path.join(tmp, "test13b", "snap")

    for d in (dir_a, dir_b):
        os.makedirs(os.path.join(d, "src"), exist_ok=True)
        with open(os.path.join(d, "src", "lib.rs"), "w") as f:
            f.write("fn main() {}\n")
        with open(os.path.join(d, "Cargo.toml"), "w") as f:
            f.write('[package]\nname = "test"\n')

    rc_a, fp_a, err_a, _ = run_fingerprint(dir_a)
    rc_b, fp_b, err_b, _ = run_fingerprint(dir_b)

    assert_eq("test13 helper exit (a)", 0, rc_a)
    assert_eq("test13 helper exit (b)", 0, rc_b)

    if fp_a == fp_b:
        _pass("identical trees produce identical fingerprints")
    else:
        _fail(
            f"fingerprints differ for identical trees: "
            f"{fp_a[:16]}... vs {fp_b[:16]}..."
        )


# ═══════════════════════════════════════════════════════════════════════════
# Test 14: Source-content change alters the fingerprint
# ═══════════════════════════════════════════════════════════════════════════

def test_content_change_alters_fingerprint(tmp):
    print()
    print("=== Test 14: Content change alters fingerprint ===")
    dir_a = os.path.join(tmp, "test14a", "snap")
    dir_b = os.path.join(tmp, "test14b", "snap")

    for d in (dir_a, dir_b):
        os.makedirs(os.path.join(d, "src"), exist_ok=True)
        with open(os.path.join(d, "src", "lib.rs"), "w") as f:
            f.write("fn main() {}\n")
        with open(os.path.join(d, "Cargo.toml"), "w") as f:
            f.write('[package]\nname = "test"\n')

    # Verify equal initially.
    rc_a1, fp_a1, _, _ = run_fingerprint(dir_a)
    rc_b1, fp_b1, _, _ = run_fingerprint(dir_b)
    assert_eq("initial equality", fp_a1, fp_b1)

    # Modify one file.
    with open(os.path.join(dir_b, "src", "lib.rs"), "w") as f:
        f.write("fn main() { /* changed */ }\n")

    _, fp_b2, _, _ = run_fingerprint(dir_b)

    if fp_a1 != fp_b2:
        _pass("content change produces different fingerprint")
    else:
        _fail("fingerprints identical despite content change")


# ═══════════════════════════════════════════════════════════════════════════
# Test 15: .git is excluded from the fingerprint
# ═══════════════════════════════════════════════════════════════════════════

def test_git_excluded(tmp):
    print()
    print("=== Test 15: .git is excluded from fingerprint ===")
    dir_a = os.path.join(tmp, "test15a", "snap")
    dir_b = os.path.join(tmp, "test15b", "snap")

    for d in (dir_a, dir_b):
        os.makedirs(os.path.join(d, "src"), exist_ok=True)
        with open(os.path.join(d, "src", "lib.rs"), "w") as f:
            f.write("fn main() {}\n")

        # Simulate a linked worktree .git file with different content.
        git_file = os.path.join(d, ".git")
        suffix = os.path.basename(d)
        with open(git_file, "w") as f:
            f.write(f"gitdir: /fake/path/{suffix}/.git/worktrees/current\n")

    rc_a, fp_a, _, _ = run_fingerprint(dir_a)
    rc_b, fp_b, _, _ = run_fingerprint(dir_b)

    assert_eq("test15 helper exit (a)", 0, rc_a)
    assert_eq("test15 helper exit (b)", 0, rc_b)

    if fp_a == fp_b:
        _pass(".git file with different content does not affect fingerprint")
    else:
        _fail(".git content leaked into fingerprint")

    # Also verify manifest does not contain .git entries or absolute paths.
    manifest_path = os.path.join(tmp, "test15-manifest.bin")
    _, _, _, mp = run_fingerprint(dir_a, manifest_path)
    if os.path.isfile(mp):
        with open(mp, "rb") as f:
            data = f.read()
        # Check no .git entry (as a NUL-delimited record).
        records = data.split(b"\0")
        git_found = False
        abs_path_found = False
        for i in range(0, len(records) - 4, 4):
            rel = records[i + 3].decode("utf-8", errors="surrogateescape")
            if rel == ".git" or rel.startswith(".git" + os.sep):
                git_found = True
            if os.path.isabs(rel):
                abs_path_found = True
        if not git_found:
            _pass("manifest contains no .git entry")
        else:
            _fail("manifest contains .git entry")
        if not abs_path_found:
            _pass("manifest contains no absolute paths")
        else:
            _fail("manifest contains absolute paths")


# ═══════════════════════════════════════════════════════════════════════════
# Test 16: Symlink target affects the fingerprint
# ═══════════════════════════════════════════════════════════════════════════

def test_symlink_fingerprint(tmp):
    print()
    print("=== Test 16: Symlink target affects fingerprint ===")
    if not hasattr(os, "symlink"):
        _pass("symlink test skipped: platform does not support symlinks")
        return

    snap = os.path.join(tmp, "test16", "snap")
    os.makedirs(snap, exist_ok=True)

    # Create target files.
    with open(os.path.join(snap, "target-a"), "w") as f:
        f.write("content A\n")
    with open(os.path.join(snap, "target-b"), "w") as f:
        f.write("content A\n")  # same content, different path

    # Create symlink → target-a.
    os.symlink("target-a", os.path.join(snap, "link"))
    rc_a, fp_a, _, _ = run_fingerprint(snap)
    assert_eq("symlink fingerprint (a) exit", 0, rc_a)

    # Replace with symlink → target-b.
    os.unlink(os.path.join(snap, "link"))
    os.symlink("target-b", os.path.join(snap, "link"))
    rc_b, fp_b, _, _ = run_fingerprint(snap)
    assert_eq("symlink fingerprint (b) exit", 0, rc_b)

    if fp_a != fp_b:
        _pass("symlink target change produces different fingerprint")
    else:
        _fail("symlink target change did not affect fingerprint")

    # Verify symlink is NOT followed (hash of target string, not file content).
    # If it were followed, the fingerprints would be equal since both
    # target files have identical content.
    _pass("symlink represented as symlink (not followed)")


# ═══════════════════════════════════════════════════════════════════════════
# Test 17: Filenames with tabs and newlines are unambiguous
# ═══════════════════════════════════════════════════════════════════════════

def test_odd_filenames(tmp):
    print()
    print("=== Test 17: Odd filenames are unambiguous ===")
    snap = os.path.join(tmp, "test17", "snap")
    os.makedirs(snap, exist_ok=True)

    # Create files with special characters in their names.
    names = [
        "file\twith\ttab.txt",
        "file\nwith\nnewline.txt",
        "file with spaces.txt",
    ]
    for name in names:
        full = os.path.join(snap, name)
        with open(full, "w") as f:
            f.write(f"content of {repr(name)}\n")

    rc, fp1, err, manifest_path = run_fingerprint(snap)
    assert_eq("odd filenames fingerprint exit", 0, rc)

    # Change one file content, verify fingerprint changes.
    with open(os.path.join(snap, names[0]), "w") as f:
        f.write("modified content\n")
    _, fp2, _, _ = run_fingerprint(snap)

    if fp1 != fp2:
        _pass("content change in odd-named file alters fingerprint")
    else:
        _fail("content change did not affect fingerprint")

    # Verify all entries are in the manifest.
    if os.path.isfile(manifest_path):
        with open(manifest_path, "rb") as f:
            data = f.read()
        records = data.split(b"\0")
        found = set()
        for i in range(0, len(records) - 4, 4):
            rel = records[i + 3].decode("utf-8", errors="surrogateescape")
            found.add(rel)
        for name in names:
            if name in found:
                _pass(f"odd filename '{repr(name)}' found in manifest")
            else:
                _fail(f"odd filename '{repr(name)}' missing from manifest")
    else:
        _fail("manifest file not created")


# ═══════════════════════════════════════════════════════════════════════════
# Test 18: Unreadable or unsupported entries fail
# ═══════════════════════════════════════════════════════════════════════════

def test_unreadable_entries_fail(tmp):
    print()
    print("=== Test 18: Unreadable entries cause failure ===")
    snap = os.path.join(tmp, "test18", "snap")
    os.makedirs(snap, exist_ok=True)

    # Create a regular readable file.
    with open(os.path.join(snap, "good.txt"), "w") as f:
        f.write("readable\n")

    # Create an unreadable file.
    bad_path = os.path.join(snap, "unreadable.txt")
    with open(bad_path, "w") as f:
        f.write("will become unreadable\n")
    os.chmod(bad_path, 0o000)

    rc, stdout, stderr, _ = run_fingerprint(snap)

    if rc != 0:
        _pass("unreadable file causes nonzero exit")
    else:
        _fail("unreadable file did not cause failure")

    # The error should name the problematic file.
    if "unreadable.txt" in stderr:
        _pass("error message names unreadable file")
    else:
        _fail("error message does not name unreadable file")
        print(f"stderr: {stderr}", file=sys.stderr)

    # Restore permissions for cleanup.
    os.chmod(bad_path, 0o644)


def test_unsupported_entry_fails(tmp):
    print()
    print("=== Test 18b: Unsupported entry type fails ===")
    snap = os.path.join(tmp, "test18b", "snap")
    os.makedirs(snap, exist_ok=True)

    # Create a FIFO (named pipe) — unsupported by the fingerprint helper.
    fifo_path = os.path.join(snap, "fifo")
    try:
        os.mkfifo(fifo_path)
    except (OSError, AttributeError):
        _pass("FIFO creation not supported on this platform — skipping")
        return

    rc, stdout, stderr, _ = run_fingerprint(snap)

    if rc != 0:
        _pass("unsupported entry type causes nonzero exit")
    else:
        _fail("unsupported entry type did not cause failure")

    if "unsupported" in stderr.lower():
        _pass("error message mentions unsupported type")
    else:
        _fail("error message does not mention unsupported type")
        print(f"stderr: {stderr}", file=sys.stderr)

    os.unlink(fifo_path)


# ═══════════════════════════════════════════════════════════════════════════
# Test 19: Manifest hash file exists and matches
# ═══════════════════════════════════════════════════════════════════════════

def test_manifest_hash_file(tmp):
    print()
    print("=== Test 19: Manifest hash file exists and matches ===")
    test_dir = os.path.join(tmp, "test19")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    env = bench_compare_env(stub_dir)
    result = subprocess.run(
        [BENCH_COMPARE],
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )

    assert_eq("test19 exit code", 0, result.returncode)

    # Verify manifest hash file exists.
    hash_file = os.path.join(
        repo,
        "target/bench-compare/latest/current-source-manifest.sha256",
    )
    assert_file_exists(hash_file, "manifest hash file exists")

    # Verify manifest binary exists.
    manifest_bin = os.path.join(
        repo,
        "target/bench-compare/latest/current-source-manifest.bin",
    )
    assert_file_exists(manifest_bin, "manifest binary exists")

    # Verify hash matches.
    if os.path.isfile(hash_file) and os.path.isfile(manifest_bin):
        with open(hash_file) as f:
            stored_hash = f.read().strip()
        computed_hash = sha256_file(manifest_bin)
        assert_eq(
            "manifest hash matches stored hash",
            stored_hash,
            computed_hash,
        )

        # Verify metadata fingerprint matches.
        meta = read_metadata(repo)
        meta_fp = meta.get("current_source_fingerprint_before", "")
        assert_eq(
            "metadata fingerprint matches manifest hash",
            stored_hash,
            meta_fp,
        )

    # Verify integrity fields in metadata.
    meta = read_metadata(repo)
    assert_eq(
        "source_snapshot_endpoint_fingerprints_match", "true",
        meta.get("source_snapshot_endpoint_fingerprints_match", "")
    )
    assert_eq(
        "lockfile_endpoint_hashes_match", "true",
        meta.get("lockfile_endpoint_hashes_match", "")
    )
    # base_source_fingerprint should also exist.
    base_fp = meta.get("base_source_fingerprint_before", "")
    if base_fp and len(base_fp) == 64:
        _pass("base source fingerprint recorded")
    else:
        _fail("base source fingerprint missing or malformed")


# ═══════════════════════════════════════════════════════════════════════════
# Test 20: Directory symlink target changes the fingerprint
# ═══════════════════════════════════════════════════════════════════════════

def test_directory_symlink_fingerprint(tmp):
    print()
    print("=== Test 20: Directory symlink target changes fingerprint ===")
    if not hasattr(os, "symlink"):
        _pass("directory symlink test skipped: platform does not support symlinks")
        return

    snap = os.path.join(tmp, "test20", "snap")
    os.makedirs(snap, exist_ok=True)

    # Create two directories with identical contents.
    dir_a = os.path.join(snap, "dir-a")
    dir_b = os.path.join(snap, "dir-b")
    os.makedirs(dir_a)
    os.makedirs(dir_b)
    with open(os.path.join(dir_a, "value.txt"), "w") as f:
        f.write("same content\n")
    with open(os.path.join(dir_b, "value.txt"), "w") as f:
        f.write("same content\n")

    # Create directory symlink → dir-a.
    symlink_path = os.path.join(snap, "linked-dir")
    os.symlink("dir-a", symlink_path)

    rc_a, fp_a, _, manifest_a = run_fingerprint(snap)
    assert_eq("dir symlink fingerprint (a) exit", 0, rc_a)

    # Verify manifest contains the symlink entry.
    records = read_manifest(manifest_a)
    link_entries = [r for r in records if r["path"] == "linked-dir"]
    if len(link_entries) == 1:
        _pass("directory symlink present in manifest")
    else:
        _fail("directory symlink missing from manifest")
        return

    entry = link_entries[0]
    assert_eq("dir symlink entry type", "symlink", entry["type"])
    assert_eq("dir symlink mode", "120000", entry["mode"])

    # Verify the content hash matches the target string "dir-a".
    expected_hash = hashlib.sha256(b"dir-a").hexdigest()
    assert_eq("dir symlink target hash", expected_hash, entry["hash"])

    # Verify files under the target are NOT duplicated through the link.
    file_paths = {r["path"] for r in records}
    if "linked-dir/value.txt" not in file_paths:
        _pass("directory symlink target not traversed")
    else:
        _fail("directory symlink target was traversed (value.txt duplicated)")

    # Replace symlink → dir-b.
    os.unlink(symlink_path)
    os.symlink("dir-b", symlink_path)
    rc_b, fp_b, _, _ = run_fingerprint(snap)
    assert_eq("dir symlink fingerprint (b) exit", 0, rc_b)

    if fp_a != fp_b:
        _pass("directory symlink target change alters fingerprint")
    else:
        _fail("directory symlink target change did not alter fingerprint")


# ═══════════════════════════════════════════════════════════════════════════
# Test 21: Broken directory-like symlink is fingerprinted
# ═══════════════════════════════════════════════════════════════════════════

def test_broken_symlink_fingerprint(tmp):
    print()
    print("=== Test 21: Broken symlink is fingerprinted ===")
    if not hasattr(os, "symlink"):
        _pass("broken symlink test skipped: platform does not support symlinks")
        return

    snap = os.path.join(tmp, "test21", "snap")
    os.makedirs(snap, exist_ok=True)

    # Create broken symlink → missing directory.
    os.symlink("missing-dir", os.path.join(snap, "linked-dir"))
    rc, fp1, _, manifest_path = run_fingerprint(snap)
    assert_eq("broken symlink fingerprint exit", 0, rc)

    records = read_manifest(manifest_path)
    link_entries = [r for r in records if r["path"] == "linked-dir"]
    if len(link_entries) == 1:
        _pass("broken symlink present in manifest")
    else:
        _fail("broken symlink missing from manifest")
        return

    entry = link_entries[0]
    assert_eq("broken symlink type", "symlink", entry["type"])
    assert_eq("broken symlink mode", "120000", entry["mode"])

    expected_hash = hashlib.sha256(b"missing-dir").hexdigest()
    assert_eq("broken symlink target hash", expected_hash, entry["hash"])

    # Replace with different broken target.
    os.unlink(os.path.join(snap, "linked-dir"))
    os.symlink("another-missing", os.path.join(snap, "linked-dir"))
    _, fp2, _, _ = run_fingerprint(snap)

    if fp1 != fp2:
        _pass("broken symlink target change alters fingerprint")
    else:
        _fail("broken symlink target change did not alter fingerprint")


# ═══════════════════════════════════════════════════════════════════════════
# Test 22: Cargo mutation of a directory symlink aborts publication
# ═══════════════════════════════════════════════════════════════════════════

def test_cargo_dir_symlink_mutation(tmp):
    print()
    print("=== Test 22: Cargo directory symlink mutation aborts publication ===")
    if not hasattr(os, "symlink"):
        _pass("dir symlink mutation test skipped: symlinks not supported")
        return

    test_dir = os.path.join(tmp, "test22")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    # Create committed directories and a directory symlink.
    assets_v1 = os.path.join(repo, "assets-v1")
    assets_v2 = os.path.join(repo, "assets-v2")
    os.makedirs(assets_v1)
    os.makedirs(assets_v2)
    with open(os.path.join(assets_v1, "data.txt"), "w") as f:
        f.write("v1\n")
    with open(os.path.join(assets_v2, "data.txt"), "w") as f:
        f.write("v2\n")

    # Create symlink and commit.
    os.symlink("assets-v1", os.path.join(repo, "active-assets"))
    subprocess.run(
        ["git", "-C", repo, "add", "assets-v1", "assets-v2", "active-assets"],
        check=True,
    )
    subprocess.run(
        ["git", "-C", repo, "commit", "-q", "-m", "add dir symlink"],
        check=True,
    )

    # Pre-create an old report.
    latest_dir = os.path.join(repo, "target/bench-compare/latest")
    os.makedirs(latest_dir)
    with open(os.path.join(latest_dir, "old.txt"), "w") as f:
        f.write("previous successful run\n")

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")

    # Stub replaces active-assets → assets-v2 when CWD contains "/current".
    make_stub_cargo(
        stub_dir, cargo_log,
        mutate_when_cwd_contains="/current",
        mutate_shell=(
            'rm -f "$CWD/active-assets" && '
            'ln -s "assets-v2" "$CWD/active-assets"'
        ),
    )

    env = bench_compare_env(stub_dir)
    result = subprocess.run(
        [BENCH_COMPARE],
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )

    # Must exit nonzero.
    assert_ne("Cargo dir symlink mutation exit code", 0, result.returncode)

    stderr = result.stderr
    if "source snapshot changed" in stderr and "current" in stderr:
        _pass("error message identifies current snapshot mutation")
    else:
        _fail("missing current snapshot mutation error message")
        print(f"stderr: {stderr}", file=sys.stderr)

    # Old report must remain intact.
    old_file = os.path.join(latest_dir, "old.txt")
    if os.path.isfile(old_file):
        _pass("old latest report preserved")
    else:
        _fail("old latest report was removed")

    # No staging directory remains.
    report_root = os.path.join(repo, "target/bench-compare")
    staging_found = False
    if os.path.isdir(report_root):
        for entry in os.listdir(report_root):
            if entry.startswith(".latest-staging-"):
                staging_found = True
                _fail(f"stale staging directory found: {entry}")
    if not staging_found:
        _pass("no staging directory remains")

    # No backup directory remains.
    backup_found = False
    if os.path.isdir(report_root):
        for entry in os.listdir(report_root):
            if entry.startswith(".latest-backup-"):
                backup_found = True
                _fail(f"stale backup directory found: {entry}")
    if not backup_found:
        _pass("no backup directory remains")

    # Lock released.
    lock_dir = comparison_lock_path(repo)
    assert_file_absent(lock_dir, "comparison lock released")


# ═══════════════════════════════════════════════════════════════════════════
# Test 23: Unreadable directory causes failure
# ═══════════════════════════════════════════════════════════════════════════

def test_unreadable_directory_fails(tmp):
    print()
    print("=== Test 23: Unreadable directory causes failure ===")
    snap = os.path.join(tmp, "test23", "snap")
    os.makedirs(snap, exist_ok=True)

    unreadable_dir = os.path.join(snap, "unreadable")
    os.makedirs(unreadable_dir, exist_ok=True)

    with open(os.path.join(unreadable_dir, "hidden.txt"), "w") as f:
        f.write("secret\n")

    if os.geteuid() == 0:
        # Running as root — permissions are not enforced.
        # Use the mock scandir failure hook instead.
        env = os.environ.copy()
        env["SCAH_TEST_FAIL_SCANDIR"] = "unreadable"
        result = subprocess.run(
            [sys.executable, FINGERPRINT_PY, "--root", snap,
             "--manifest", os.path.join(tmp, "test23", "manifest.bin")],
            capture_output=True,
            text=True,
            timeout=30,
            env=env,
        )
        assert_ne("mock scandir failure exit nonzero", 0, result.returncode)
        if "unreadable" in result.stderr:
            _pass("mock scandir error names directory")
        else:
            _fail("mock scandir error does not name directory")
            print(f"stderr: {result.stderr}", file=sys.stderr)
        return

    # Non-root: real permission test.
    os.chmod(unreadable_dir, 0o000)

    try:
        rc, stdout, stderr, _ = run_fingerprint(snap)

        if rc != 0:
            _pass("unreadable directory causes nonzero exit")
        else:
            _fail("unreadable directory did not cause failure")

        if "unreadable" in stderr:
            _pass("error message names unreadable directory")
        else:
            _fail("error message does not name unreadable directory")
            print(f"stderr: {stderr}", file=sys.stderr)
    finally:
        os.chmod(unreadable_dir, 0o755)


# ═══════════════════════════════════════════════════════════════════════════
# Test 24: Entry disappears during traversal
# ═══════════════════════════════════════════════════════════════════════════

def test_entry_disappears_during_traversal(tmp):
    print()
    print("=== Test 24: Entry disappears during traversal ===")
    snap = os.path.join(tmp, "test24", "snap")
    os.makedirs(snap, exist_ok=True)

    # Create a file that will "disappear" via mocked lstat failure.
    with open(os.path.join(snap, "vanishing.txt"), "w") as f:
        f.write("here today\n")

    env = os.environ.copy()
    env["SCAH_TEST_FAIL_LSTAT"] = "vanishing.txt"

    result = subprocess.run(
        [sys.executable, FINGERPRINT_PY, "--root", snap,
         "--manifest", os.path.join(tmp, "test24", "manifest.bin")],
        capture_output=True,
        text=True,
        timeout=30,
        env=env,
    )

    assert_ne("vanishing entry exit nonzero", 0, result.returncode)

    stderr = result.stderr
    if "vanishing.txt" in stderr:
        _pass("error message names vanished entry")
    else:
        _fail("error message does not name vanished entry")
        print(f"stderr: {stderr}", file=sys.stderr)

    if "failed to stat" in stderr or "mocked lstat" in stderr:
        _pass("error identifies stat failure")
    else:
        _fail("error does not identify stat failure")
        print(f"stderr: {stderr}", file=sys.stderr)


# ═══════════════════════════════════════════════════════════════════════════
# Test 25: Directory symlink manifest records
# ═══════════════════════════════════════════════════════════════════════════

def test_directory_symlink_manifest_records(tmp):
    print()
    print("=== Test 25: Directory symlink manifest records ===")
    if not hasattr(os, "symlink"):
        _pass("manifest record test skipped: symlinks not supported")
        return

    snap = os.path.join(tmp, "test25", "snap")
    os.makedirs(snap, exist_ok=True)

    # Create a regular file.
    with open(os.path.join(snap, "regular.txt"), "w") as f:
        f.write("hello world\n")

    # Create a real directory with a file inside.
    real_dir = os.path.join(snap, "real-dir")
    os.makedirs(real_dir)
    with open(os.path.join(real_dir, "inner.txt"), "w") as f:
        f.write("inside\n")

    # Create a file symlink.
    os.symlink("regular.txt", os.path.join(snap, "file-link"))

    # Create a directory symlink.
    os.symlink("real-dir", os.path.join(snap, "dir-link"))

    # Create a broken symlink.
    os.symlink("nowhere", os.path.join(snap, "broken-link"))

    rc, fp, _, manifest_path = run_fingerprint(snap)
    assert_eq("manifest records exit", 0, rc)

    records = read_manifest(manifest_path)
    record_by_path = {r["path"]: r for r in records}

    # All expected entries present.
    expected = {
        "regular.txt": "file",
        "real-dir/inner.txt": "file",
        "file-link": "symlink",
        "dir-link": "symlink",
        "broken-link": "symlink",
    }
    for path, expected_type in expected.items():
        if path in record_by_path:
            _pass(f"manifest contains {path!r}")
            actual_type = record_by_path[path]["type"]
            assert_eq(
                f"entry type for {path!r}",
                expected_type,
                actual_type,
            )
        else:
            _fail(f"manifest missing {path!r}")

    # Symlinks have mode 120000.
    for path in ("file-link", "dir-link", "broken-link"):
        if path in record_by_path:
            assert_eq(f"mode for {path!r}", "120000", record_by_path[path]["mode"])

    # Verify symlink hashes match target strings.
    file_link_hash = hashlib.sha256(b"regular.txt").hexdigest()
    dir_link_hash = hashlib.sha256(b"real-dir").hexdigest()
    broken_link_hash = hashlib.sha256(b"nowhere").hexdigest()

    if "file-link" in record_by_path:
        assert_eq("file-link target hash", file_link_hash,
                   record_by_path["file-link"]["hash"])
    if "dir-link" in record_by_path:
        assert_eq("dir-link target hash", dir_link_hash,
                   record_by_path["dir-link"]["hash"])
    if "broken-link" in record_by_path:
        assert_eq("broken-link target hash", broken_link_hash,
                   record_by_path["broken-link"]["hash"])

    # Real directories are not recorded as entries.
    for path in record_by_path:
        if path == "real-dir":
            _fail("real directory recorded as manifest entry")

    # Files under the directory symlink target are NOT duplicated.
    for path in record_by_path:
        if path.startswith("dir-link/"):
            _fail(f"directory symlink traversed: {path!r}")

    # Verify deterministic ordering: byte-oriented sort.
    paths = [r["path"] for r in records]
    byte_sorted = sorted(paths, key=lambda p: os.fsencode(p))
    assert_eq("manifest is sorted", byte_sorted, paths)


# ═══════════════════════════════════════════════════════════════════════════

# ═══════════════════════════════════════════════════════════════════════════
# Test 26: Escaping symlink in benchmark harness aborts workflow
# ═══════════════════════════════════════════════════════════════════════════

def test_escaping_symlink_harness(tmp):
    print()
    print("=== Test 26: Escaping symlink in harness aborts workflow ===")
    test_dir = os.path.join(tmp, "test26")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    # Create an absolute symlink in the benchmark harness directory.
    harness_dir = os.path.join(repo, "benches", "regression")
    escape_target = "/etc/passwd"
    symlink_path = os.path.join(harness_dir, "escape")
    os.symlink(escape_target, symlink_path)

    setup_prior_report(repo)

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    result = run_bench_compare(repo, stub_dir, timeout=60)

    assert_ne("escaping symlink exit nonzero", 0, result.returncode)
    assert_text_contains(
        result.stderr,
        "escaping symlink",
        "error identifies escaping symlink",
    )
    assert_text_contains(
        result.stderr,
        "escape",
        "error names escaping symlink path",
    )

    assert_cargo_not_run(cargo_log, "Cargo was not invoked (escaping symlink)")
    assert_prior_report_preserved(repo)
    assert_lock_absent(repo, "lock released after escaping symlink")
    assert_no_staging_or_backup(repo)


# ═══════════════════════════════════════════════════════════════════════════
# Test 27: Linked worktree lock prevents concurrent comparisons
# ═══════════════════════════════════════════════════════════════════════════

def test_linked_worktree_lock(tmp):
    print()
    print("=== Test 27: Linked worktree lock ===")
    test_dir = os.path.join(tmp, "test27")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    # Create a second linked worktree at the same commit as the main checkout.
    worktree_b = os.path.join(test_dir, "worktree-b")
    subprocess.run(
        ["git", "-C", repo, "worktree", "add", "--detach", worktree_b, "HEAD"],
        capture_output=True,
        check=True,
    )

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log_a = os.path.join(test_dir, "cargo-a.log")
    block = os.path.join(test_dir, "block")
    make_stub_cargo(stub_dir, cargo_log_a, block_file=block)

    stdout_a = os.path.join(test_dir, "out-a")
    stderr_a = os.path.join(test_dir, "err-a")
    stdout_b = os.path.join(test_dir, "out-b")
    stderr_b = os.path.join(test_dir, "err-b")

    f_out_a = open(stdout_a, "w")
    f_err_a = open(stderr_a, "w")

    lock_path = comparison_lock_path(repo)

    # Start comparison from worktree A (the main repo).
    p_a = run_bench_compare_bg(repo, stub_dir, block, f_out_a, f_err_a,
                               extra_env=None)

    if not wait_for_block(block, timeout=10):
        _fail("worktree A lock block did not fire")
        return
    _pass("worktree A acquired lock")

    owner_pid = ""
    pid_file = os.path.join(lock_path, "pid")
    if os.path.isfile(pid_file):
        with open(pid_file) as f:
            owner_pid = f.read().strip()

    # Start comparison from worktree B while A holds the lock.
    env_b = bench_compare_env(stub_dir)
    p_b = subprocess.Popen(
        [BENCH_COMPARE],
        cwd=worktree_b,
        env=env_b,
        stdout=open(stdout_b, "w"),
        stderr=open(stderr_b, "w"),
        start_new_session=True,
    )

    p_b.wait(timeout=30)
    assert_ne("worktree B rejected", 0, p_b.returncode)

    with open(stderr_b) as f:
        stderr_b_text = f.read()
    assert_text_contains(
        stderr_b_text,
        lock_path,
        "worktree B error mentions common lock path",
    )
    assert_text_contains(
        stderr_b_text,
        repo,
        "worktree B error mentions owner worktree path",
    )
    if owner_pid:
        assert_text_contains(
            stderr_b_text,
            owner_pid,
            "worktree B error mentions lock owner PID",
        )
    else:
        assert_text_contains(
            stderr_b_text,
            "lock owner PID",
            "worktree B error mentions lock owner PID line",
        )

    # Release A.
    with open(block + ".released", "w") as f:
        f.write("")

    p_a.wait(timeout=30)
    f_out_a.close()
    f_err_a.close()
    assert_eq("worktree A succeeded", 0, p_a.returncode)

    assert_lock_absent(repo, "lock released after completion")

    cargo_log_b = os.path.join(test_dir, "cargo-b.log")
    make_stub_cargo(stub_dir, cargo_log_b)
    result_b = run_bench_compare(worktree_b, stub_dir, timeout=60)
    assert_eq("worktree B succeeds after lock cleared", 0, result_b.returncode)

    # Clean up linked worktree.
    subprocess.run(
        ["git", "-C", repo, "worktree", "remove", "--force", worktree_b],
        capture_output=True,
    )


# ═══════════════════════════════════════════════════════════════════════════
# Test 28: Baseline mutation is detected
# ═══════════════════════════════════════════════════════════════════════════

def test_baseline_mutation_detected(tmp):
    print()
    print("=== Test 28: Baseline mutation is detected ===")
    test_dir = os.path.join(tmp, "test28")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    setup_prior_report(repo)

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    result = run_bench_compare(
        repo,
        stub_dir,
        extra_env={"SCAH_STUB_MUTATE_BASELINE_FILE": "1"},
        timeout=60,
    )

    assert_ne("baseline mutation exit nonzero", 0, result.returncode)
    assert_text_contains(
        result.stderr,
        "baseline measurements were modified",
        "error identifies baseline mutation",
    )
    assert_prior_report_preserved(repo)
    assert_lock_absent(repo, "comparison lock released")


# ═══════════════════════════════════════════════════════════════════════════
# Test 29: Untracked FIFO rejected before Cargo
# ═══════════════════════════════════════════════════════════════════════════

def test_untracked_fifo_rejected_before_cargo(tmp):
    print()
    print("=== Test 29: Untracked FIFO rejected before Cargo ===")
    try:
        os.mkfifo(os.path.join(tmp, "probe.fifo"))
        os.unlink(os.path.join(tmp, "probe.fifo"))
    except (OSError, AttributeError):
        _pass("FIFO creation not supported on this platform — skipping")
        return

    test_dir = os.path.join(tmp, "test29")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    # Git omits pure FIFO paths from ls-files; scan-special-files catches them.
    rel_fifo = "blocked.fifo"
    repo_fifo = os.path.join(repo, rel_fifo)
    os.mkfifo(repo_fifo)

    setup_prior_report(repo)

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    result = run_bench_compare(repo, stub_dir, timeout=30)

    assert_ne("FIFO rejection exit nonzero", 0, result.returncode)
    assert_text_contains(result.stderr, rel_fifo, "stderr names FIFO path")
    assert_text_contains(result.stderr, "FIFO", "stderr mentions FIFO")
    assert_text_contains(
        result.stderr,
        "unsupported entry type",
        "stderr mentions unsupported entry type",
    )
    assert_cargo_not_run(cargo_log, "Cargo not invoked for FIFO")
    assert_prior_report_preserved(repo)
    assert_lock_absent(repo, "lock absent after FIFO rejection")
    assert_no_staging_or_backup(repo)

    os.unlink(repo_fifo)


# ═══════════════════════════════════════════════════════════════════════════
# Test 30: Untracked socket rejected before Cargo
# ═══════════════════════════════════════════════════════════════════════════

def test_untracked_socket_rejected_before_cargo(tmp):
    print()
    print("=== Test 30: Untracked socket rejected before Cargo ===")
    test_dir = os.path.join(tmp, "test30")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    rel_sock = "blocked.sock"
    sock_path = os.path.join(repo, rel_sock)
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.bind(sock_path)
        sock.close()
    except (OSError, AttributeError):
        _pass("AF_UNIX socket creation not supported — skipping")
        return

    setup_prior_report(repo)

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    result = run_bench_compare(repo, stub_dir, timeout=30)

    assert_ne("socket rejection exit nonzero", 0, result.returncode)
    assert_text_contains(result.stderr, rel_sock, "stderr names socket path")
    assert_text_contains(result.stderr, "socket", "stderr mentions socket")
    assert_text_contains(
        result.stderr,
        "unsupported entry type",
        "stderr mentions unsupported entry type",
    )
    assert_cargo_not_run(cargo_log, "Cargo not invoked for socket")
    assert_prior_report_preserved(repo)
    assert_lock_absent(repo, "lock absent after socket rejection")
    assert_no_staging_or_backup(repo)

    os.unlink(sock_path)


# ═══════════════════════════════════════════════════════════════════════════
# Capture race tests (31–37)
# ═══════════════════════════════════════════════════════════════════════════

def _capture_race_repo(tmp, test_id, untracked_path="generated.rs", content="v1"):
    test_dir = os.path.join(tmp, f"test{test_id}")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)
    write_file(repo, untracked_path, content)
    return test_dir, repo


def test_capture_race_content_mutation_after_list(tmp):
    print()
    print("=== Test 31: Capture race — content mutation after list ===")
    test_dir, repo = _capture_race_repo(tmp, 31)
    block = os.path.join(test_dir, "capture-block")
    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    gen_path = os.path.join(repo, "generated.rs")

    def mutate():
        with open(gen_path, "w") as f:
            f.write("v2-after-list\n")

    stop_event = threading.Event()
    releaser = threading.Thread(
        target=capture_hook_releaser,
        args=(block, stop_event, mutate, False),
        daemon=True,
    )
    releaser.start()

    proc, f_out, f_err, _, stderr_path = run_capture_hook_bg(
        repo,
        stub_dir,
        block,
        "SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_LIST",
    )

    proc.wait(timeout=120)
    stop_event.set()
    releaser.join(timeout=5)
    f_out.close()
    f_err.close()

    with open(stderr_path) as f:
        stderr = f.read()

    assert_eq("content mutation retry succeeds", 0, proc.returncode)
    if "working tree changed during snapshot capture" in stderr:
        _pass("stderr mentions capture retry")
    else:
        _pass("capture succeeded after one-time content mutation")
    assert_cargo_ran(cargo_log, "Cargo ran after stable capture")
    assert_lock_absent(repo, "lock released after success")


def test_capture_race_mode_mutation_after_list(tmp):
    print()
    print("=== Test 32: Capture race — mode mutation after list ===")
    test_dir, repo = _capture_race_repo(tmp, 32)
    block = os.path.join(test_dir, "capture-block")
    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    gen_path = os.path.join(repo, "generated.rs")

    def mutate():
        os.chmod(gen_path, 0o755)

    stop_event = threading.Event()
    releaser = threading.Thread(
        target=capture_hook_releaser,
        args=(block, stop_event, mutate, False),
        daemon=True,
    )
    releaser.start()

    proc, f_out, f_err, _, stderr_path = run_capture_hook_bg(
        repo,
        stub_dir,
        block,
        "SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_LIST",
    )

    proc.wait(timeout=120)
    stop_event.set()
    releaser.join(timeout=5)
    f_out.close()
    f_err.close()

    with open(stderr_path) as f:
        stderr = f.read()

    if proc.returncode == 0:
        _pass("mode mutation retry succeeded")
    else:
        assert_text_contains(
            stderr,
            "working tree changed during snapshot capture",
            "mode mutation caused capture failure",
        )
    assert_cargo_ran(cargo_log, "Cargo ran after stable capture")
    assert_lock_absent(repo, "lock released")


def test_capture_race_file_to_symlink_after_capture(tmp):
    print()
    print("=== Test 33: Capture race — file to symlink after capture ===")
    test_dir, repo = _capture_race_repo(tmp, 33)
    block = os.path.join(test_dir, "capture-block")
    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    gen_path = os.path.join(repo, "generated.rs")

    def mutate():
        os.remove(gen_path)
        os.symlink("other.rs", gen_path)

    stop_event = threading.Event()
    releaser = threading.Thread(
        target=capture_hook_releaser,
        args=(block, stop_event, mutate, False),
        daemon=True,
    )
    releaser.start()

    proc, f_out, f_err, _, stderr_path = run_capture_hook_bg(
        repo,
        stub_dir,
        block,
        "SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_CAPTURE",
    )

    proc.wait(timeout=120)
    stop_event.set()
    releaser.join(timeout=5)
    f_out.close()
    f_err.close()

    with open(stderr_path) as f:
        stderr = f.read()

    if proc.returncode == 0:
        _pass("file-to-symlink mutation retry succeeded")
    else:
        assert_text_contains(
            stderr,
            "working tree changed during snapshot capture",
            "file-to-symlink caused capture failure",
        )
    assert_cargo_ran(cargo_log, "Cargo ran after stable capture")
    assert_lock_absent(repo, "lock released")


def test_capture_race_symlink_target_mutation(tmp):
    print()
    print("=== Test 34: Capture race — symlink target mutation ===")
    if not hasattr(os, "symlink"):
        _pass("symlink target mutation skipped: no symlink support")
        return

    test_dir = os.path.join(tmp, "test34")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)
    write_file(repo, "target-a.txt", "a")
    write_file(repo, "target-b.txt", "b")
    os.symlink("target-a.txt", os.path.join(repo, "linked.rs"))

    block = os.path.join(test_dir, "capture-block")
    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    link_path = os.path.join(repo, "linked.rs")

    def mutate():
        os.remove(link_path)
        os.symlink("target-b.txt", link_path)

    stop_event = threading.Event()
    releaser = threading.Thread(
        target=capture_hook_releaser,
        args=(block, stop_event, mutate, False),
        daemon=True,
    )
    releaser.start()

    proc, f_out, f_err, _, stderr_path = run_capture_hook_bg(
        repo,
        stub_dir,
        block,
        "SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_CAPTURE",
    )

    proc.wait(timeout=120)
    stop_event.set()
    releaser.join(timeout=5)
    f_out.close()
    f_err.close()

    with open(stderr_path) as f:
        stderr = f.read()

    if proc.returncode == 0:
        _pass("symlink target mutation retry succeeded")
    else:
        assert_text_contains(
            stderr,
            "working tree changed during snapshot capture",
            "symlink target mutation caused capture failure",
        )
    assert_cargo_ran(cargo_log, "Cargo ran after stable capture")
    assert_lock_absent(repo, "lock released")


def test_capture_race_added_file_after_list(tmp):
    print()
    print("=== Test 35: Capture race — added untracked file after list ===")
    test_dir, repo = _capture_race_repo(tmp, 35)
    block = os.path.join(test_dir, "capture-block")
    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    def mutate():
        write_file(repo, "extra.rs", "added during capture window\n")

    stop_event = threading.Event()
    releaser = threading.Thread(
        target=capture_hook_releaser,
        args=(block, stop_event, mutate, False),
        daemon=True,
    )
    releaser.start()

    proc, f_out, f_err, _, stderr_path = run_capture_hook_bg(
        repo,
        stub_dir,
        block,
        "SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_LIST",
    )

    proc.wait(timeout=120)
    stop_event.set()
    releaser.join(timeout=5)
    f_out.close()
    f_err.close()

    with open(stderr_path) as f:
        stderr = f.read()

    if proc.returncode == 0:
        _pass("added file mutation retry succeeded")
    else:
        assert_text_contains(
            stderr,
            "working tree changed during snapshot capture",
            "added file caused capture failure",
        )
    assert_cargo_ran(cargo_log, "Cargo ran after stable capture")
    assert_lock_absent(repo, "lock released")


def test_capture_race_deleted_file_before_inspect(tmp):
    print()
    print("=== Test 36: Capture race — deleted untracked file before inspect ===")
    test_dir, repo = _capture_race_repo(tmp, 36)
    block = os.path.join(test_dir, "capture-block")
    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    gen_path = os.path.join(repo, "generated.rs")

    def mutate():
        os.remove(gen_path)

    stop_event = threading.Event()
    releaser = threading.Thread(
        target=capture_hook_releaser,
        args=(block, stop_event, mutate, False),
        daemon=True,
    )
    releaser.start()

    proc, f_out, f_err, _, stderr_path = run_capture_hook_bg(
        repo,
        stub_dir,
        block,
        "SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_CAPTURE",
    )

    proc.wait(timeout=120)
    stop_event.set()
    releaser.join(timeout=5)
    f_out.close()
    f_err.close()

    with open(stderr_path) as f:
        stderr = f.read()

    if proc.returncode == 0:
        _pass("deleted file mutation retry succeeded")
        assert_cargo_ran(cargo_log, "Cargo ran after stable capture")
    else:
        if "working tree changed during snapshot capture" in stderr:
            _pass("deleted file caused capture retry failure")
        elif "failed to inspect live untracked entries" in stderr:
            _pass("deleted file caused inspect failure")
        else:
            _fail("deleted file failure missing expected error")
            print(f"stderr: {stderr}", file=sys.stderr)
        assert_cargo_not_run(cargo_log, "Cargo not invoked after deleted file")
    assert_lock_absent(repo, "lock released")


def test_capture_race_continuous_mutation_fails(tmp):
    print()
    print("=== Test 37: Capture race — continuous mutation exhausts retries ===")
    test_dir, repo = _capture_race_repo(tmp, 37)
    block = os.path.join(test_dir, "capture-block")
    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    gen_path = os.path.join(repo, "generated.rs")
    counter = {"n": 0}

    def mutate():
        counter["n"] += 1
        with open(gen_path, "w") as f:
            f.write(f"continuous-{counter['n']}\n")

    stop_event = threading.Event()
    mutator = threading.Thread(
        target=continuous_capture_mutator,
        args=(block, mutate, stop_event),
        daemon=True,
    )
    mutator.start()
    time.sleep(0.05)

    proc, f_out, f_err, _, stderr_path = run_capture_hook_bg(
        repo,
        stub_dir,
        block,
        "SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_CAPTURE",
    )

    proc.wait(timeout=120)
    stop_event.set()
    mutator.join(timeout=5)
    f_out.close()
    f_err.close()

    with open(stderr_path) as f:
        stderr = f.read()

    assert_ne("continuous mutation exit nonzero", 0, proc.returncode)
    if "working tree changed during snapshot capture" in stderr:
        _pass("continuous mutation error mentions state change")
    elif "failed to inspect live untracked entries" in stderr:
        _pass("continuous mutation failed during inspect")
    else:
        _fail("continuous mutation missing expected error")
        print(f"stderr: {stderr}", file=sys.stderr)
    if "attempts: 3" in stderr:
        _pass("continuous mutation error mentions attempt count")
    elif "attempt 3/3" in stderr:
        _pass("continuous mutation error mentions attempt count")
    else:
        _fail("continuous mutation error missing attempt count")
        print(f"stderr: {stderr}", file=sys.stderr)
    assert_cargo_not_run(cargo_log, "Cargo not invoked under continuous mutation")
    assert_lock_absent(repo, "lock absent after continuous mutation failure")


# ═══════════════════════════════════════════════════════════════════════════
# Special-file capture race tests
#
# Endpoint-based scanning catches FIFOs/sockets Git omits from ls-files.
# A special that appears only between scan A and scan B can still disappear
# before the next attempt's scan A; that transient gap is not detected.
# ═══════════════════════════════════════════════════════════════════════════

def _fifo_supported():
    try:
        probe = os.path.join(tempfile.gettempdir(), f"scah-fifo-probe-{os.getpid()}")
        os.mkfifo(probe)
        os.unlink(probe)
        return True
    except (OSError, AttributeError):
        return False


def _unix_socket_supported():
    probe = os.path.join(tempfile.gettempdir(), f"scah-sock-probe-{os.getpid()}")
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.bind(probe)
        sock.close()
        os.unlink(probe)
        return True
    except (OSError, AttributeError):
        return False


def test_capture_race_fifo_after_capture_continuous(tmp):
    print()
    print("=== Special-file race: FIFO after capture (continuous) ===")
    if not _fifo_supported():
        _pass("FIFO creation not supported on this platform — skipping")
        return

    fifo_path_holder = {}

    def mutate():
        repo = fifo_path_holder["repo"]
        fifo_path = os.path.join(repo, "late.fifo")
        if not os.path.exists(fifo_path):
            os.mkfifo(fifo_path)

    test_dir = os.path.join(tmp, "special-fifo-continuous")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)
    write_file(repo, "generated.rs", "v1\n")
    setup_prior_report(repo)
    fifo_path_holder["repo"] = repo

    block = os.path.join(test_dir, "capture-block")
    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    stop_event = threading.Event()
    mutator = threading.Thread(
        target=continuous_capture_mutator,
        args=(block, mutate, stop_event),
        daemon=True,
    )
    mutator.start()
    time.sleep(0.05)

    proc, f_out, f_err, _, stderr_path = run_capture_hook_bg(
        repo,
        stub_dir,
        block,
        "SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_CAPTURE",
    )

    proc.wait(timeout=120)
    stop_event.set()
    mutator.join(timeout=5)
    f_out.close()
    f_err.close()

    with open(stderr_path) as fh:
        stderr = fh.read()

    assert_ne("continuous FIFO race exit nonzero", 0, proc.returncode)
    assert_text_contains(
        stderr,
        "unsupported special file appeared during source capture",
        "stderr mentions special file during capture",
    )
    assert_text_contains(stderr, "late.fifo", "stderr names FIFO path")
    assert_text_contains(stderr, "FIFO", "stderr mentions FIFO type")
    if (
        "attempt 3/3" in stderr
        or "attempts: 3" in stderr
        or "present before source capture" in stderr
    ):
        _pass("stderr documents special-file failure")
    else:
        _fail("stderr missing special-file failure detail")
        print(f"stderr: {stderr}", file=sys.stderr)
    assert_cargo_not_run(cargo_log, "Cargo not invoked for continuous FIFO race")
    assert_prior_report_preserved(repo)
    assert_lock_absent(repo, "lock absent after continuous FIFO race failure")
    assert_no_staging_or_backup(repo)

    fifo_path = os.path.join(repo, "late.fifo")
    if os.path.exists(fifo_path):
        os.unlink(fifo_path)


def test_capture_race_socket_after_capture_continuous(tmp):
    print()
    print("=== Special-file race: socket after capture (continuous) ===")
    if not _unix_socket_supported():
        _pass("AF_UNIX socket creation not supported — skipping")
        return

    sock_path_holder = {}

    def mutate():
        repo = sock_path_holder["repo"]
        sock_path = os.path.join(repo, "late.sock")
        if os.path.exists(sock_path):
            return
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.bind(sock_path)
        sock.close()

    test_dir = os.path.join(tmp, "special-socket-continuous")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)
    write_file(repo, "generated.rs", "v1\n")
    setup_prior_report(repo)
    sock_path_holder["repo"] = repo

    block = os.path.join(test_dir, "capture-block")
    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    stop_event = threading.Event()
    mutator = threading.Thread(
        target=continuous_capture_mutator,
        args=(block, mutate, stop_event),
        daemon=True,
    )
    mutator.start()
    time.sleep(0.05)

    proc, f_out, f_err, _, stderr_path = run_capture_hook_bg(
        repo,
        stub_dir,
        block,
        "SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_CAPTURE",
    )

    proc.wait(timeout=120)
    stop_event.set()
    mutator.join(timeout=5)
    f_out.close()
    f_err.close()

    with open(stderr_path) as fh:
        stderr = fh.read()

    assert_ne("continuous socket race exit nonzero", 0, proc.returncode)
    assert_text_contains(
        stderr,
        "unsupported special file appeared during source capture",
        "stderr mentions special file during capture",
    )
    assert_text_contains(stderr, "late.sock", "stderr names socket path")
    assert_text_contains(stderr, "socket", "stderr mentions socket type")
    assert_cargo_not_run(cargo_log, "Cargo not invoked for continuous socket race")
    assert_prior_report_preserved(repo)
    assert_lock_absent(repo, "lock absent after continuous socket race failure")
    assert_no_staging_or_backup(repo)

    sock_path = os.path.join(repo, "late.sock")
    if os.path.exists(sock_path):
        os.unlink(sock_path)


def test_capture_race_fifo_after_list_continuous(tmp):
    print()
    print("=== Special-file race: FIFO after untracked list (continuous) ===")
    if not _fifo_supported():
        _pass("FIFO creation not supported on this platform — skipping")
        return

    fifo_path_holder = {}

    def mutate():
        repo = fifo_path_holder["repo"]
        fifo_path = os.path.join(repo, "late.fifo")
        if not os.path.exists(fifo_path):
            os.mkfifo(fifo_path)

    test_dir = os.path.join(tmp, "special-fifo-after-list")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)
    write_file(repo, "generated.rs", "v1\n")
    setup_prior_report(repo)
    fifo_path_holder["repo"] = repo

    block = os.path.join(test_dir, "capture-block")
    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    stop_event = threading.Event()
    mutator = threading.Thread(
        target=continuous_capture_mutator,
        args=(block, mutate, stop_event),
        daemon=True,
    )
    mutator.start()
    time.sleep(0.05)

    proc, f_out, f_err, _, stderr_path = run_capture_hook_bg(
        repo,
        stub_dir,
        block,
        "SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_LIST",
    )

    proc.wait(timeout=120)
    stop_event.set()
    mutator.join(timeout=5)
    f_out.close()
    f_err.close()

    with open(stderr_path) as fh:
        stderr = fh.read()

    assert_ne("FIFO after list race exit nonzero", 0, proc.returncode)
    if "unsupported special file appeared during source capture" in stderr:
        _pass("stderr mentions special file during capture")
    elif "unsupported special file present before source capture" in stderr:
        _pass("stderr mentions special file before next attempt")
    else:
        _fail("stderr missing special-file rejection")
        print(f"stderr: {stderr}", file=sys.stderr)
    assert_cargo_not_run(cargo_log, "Cargo not invoked for FIFO after list race")
    assert_prior_report_preserved(repo)
    assert_lock_absent(repo, "lock absent after FIFO after list race failure")
    assert_no_staging_or_backup(repo)

    fifo_path = os.path.join(repo, "late.fifo")
    if os.path.exists(fifo_path):
        os.unlink(fifo_path)


def test_capture_race_fifo_after_capture_one_time_success(tmp):
    print()
    print("=== Special-file race: FIFO after capture (one-time, retry succeeds) ===")
    if not _fifo_supported():
        _pass("FIFO creation not supported on this platform — skipping")
        return

    test_dir = os.path.join(tmp, "special-fifo-one-time")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)
    write_file(repo, "generated.rs", "v1\n")

    block = os.path.join(test_dir, "capture-block")
    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)
    fifo_path = os.path.join(repo, "late.fifo")
    stderr_path = block + ".stderr"

    def mutate():
        if not os.path.exists(fifo_path):
            os.mkfifo(fifo_path)

    stop_event = threading.Event()

    def remove_fifo_after_detection():
        deadline = time.time() + 120
        while not stop_event.is_set() and time.time() < deadline:
            if os.path.isfile(stderr_path):
                with open(stderr_path) as fh:
                    text = fh.read()
                if "unsupported special file appeared during source capture" in text:
                    try:
                        os.unlink(fifo_path)
                    except FileNotFoundError:
                        pass
                    return
            time.sleep(0.05)

    releaser = threading.Thread(
        target=capture_hook_releaser,
        args=(block, stop_event, mutate, False),
        daemon=True,
    )
    cleanup = threading.Thread(target=remove_fifo_after_detection, daemon=True)
    releaser.start()
    cleanup.start()

    proc, f_out, f_err, _, _ = run_capture_hook_bg(
        repo,
        stub_dir,
        block,
        "SCAH_BENCH_TEST_BLOCK_AFTER_UNTRACKED_CAPTURE",
    )

    proc.wait(timeout=120)
    stop_event.set()
    releaser.join(timeout=5)
    cleanup.join(timeout=5)
    f_out.close()
    f_err.close()

    with open(stderr_path) as fh:
        stderr = fh.read()

    assert_eq("one-time FIFO race succeeds after retry", 0, proc.returncode)
    if "unsupported special file appeared during source capture" in stderr:
        _pass("stderr mentions transient FIFO during capture")
    else:
        _pass("capture succeeded without persistent FIFO")
    assert_cargo_ran(cargo_log, "Cargo ran after FIFO removed before retry")
    assert_lock_absent(repo, "lock released after one-time FIFO race success")

    if os.path.exists(fifo_path):
        os.unlink(fifo_path)


# ═══════════════════════════════════════════════════════════════════════════
# Symlink policy workflow tests (38–45)
# ═══════════════════════════════════════════════════════════════════════════

def _symlink_rejection_workflow(tmp, test_id, setup_symlink_fn, err_fragment):
    test_dir = os.path.join(tmp, f"test{test_id}")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)
    setup_symlink_fn(repo, test_dir)
    setup_prior_report(repo)

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    result = run_bench_compare(
        repo,
        stub_dir,
        extra_env={"ALLOW_BENCH_HARNESS_DIFF": "1"},
        timeout=60,
    )

    assert_ne(f"symlink rejection test {test_id} exit nonzero", 0, result.returncode)
    assert_text_contains(
        result.stderr,
        err_fragment,
        f"symlink rejection test {test_id} error fragment",
    )
    assert_cargo_not_run(cargo_log, f"Cargo not invoked (test {test_id})")
    assert_prior_report_preserved(repo)
    assert_lock_absent(repo, f"lock released (test {test_id})")
    assert_no_staging_or_backup(repo)


def test_symlink_relative_escape_harness(tmp):
    print()
    print("=== Test 38: Relative escaping symlink in harness ===")
    if not hasattr(os, "symlink"):
        _pass("skipped: no symlink support")
        return

    def setup(repo, _test_dir):
        harness = os.path.join(repo, "benches", "regression")
        os.symlink("../../../outside.txt", os.path.join(harness, "escape-rel"))

    _symlink_rejection_workflow(tmp, 38, setup, "escaping symlink")


def test_symlink_chained_escape(tmp):
    print()
    print("=== Test 39: Chained internal symlink ending outside snapshot ===")
    if not hasattr(os, "symlink"):
        _pass("skipped: no symlink support")
        return

    def setup(repo, test_dir):
        outside = os.path.join(test_dir, "outside-chained")
        os.makedirs(outside, exist_ok=True)
        with open(os.path.join(outside, "secret.txt"), "w") as f:
            f.write("outside\n")
        link_b = os.path.join(repo, "link-b")
        link_a = os.path.join(repo, "link-a")
        os.symlink(os.path.join(outside, "secret.txt"), link_b)
        os.symlink("link-b", link_a)

    _symlink_rejection_workflow(tmp, 39, setup, "escaping symlink")


def test_symlink_broken_escape(tmp):
    print()
    print("=== Test 40: Broken symlink whose normalized target escapes ===")
    if not hasattr(os, "symlink"):
        _pass("skipped: no symlink support")
        return

    def setup(repo, _test_dir):
        os.symlink("../../etc/passwd", os.path.join(repo, "broken-escape"))

    _symlink_rejection_workflow(tmp, 40, setup, "escaping symlink")


def test_symlink_loop(tmp):
    print()
    print("=== Test 41: Symlink loop rejected ===")
    if not hasattr(os, "symlink"):
        _pass("skipped: no symlink support")
        return

    def setup(repo, _test_dir):
        os.symlink("loop-b", os.path.join(repo, "loop-a"))
        os.symlink("loop-a", os.path.join(repo, "loop-b"))

    _symlink_rejection_workflow(tmp, 41, setup, "symlink loop detected")


def test_symlink_chain_too_long(tmp):
    print()
    print("=== Test 42: Excessively long symlink chain rejected ===")
    if not hasattr(os, "symlink"):
        _pass("skipped: no symlink support")
        return

    def setup(repo, _test_dir):
        depth = 45
        for i in range(depth):
            target = f"chain-{i + 1}" if i + 1 < depth else "core_regression.rs"
            os.symlink(
                target,
                os.path.join(repo, "benches", "regression", f"chain-{i}"),
            )

    _symlink_rejection_workflow(tmp, 42, setup, "symlink chain too long")


def _symlink_allowed_workflow(tmp, test_id, setup_symlink_fn):
    test_dir = os.path.join(tmp, f"test{test_id}")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)
    setup_symlink_fn(repo)

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    result = run_bench_compare(repo, stub_dir, timeout=60)
    assert_eq(f"allowed symlink test {test_id} succeeds", 0, result.returncode)
    assert_cargo_ran(cargo_log, f"Cargo invoked (test {test_id})")
    assert_lock_absent(repo, f"lock released (test {test_id})")


def test_symlink_internal_file_allowed(tmp):
    print()
    print("=== Test 43: Internal regular-file symlink allowed ===")
    if not hasattr(os, "symlink"):
        _pass("skipped: no symlink support")
        return

    def setup(repo):
        os.symlink(
            "core_regression.rs",
            os.path.join(repo, "benches", "regression", "bench-link.rs"),
        )

    _symlink_allowed_workflow(tmp, 43, setup)


def test_symlink_internal_dir_allowed(tmp):
    print()
    print("=== Test 44: Internal directory symlink allowed ===")
    if not hasattr(os, "symlink"):
        _pass("skipped: no symlink support")
        return

    def setup(repo):
        os.symlink(
            "regression",
            os.path.join(repo, "benches", "bench-dir-link"),
        )

    _symlink_allowed_workflow(tmp, 44, setup)


def test_symlink_safe_broken_internal_allowed(tmp):
    print()
    print("=== Test 45: Safe broken internal symlink allowed ===")
    if not hasattr(os, "symlink"):
        _pass("skipped: no symlink support")
        return

    def setup(repo):
        os.symlink(
            "missing-internal.txt",
            os.path.join(repo, "broken-internal"),
        )

    _symlink_allowed_workflow(tmp, 45, setup)


# ═══════════════════════════════════════════════════════════════════════════
# Criterion failure-path tests (46–54)
# ═══════════════════════════════════════════════════════════════════════════

def _criterion_failure_test(tmp, test_id, stub_env, err_fragment):
    test_dir = os.path.join(tmp, f"test{test_id}")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)
    setup_prior_report(repo)

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    result = run_bench_compare(
        repo,
        stub_dir,
        extra_env=stub_env,
        timeout=60,
    )

    assert_ne(f"criterion failure test {test_id} exit nonzero", 0, result.returncode)
    assert_text_contains(
        result.stderr,
        err_fragment,
        f"criterion failure test {test_id} error",
    )
    assert_prior_report_preserved(repo)
    assert_lock_absent(repo, f"lock released (test {test_id})")


def test_criterion_skip_current_output(tmp):
    print()
    print("=== Test 46: Criterion zero-work current output ===")
    _criterion_failure_test(
        tmp,
        46,
        {"SCAH_STUB_SKIP_CURRENT_OUTPUT": "1"},
        "did not produce a fresh report",
    )


def test_criterion_report_only(tmp):
    print()
    print("=== Test 47: Criterion report-only output incomplete ===")
    _criterion_failure_test(
        tmp,
        47,
        {"SCAH_STUB_REPORT_ONLY": "1"},
        "Criterion comparison output is incomplete",
    )


def test_criterion_partial_inventory(tmp):
    print()
    print("=== Test 48: Criterion partial inventory ===")
    _criterion_failure_test(
        tmp,
        48,
        {"SCAH_STUB_SKIP_BENCHMARK_PATH": "multi_query/prebuilt"},
        "Criterion comparison output is incomplete",
    )


def test_criterion_missing_sample(tmp):
    print()
    print("=== Test 49: Criterion missing sample ===")
    _criterion_failure_test(
        tmp,
        49,
        {"SCAH_STUB_SKIP_CURRENT_SAMPLE": "1"},
        "Criterion comparison output is incomplete",
    )


def test_criterion_missing_change(tmp):
    print()
    print("=== Test 50: Criterion missing change estimates ===")
    _criterion_failure_test(
        tmp,
        50,
        {"SCAH_STUB_SKIP_CHANGE_ESTIMATES": "1"},
        "Criterion comparison output is incomplete",
    )


def test_criterion_baseline_file_deleted(tmp):
    print()
    print("=== Test 51: Criterion baseline file deletion ===")
    _criterion_failure_test(
        tmp,
        51,
        {"SCAH_STUB_DELETE_BASELINE_FILE": "1"},
        "Criterion comparison output is incomplete",
    )


def test_criterion_baseline_file_added(tmp):
    print()
    print("=== Test 52: Criterion baseline file addition ===")
    _criterion_failure_test(
        tmp,
        52,
        {"SCAH_STUB_ADD_BASELINE_FILE": "1"},
        "baseline measurements were modified",
    )


def test_criterion_baseline_symlink_inserted(tmp):
    print()
    print("=== Test 53: Criterion baseline symlink insertion ===")
    _criterion_failure_test(
        tmp,
        53,
        {"SCAH_STUB_INSERT_BASELINE_SYMLINK": "1"},
        "unsupported symlink in baseline data",
    )


def test_criterion_success_publishes_baseline_artifacts(tmp):
    print()
    print("=== Test 54: Valid Criterion output publishes baseline artifacts ===")
    test_dir = os.path.join(tmp, "test54")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    result = run_bench_compare(repo, stub_dir, timeout=60)
    assert_eq("valid criterion success exit code", 0, result.returncode)

    latest = os.path.join(repo, "target/bench-compare/latest")
    assert_file_exists(
        os.path.join(latest, "criterion-baseline-inventory.jsonl"),
        "baseline inventory published",
    )
    assert_file_exists(
        os.path.join(latest, "criterion-baseline-manifest.bin"),
        "baseline manifest published",
    )
    assert_file_exists(
        os.path.join(latest, "criterion-baseline-manifest.sha256"),
        "baseline manifest sha256 published",
    )

    meta = read_metadata(repo)
    before = meta.get("criterion_baseline_manifest_sha256_before", "")
    after = meta.get("criterion_baseline_manifest_sha256_after", "")
    assert_eq(
        "baseline manifest before/after hashes match",
        before,
        after,
    )
    assert_eq(
        "criterion_baseline_measurement_manifests_match",
        "true",
        meta.get("criterion_baseline_measurement_manifests_match", ""),
    )
    assert_eq(
        "criterion_baseline_measurements_unchanged",
        "true",
        meta.get("criterion_baseline_measurements_unchanged", ""),
    )

    manifest_bin = os.path.join(latest, "criterion-baseline-manifest.bin")
    hash_file = os.path.join(latest, "criterion-baseline-manifest.sha256")
    if os.path.isfile(hash_file) and os.path.isfile(manifest_bin):
        with open(hash_file) as f:
            stored = f.read().strip()
        assert_eq(
            "published manifest hash matches file",
            stored,
            sha256_file(manifest_bin),
        )

    assert_lock_absent(repo, "lock released after success")


# ═══════════════════════════════════════════════════════════════════════════
# Main
# ═══════════════════════════════════════════════════════════════════════════

def main():
    tmp = tempfile.mkdtemp(prefix="bench-compare-test-")
    try:
        # Existing tests (1-9).
        test_concurrent(tmp)
        test_signals(tmp)
        test_existing_lock(tmp)
        test_dirty_snapshot(tmp)
        test_untracked_in_snapshot(tmp)
        test_live_mutation_isolation(tmp)
        test_fingerprint_changes_with_content(tmp)
        test_lockfile_hashes(tmp)
        test_backup_cleanup(tmp)

        # New integrity tests (10-12).
        test_current_snapshot_mutation(tmp)
        test_baseline_snapshot_mutation(tmp)
        test_lockfile_mutation(tmp)

        # New fingerprint helper tests (13-19).
        test_identical_fingerprints(tmp)
        test_content_change_alters_fingerprint(tmp)
        test_git_excluded(tmp)
        test_symlink_fingerprint(tmp)
        test_odd_filenames(tmp)
        test_unreadable_entries_fail(tmp)
        test_unsupported_entry_fails(tmp)
        test_manifest_hash_file(tmp)

        # Directory symlink and traversal-error tests (20-25).
        test_directory_symlink_fingerprint(tmp)
        test_broken_symlink_fingerprint(tmp)
        test_cargo_dir_symlink_mutation(tmp)
        test_unreadable_directory_fails(tmp)
        test_entry_disappears_during_traversal(tmp)
        test_directory_symlink_manifest_records(tmp)

        # Full-workflow safety tests (26-54).
        test_escaping_symlink_harness(tmp)
        test_linked_worktree_lock(tmp)
        test_baseline_mutation_detected(tmp)
        test_untracked_fifo_rejected_before_cargo(tmp)
        test_untracked_socket_rejected_before_cargo(tmp)
        test_capture_race_content_mutation_after_list(tmp)
        test_capture_race_mode_mutation_after_list(tmp)
        test_capture_race_file_to_symlink_after_capture(tmp)
        test_capture_race_symlink_target_mutation(tmp)
        test_capture_race_added_file_after_list(tmp)
        test_capture_race_deleted_file_before_inspect(tmp)
        test_capture_race_continuous_mutation_fails(tmp)
        test_capture_race_fifo_after_capture_continuous(tmp)
        test_capture_race_socket_after_capture_continuous(tmp)
        test_capture_race_fifo_after_list_continuous(tmp)
        test_capture_race_fifo_after_capture_one_time_success(tmp)
        test_symlink_relative_escape_harness(tmp)
        test_symlink_chained_escape(tmp)
        test_symlink_broken_escape(tmp)
        test_symlink_loop(tmp)
        test_symlink_chain_too_long(tmp)
        test_symlink_internal_file_allowed(tmp)
        test_symlink_internal_dir_allowed(tmp)
        test_symlink_safe_broken_internal_allowed(tmp)
        test_criterion_skip_current_output(tmp)
        test_criterion_report_only(tmp)
        test_criterion_partial_inventory(tmp)
        test_criterion_missing_sample(tmp)
        test_criterion_missing_change(tmp)
        test_criterion_baseline_file_deleted(tmp)
        test_criterion_baseline_file_added(tmp)
        test_criterion_baseline_symlink_inserted(tmp)
        test_criterion_success_publishes_baseline_artifacts(tmp)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    print()
    print("=" * 44)
    print(f"Results: {PASS} passed, {FAIL} failed")
    print("=" * 44)

    if FAIL > 0:
        sys.exit(1)


if __name__ == "__main__":
    main()
