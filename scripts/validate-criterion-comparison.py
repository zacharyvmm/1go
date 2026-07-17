#!/usr/bin/env python3
"""Validate that every expected Criterion benchmark produced fresh output.

Reads the baseline inventory (produced by ``copy-criterion-baseline.py
--inventory``) and checks that every expected benchmark path under the
current Criterion root has:

- ``new/estimates.json`` — a fresh current measurement ran
- ``new/sample.json`` — current sample data exists
- ``change/estimates.json`` — Criterion compared current against baseline

Interface::

    python3 scripts/validate-criterion-comparison.py \\
        --criterion-root <current-target>/criterion \\
        --inventory <baseline-inventory.jsonl>

Exit status:
  0 — all expected benchmarks are complete
  1 — one or more expected benchmarks are incomplete or missing
"""

import argparse
import json
import os
import sys
from typing import List


REQUIRED_FILES = [
    ("new/estimates.json", "fresh current measurement"),
    ("new/sample.json", "current sample data"),
    ("change/estimates.json", "comparison estimates"),
]


def _load_inventory(path: str) -> List[dict]:
    """Load the baseline inventory JSONL file."""
    entries: List[dict] = []
    with open(path, "r", encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            entries.append(json.loads(line))
    return entries


def validate(criterion_root: str, inventory_path: str) -> int:
    """Validate Criterion output against the baseline inventory.

    Returns 0 on success, 1 on failure.
    """
    criterion_root = os.path.abspath(criterion_root)

    if not os.path.isdir(criterion_root):
        print("error: criterion root does not exist", file=sys.stderr)
        return 1

    entries = _load_inventory(inventory_path)

    if not entries:
        print("error: baseline inventory is empty — no benchmarks were copied", file=sys.stderr)
        return 1

    errors: List[str] = []

    for entry in entries:
        benchmark_path = entry["benchmark_path"]
        bench_dir = os.path.join(criterion_root, benchmark_path)

        missing: List[str] = []
        for rel_file, description in REQUIRED_FILES:
            full_path = os.path.join(bench_dir, rel_file)
            if not os.path.isfile(full_path):
                missing.append(f"{rel_file} ({description})")

        if missing:
            errors.append(
                f"error: Criterion comparison incomplete for:\n"
                f"  {benchmark_path}\n"
                f"missing:\n"
                + "".join(f"  {m}\n" for m in missing).rstrip()
            )

    if errors:
        for err in errors:
            print(err, file=sys.stderr)
        print(
            f"\nerror: {len(errors)} of {len(entries)} benchmark(s) "
            f"have incomplete or missing Criterion output",
            file=sys.stderr,
        )
        return 1

    print(
        f"All {len(entries)} expected benchmark(s) produced complete "
        f"Criterion output"
    )
    return 0


# ── CLI ──────────────────────────────────────────────────────────────────────


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Validate Criterion comparison output"
    )
    parser.add_argument(
        "--criterion-root",
        required=True,
        help="Current Criterion root directory",
    )
    parser.add_argument(
        "--inventory",
        required=True,
        help="Baseline inventory JSONL file",
    )
    args = parser.parse_args()

    rc = validate(args.criterion_root, args.inventory)
    sys.exit(rc)


if __name__ == "__main__":
    main()
