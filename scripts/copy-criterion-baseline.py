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
        --baseline <baseline-name>

Fails when:
  - No matching saved-baseline measurement directories exist.
  - A source path has an unexpected shape.
  - Destination paths would escape the target.
  - A current-output directory already exists unexpectedly.
"""

import argparse
import os
import shutil
import sys


def _is_measurement_dir(path: str) -> bool:
    """True when *path* looks like a Criterion saved-baseline measurement dir."""
    estimates = os.path.join(path, "estimates.json")
    return os.path.isfile(estimates)
def copy_baseline(
    source: str, destination: str, baseline_name: str
) -> list[str]:
    """Copy saved-baseline measurements from *source* to *destination*.

    Walks the Criterion directory tree recursively. Any directory named
    *baseline_name* containing an ``estimates.json`` file is treated as a
    saved-baseline measurement and copied, preserving the relative path under
    *source*.

    Returns the list of copied relative paths.
    """
    source = os.path.abspath(source)
    destination = os.path.abspath(destination)
    copied: list[str] = []

    if not os.path.isdir(source):
        print(f"error: source directory does not exist: {source}", file=sys.stderr)
        sys.exit(1)

    os.makedirs(destination, exist_ok=True)

    for dirpath, dirnames, _filenames in os.walk(source):
        # Skip the report/, new/, change/ directories entirely.
        rel_dir = os.path.relpath(dirpath, source)
        if rel_dir == "report" or rel_dir.startswith("report/") or \
           rel_dir == "new" or rel_dir.startswith("new/") or \
           rel_dir == "change" or rel_dir.startswith("change/"):
            continue

        # Check if this directory is a saved-baseline measurement.
        if os.path.basename(dirpath) != baseline_name:
            continue

        if not _is_measurement_dir(dirpath):
            continue

        # Compute the relative path from source.
        rel = os.path.relpath(dirpath, source)
        dst = os.path.join(destination, rel)

        # Sanity: destination must be under the destination root.
        dst_abs = os.path.abspath(dst)
        try:
            if os.path.commonpath([dst_abs, destination]) != destination:
                print(f"error: destination escapes target: {rel}", file=sys.stderr)
                sys.exit(1)
        except ValueError:
            print(f"error: destination escapes target: {rel}", file=sys.stderr)
            sys.exit(1)

        if os.path.exists(dst):
            print(
                f"error: destination already exists: {rel}",
                file=sys.stderr,
            )
            sys.exit(1)

        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copytree(dirpath, dst, symlinks=True)
        copied.append(rel)

        # Don't descend into the baseline directory (it's the leaf).
        dirnames.clear()

    if not copied:
        print(
            f"error: no saved-baseline measurement directories found for "
            f"baseline '{baseline_name}' in {source}",
            file=sys.stderr,
        )
        sys.exit(1)

    return copied


# ── CLI ──────────────────────────────────────────────────────────────────────


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Copy Criterion saved-baseline measurements only."
    )
    parser.add_argument(
        "--source",
        required=True,
        help="Baseline Criterion directory (e.g. <base-target>/criterion)",
    )
    parser.add_argument(
        "--destination",
        required=True,
        help="Current Criterion directory (e.g. <current-target>/criterion)",
    )
    parser.add_argument(
        "--baseline",
        required=True,
        help="Saved-baseline name (e.g. 'main')",
    )
    args = parser.parse_args()

    copied = copy_baseline(args.source, args.destination, args.baseline)
    print(f"copied {len(copied)} saved-baseline measurement directories:")
    for path in copied:
        print(f"  {path}")


if __name__ == "__main__":
    main()
