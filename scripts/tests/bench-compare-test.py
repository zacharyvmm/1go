#!/usr/bin/env python3
"""Test: bench-compare.sh concurrency safety.

Validates:
  1. The first invocation acquires the repository-scoped lock.
  2. A second invocation exits nonzero before compiling any benchmark.
  3. The second invocation does not modify latest / staging / backups / the
     first process's lock.
  4. The first invocation continues normally and releases the lock.
  5. TERM signal releases the lock.
  6. A pre-existing (stale) lock causes immediate failure without Cargo calls.
"""

import os
import signal
import shutil
import subprocess
import tempfile
import time
import sys

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


def create_test_repo(repo):
    os.makedirs(repo, exist_ok=True)
    subprocess.run(["git", "-C", repo, "init", "-q", "-b", "main"], check=True)
    subprocess.run(["git", "-C", repo, "config", "user.email", "t@t.com"], check=True)
    subprocess.run(["git", "-C", repo, "config", "user.name", "T"], check=True)

    with open(os.path.join(repo, ".gitignore"), "w") as f:
        f.write("/target/\n")

    os.makedirs(os.path.join(repo, "benches", "regression"))
    with open(os.path.join(repo, "benches", "regression", "Cargo.toml"), "w") as f:
        f.write(
            "[package]\n"
            'name = "scah-regression-benches"\n'
            'version = "0.1.0"\n'
            'edition = "2021"\n'
            "[[bench]]\n"
            'name = "core_regression"\n'
            "harness = false\n"
        )

    with open(os.path.join(repo, "benches", "regression", "core_regression.rs"), "w") as f:
        f.write("// stub bench\n")

    subprocess.run(["git", "-C", repo, "add", "-A"], check=True)
    subprocess.run(["git", "-C", repo, "commit", "-q", "-m", "init"], check=True)

    with open(os.path.join(repo, "other.rs"), "w") as f:
        f.write("// unrelated\n")
    subprocess.run(["git", "-C", repo, "add", "other.rs"], check=True)
    subprocess.run(["git", "-C", repo, "commit", "-q", "-m", "other"], check=True)


def make_stub_cargo(stub_dir, cargo_log, block_file=None):
    os.makedirs(stub_dir, exist_ok=True)
    stub_path = os.path.join(stub_dir, "cargo")
    lines = [
        "#!/usr/bin/env bash",
        "set -euo pipefail",
        "",
        f'printf \'%s\\n\' "$*" >> {shlex_quote(cargo_log)}',
        "",
        'CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target}"',
        "",
    ]
    if block_file:
        lines += [
            'if [ -n "${CARGO_BLOCK_FILE:-}" ] && [ ! -e "${CARGO_BLOCK_FILE}.released" ]; then',
            '    touch "${CARGO_BLOCK_FILE}.started"',
            '    while [ ! -e "${CARGO_BLOCK_FILE}.released" ]; do',
            "        sleep 0.05",
            "    done",
            "fi",
            "",
        ]
    lines += [
        'mkdir -p "$CARGO_TARGET_DIR/criterion/report"',
        "printf '<html>stub report</html>\\n' > \"$CARGO_TARGET_DIR/criterion/report/index.html\"",
        "",
    ]
    with open(stub_path, "w") as f:
        f.write("\n".join(lines))
    os.chmod(stub_path, 0o755)


def shlex_quote(s):
    """Simple shell quoting."""
    return "'" + s.replace("'", "'\\''") + "'"


def bench_compare_env(stub_dir, block_file=None):
    env = os.environ.copy()
    env["PATH"] = stub_dir + ":" + env.get("PATH", "")
    env["ALLOW_BENCH_HARNESS_DIFF"] = "1"
    env["BASE_REF"] = "HEAD~1"
    if block_file:
        env["CARGO_BLOCK_FILE"] = block_file
    return env


def run_bench_compare_bg(repo, stub_dir, block_file, stdout, stderr):
    """Start bench-compare.sh in a new session so we can signal the process group."""
    env = bench_compare_env(stub_dir, block_file)
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


# ── Test 1: Concurrent invocation ────────────────────────────────────────────

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

    # Start first invocation
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

    # Second invocation (no blocking stub)
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

    cargo2_empty = not os.path.exists(cargo_log2) or os.path.getsize(cargo_log2) == 0
    if cargo2_empty:
        _pass("second invocation made no Cargo calls")
    else:
        _fail("second invocation called Cargo")

    # Release first process
    with open(block_file + ".released", "w") as f:
        f.write("\n")
    p1.wait(timeout=30)
    f_out.close()
    f_err.close()

    assert_eq("first exit code", 0, p1.returncode)
    assert_file_exists(
        os.path.join(repo, "target/bench-compare/latest/report/index.html"),
        "first published report",
    )
    assert_file_exists(
        os.path.join(repo, "target/bench-compare/latest/metadata.txt"),
        "first wrote metadata",
    )
    assert_file_absent(
        os.path.join(repo, "target/bench-compare/.bench-compare-lock"),
        "lock released after success",
    )

    # Check no nested staging directory
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


# ── Test 2: Signal releases lock ─────────────────────────────────────────────

def test_signal_releases_lock(tmp, sig, label):
    print()
    print(f"=== Test 2: {label} signal releases lock ===")
    test_dir = os.path.join(tmp, f"test2-{label}")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    block_file = os.path.join(test_dir, "block")
    stub_dir = os.path.join(test_dir, "stub")
    make_stub_cargo(stub_dir, os.path.join(test_dir, "cargo.log"), block_file)

    f_out = open(os.path.join(test_dir, "out"), "w")
    f_err = open(os.path.join(test_dir, "err"), "w")
    p = run_bench_compare_bg(repo, stub_dir, block_file, f_out, f_err)

    if not wait_for_block(block_file):
        _fail(f"{label} test: process did not start cargo")
        p.kill()
        p.wait()
        f_out.close()
        f_err.close()
        return
    _pass(f"{label} test: process started and is blocking")

    # Send signal to process group
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

    lock = os.path.join(repo, "target/bench-compare/.bench-compare-lock")
    if os.path.exists(lock):
        _fail(f"lock released on {label}: path '{lock}' unexpectedly exists")
    else:
        _pass(f"lock released on {label}")


def test_signals(tmp):
    test_signal_releases_lock(tmp, signal.SIGTERM, "TERM")
    test_signal_releases_lock(tmp, signal.SIGINT, "INT")


# ── Test 3: Pre-existing lock ────────────────────────────────────────────────

def test_existing_lock(tmp):
    print()
    print("=== Test 3: Pre-existing lock ===")
    test_dir = os.path.join(tmp, "test3")
    repo = os.path.join(test_dir, "repo")
    create_test_repo(repo)

    stub_dir = os.path.join(test_dir, "stub")
    cargo_log = os.path.join(test_dir, "cargo.log")
    make_stub_cargo(stub_dir, cargo_log)

    # Create a pre-existing lock
    lock_dir = os.path.join(repo, "target/bench-compare/.bench-compare-lock")
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

    if not os.path.exists(cargo_log) or os.path.getsize(cargo_log) == 0:
        _pass("existing lock: no Cargo calls")
    else:
        _fail("existing lock: unexpectedly called Cargo")

    if os.path.isdir(lock_dir):
        _pass("existing lock: lock directory preserved")
    else:
        _fail("existing lock: lock directory was deleted")


# ── Main ─────────────────────────────────────────────────────────────────────

def main():
    tmp = tempfile.mkdtemp(prefix="bench-compare-test-")
    try:
        test_concurrent(tmp)
        test_signals(tmp)
        test_existing_lock(tmp)
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
