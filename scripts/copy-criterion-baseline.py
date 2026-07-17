#!/usr/bin/env python3
"""Copy only Criterion saved-baseline measurements to the current target.

Criterion 0.8.1 stores saved baselines under::

    criterion/<group-name>/<benchmark-id>/<baseline-name>/

This helper locates only those measurement directories and copies them,
excluding ``report/``, ``new/``, ``change/``, and unrelated saved baselines.

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

import argparse
import json
import os
import shutil
import sys
from typing import List


class BaselineCopyError(RuntimeError):
    """Raised when baseline copying fails for a recoverable reason."""


def _is_measurement_dir(path: str) -> bool:
    """True when *path* looks like a Criterion saved-baseline measurement dir."""
    estimates = os.path.join(path, "estimates.json")
    return os.path.isfile(estimates)


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

    for dirpath, dirnames, _filenames in os.walk(source):
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
        except ValueError:
            raise BaselineCopyError(f"destination escapes target: {rel}")

        if os.path.exists(dst):
            raise BaselineCopyError(f"destination already exists: {rel}")

        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copytree(dirpath, dst, symlinks=True)

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
