#!/usr/bin/env python3
"""Copy only Criterion saved-baseline measurements to the current target.

Criterion 0.8.1 stores saved baselines under::

    criterion/<group-name>/<benchmark-id>/<baseline-name>/

This helper locates only those measurement directories and copies them,
excluding ``report/``, ``new/``, ``change/``, and unrelated saved baselines.

Symlinks and special filesystem entries in baseline data are rejected.
Only regular files and directories are copied.

Interface::

    python3 scripts/copy-criterion-baseline.py \\
        --source <base-target>/criterion \\
        --destination <current-target>/criterion \\
        --baseline <baseline-name> \\
        [--inventory <inventory-file.jsonl>]

The optional ``--inventory`` flag writes a JSON-lines file with one record
per copied benchmark::

    {"benchmark_path": "parse/foo/100", "baseline_path": "parse/foo/100/main"}

This inventory is the authoritative list of expected benchmark IDs for
downstream validation.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import stat
import sys
from typing import List


class BaselineCopyError(RuntimeError):
    """Raised when baseline copying fails for a recoverable reason."""


def _is_measurement_dir(path: str) -> bool:
    """True when *path* looks like a Criterion saved-baseline measurement dir."""
    estimates = os.path.join(path, "estimates.json")
    return os.path.isfile(estimates)


def _reject_special_tree(root: str) -> None:
    """Reject symlinks and special files anywhere under *root*."""
    for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
        try:
            st_dir = os.lstat(dirpath)
        except OSError as exc:
            raise BaselineCopyError(f"cannot lstat {dirpath}: {exc}") from exc

        if stat.S_ISLNK(st_dir.st_mode):
            raise BaselineCopyError(f"symlink in baseline source tree: {dirpath}")

        # Detect directory entries that are symlinks via scandir when available.
        try:
            with os.scandir(dirpath) as iterator:
                for entry in iterator:
                    try:
                        st = entry.stat(follow_symlinks=False)
                    except OSError as exc:
                        raise BaselineCopyError(
                            f"cannot lstat {entry.path}: {exc}"
                        ) from exc
                    if stat.S_ISLNK(st.st_mode):
                        raise BaselineCopyError(
                            f"symlink in baseline source tree: {entry.path}"
                        )
                    if stat.S_ISFIFO(st.st_mode):
                        raise BaselineCopyError(
                            f"FIFO in baseline source tree: {entry.path}"
                        )
                    if stat.S_ISSOCK(st.st_mode):
                        raise BaselineCopyError(
                            f"socket in baseline source tree: {entry.path}"
                        )
                    if stat.S_ISBLK(st.st_mode) or stat.S_ISCHR(st.st_mode):
                        raise BaselineCopyError(
                            f"device node in baseline source tree: {entry.path}"
                        )
                    if not (stat.S_ISDIR(st.st_mode) or stat.S_ISREG(st.st_mode)):
                        raise BaselineCopyError(
                            f"unsupported entry in baseline source tree: {entry.path}"
                        )
        except BaselineCopyError:
            raise
        except OSError as exc:
            raise BaselineCopyError(f"cannot scan {dirpath}: {exc}") from exc

        # Keep os.walk from following unexpectedly; clear is unnecessary when
        # followlinks=False, but skip symlink dirnames defensively.
        dirnames[:] = [
            name
            for name in dirnames
            if not os.path.islink(os.path.join(dirpath, name))
        ]
        for name in filenames:
            path = os.path.join(dirpath, name)
            if os.path.islink(path):
                raise BaselineCopyError(f"symlink in baseline source tree: {path}")


def _copy_regular_tree(source: str, destination: str) -> None:
    """Copy a baseline directory tree containing only regular files/dirs."""
    _reject_special_tree(source)
    # symlinks=False copies file contents; ignore is unused because we already
    # rejected specials. dirs_exist_ok is False so collisions remain errors.
    shutil.copytree(source, destination, symlinks=False)


def copy_baseline(
    source: str,
    destination: str,
    baseline_name: str,
) -> List[dict]:
    """Copy saved-baseline measurements from *source* to *destination*.

    Returns a list of dicts with ``benchmark_path`` and ``baseline_path``.

    Raises ``BaselineCopyError`` on failure.
    """
    source = os.path.abspath(source)
    destination = os.path.abspath(destination)
    entries: List[dict] = []

    if not os.path.isdir(source):
        raise BaselineCopyError(f"source directory does not exist: {source}")

    os.makedirs(destination, exist_ok=True)

    for dirpath, dirnames, _filenames in os.walk(source, followlinks=False):
        rel_dir = os.path.relpath(dirpath, source)
        if (
            rel_dir == "report"
            or rel_dir.startswith("report/")
            or rel_dir == "new"
            or rel_dir.startswith("new/")
            or rel_dir == "change"
            or rel_dir.startswith("change/")
        ):
            continue

        if os.path.islink(dirpath):
            raise BaselineCopyError(f"symlink in baseline source tree: {dirpath}")

        if os.path.basename(dirpath) != baseline_name:
            continue

        if not _is_measurement_dir(dirpath):
            continue

        rel = os.path.relpath(dirpath, source)
        dst = os.path.join(destination, rel)

        dst_abs = os.path.abspath(dst)
        try:
            if os.path.commonpath([dst_abs, destination]) != destination:
                raise BaselineCopyError(f"destination escapes target: {rel}")
        except ValueError as exc:
            raise BaselineCopyError(f"destination escapes target: {rel}") from exc

        if os.path.exists(dst):
            raise BaselineCopyError(f"destination already exists: {rel}")

        if os.path.islink(dirpath):
            raise BaselineCopyError(f"source baseline directory is a symlink: {rel}")

        os.makedirs(os.path.dirname(dst), exist_ok=True)
        _copy_regular_tree(dirpath, dst)

        benchmark_path = os.path.dirname(rel)
        entries.append({"benchmark_path": benchmark_path, "baseline_path": rel})

        dirnames.clear()

    if not entries:
        raise BaselineCopyError(
            f"no saved-baseline measurement directories found for "
            f"baseline '{baseline_name}' in {source}"
        )

    return entries


# ── CLI ──────────────────────────────────────────────────────────────────────


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Copy Criterion saved-baseline measurements only."
    )
    parser.add_argument("--source", required=True, help="Baseline Criterion directory")
    parser.add_argument("--destination", required=True, help="Current Criterion directory")
    parser.add_argument("--baseline", required=True, help="Saved-baseline name")
    parser.add_argument("--inventory", default=None, help="Optional JSONL inventory output")
    args = parser.parse_args()

    try:
        entries = copy_baseline(args.source, args.destination, args.baseline)
    except BaselineCopyError as exc:
        print(f"error: {exc}", file=sys.stderr)
        sys.exit(1)

    print(f"copied {len(entries)} saved-baseline measurement directories:")
    for entry in entries:
        print(f"  {entry['baseline_path']}")

    if args.inventory:
        with open(args.inventory, "w", encoding="utf-8") as fh:
            for entry in entries:
                json.dump(entry, fh, sort_keys=True)
                fh.write("\n")


if __name__ == "__main__":
    main()
