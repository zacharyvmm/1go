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
"""

import os
import signal
import shutil
import subprocess
import tempfile
import time
import sys
import hashlib

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(SCRIPT_DIR, "..", ".."))
BENCH_COMPARE = os.path.join(REPO_ROOT, "scripts", "bench-compare.sh")

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


def sha256_file(path):
    """Compute SHA-256 of a file."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


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


def make_stub_cargo(stub_dir, cargo_log, block_file=None):
    """Create a fake 'cargo' executable that logs invocations.

    Logs CWD and args on every call. When SCAH_BENCH_TEST_READ_FILE is
    set, reads that file (relative to CWD) and logs its content.
    Supports CARGO_BLOCK_FILE for concurrency tests.
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
        'mkdir -p "$CARGO_TARGET_DIR/criterion/report"',
        (
            "printf '<html>stub report</html>\\n'"
            ' > "$CARGO_TARGET_DIR/criterion/report/index.html"'
        ),
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
        os.path.join(
            repo, "target/bench-compare/.bench-compare-lock"
        ),
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

    lock = os.path.join(
        repo, "target/bench-compare/.bench-compare-lock"
    )
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
    lock_dir = os.path.join(
        repo, "target/bench-compare/.bench-compare-lock"
    )
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
    fp = meta.get("current_source_fingerprint", "")
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
    fp = meta.get("current_source_fingerprint", "")
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

    fp_a = meta_a.get("current_source_fingerprint", "")
    fp_b = meta_b.get("current_source_fingerprint", "")

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
    base_hash = meta.get("base_lockfile_sha256", "")
    assert_eq(
        "base lockfile hash is missing",
        "missing",
        base_hash,
    )

    # Current snapshot has the dirty lockfile.
    current_hash = meta.get("current_lockfile_sha256", "")
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
    lock_dir = os.path.join(report_root, ".bench-compare-lock")
    if not os.path.exists(lock_dir):
        _pass("comparison lock released")
    else:
        _fail("comparison lock still present")


# ═══════════════════════════════════════════════════════════════════════════
# Main
# ═══════════════════════════════════════════════════════════════════════════

def main():
    tmp = tempfile.mkdtemp(prefix="bench-compare-test-")
    try:
        # Existing tests (1-3).
        test_concurrent(tmp)
        test_signals(tmp)
        test_existing_lock(tmp)

        # New tests (4-9).
        test_dirty_snapshot(tmp)
        test_untracked_in_snapshot(tmp)
        test_live_mutation_isolation(tmp)
        test_fingerprint_changes_with_content(tmp)
        test_lockfile_hashes(tmp)
        test_backup_cleanup(tmp)
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
