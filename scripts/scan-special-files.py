#!/usr/bin/env python3
"""Reject unsupported special files in a Git worktree before benchmarking.

``git ls-files --others`` omits pure FIFOs and may miss other special entries.
Walk the worktree like source-fingerprint (skip ``.git``, honour ``.gitignore``
via ``git check-ignore``, skip ignored directories without descending) and fail
when a non-ignored FIFO, socket, or device is present.
"""

from __future__ import annotations

import argparse
import os
import stat
import subprocess
import sys
from typing import List, Tuple


def _is_git_metadata(rel_path: str) -> bool:
    return rel_path == ".git" or rel_path.startswith(f".git{os.sep}")


def _special_label(mode: int) -> str | None:
    if stat.S_ISFIFO(mode):
        return "FIFO"
    if stat.S_ISSOCK(mode):
        return "socket"
    if stat.S_ISBLK(mode):
        return "block device"
    if stat.S_ISCHR(mode):
        return "character device"
    return None


def _is_ignored(root: str, rel_path: str) -> bool:
    proc = subprocess.run(
        ["git", "-C", root, "check-ignore", "-q", "--", rel_path],
        capture_output=True,
    )
    if proc.returncode == 0:
        return True
    if proc.returncode == 1:
        return False
    err = proc.stderr.decode("utf-8", errors="replace").strip()
    raise OSError(f"git check-ignore failed for {rel_path}: {err or proc.returncode}")


def _scan_directory(
    root: str, rel_dir: str, findings: List[Tuple[str, str]]
) -> None:
    dir_path = root if not rel_dir else os.path.join(root, rel_dir)

    try:
        with os.scandir(dir_path) as iterator:
            entries = list(iterator)
    except OSError as exc:
        raise OSError(f"cannot scan directory {rel_dir or '.'}: {exc}") from exc

    entries.sort(key=lambda entry: os.fsencode(entry.name))

    for entry in entries:
        rel_path = entry.name if not rel_dir else os.path.join(rel_dir, entry.name)

        if _is_git_metadata(rel_path):
            continue

        if _is_ignored(root, rel_path):
            continue

        try:
            st = entry.stat(follow_symlinks=False)
        except OSError as exc:
            raise OSError(f"cannot stat {rel_path}: {exc}") from exc

        label = _special_label(st.st_mode)
        if label is not None:
            findings.append((label, rel_path))

        if stat.S_ISDIR(st.st_mode) and not stat.S_ISLNK(st.st_mode):
            _scan_directory(root, rel_path, findings)


def scan_worktree(root: str) -> list[tuple[str, str]]:
    root = os.path.abspath(root)
    findings: list[tuple[str, str]] = []
    _scan_directory(root, "", findings)
    return findings


def write_manifest(path: str, findings: list[tuple[str, str]]) -> None:
    """Write TYPE\\0RELPATH\\0 records sorted by encoded relative path."""
    manifest_dir = os.path.dirname(os.path.abspath(path)) or "."
    os.makedirs(manifest_dir, exist_ok=True)
    sorted_findings = sorted(findings, key=lambda item: os.fsencode(item[1]))
    payload = b"".join(
        item[0].encode("utf-8") + b"\0" + os.fsencode(item[1]) + b"\0"
        for item in sorted_findings
    )
    with open(path, "wb") as fh:
        fh.write(payload)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Reject unsupported special files in a Git worktree."
    )
    parser.add_argument("--root", required=True, help="Repository root")
    parser.add_argument(
        "--manifest",
        help="Write special-file manifest (empty when clean)",
    )
    args = parser.parse_args()

    try:
        findings = scan_worktree(args.root)
    except OSError as exc:
        print(f"error: {exc}", file=sys.stderr)
        sys.exit(1)

    if args.manifest:
        write_manifest(args.manifest, findings)

    if not findings:
        return

    for label, rel_path in findings:
        print(f"error: unsupported entry type {label}: {rel_path}", file=sys.stderr)
    sys.exit(1)


if __name__ == "__main__":
    main()
