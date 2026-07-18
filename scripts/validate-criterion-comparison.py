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

from __future__ import annotations

import argparse
import json
import os
import sys
from typing import List, Set


REQUIRED_FILES = [
    ("new/estimates.json", "fresh current measurement"),
    ("new/sample.json", "current sample data"),
    ("change/estimates.json", "comparison estimates"),
]


class ComparisonValidationError(RuntimeError):
    """Raised when inventory records are malformed."""


def _normalize_relative(path: str, field: str) -> str:
    if not isinstance(path, str) or not path:
        raise ComparisonValidationError(f"inventory {field} must be a non-empty string")
    if os.path.isabs(path):
        raise ComparisonValidationError(f"inventory {field} must be relative: {path!r}")

    normalized = os.path.normpath(path)
    if normalized in ("", "."):
        raise ComparisonValidationError(
            f"inventory {field} is empty after normalization"
        )
    if normalized.startswith("..") or f"{os.sep}.." in f"{os.sep}{normalized}{os.sep}":
        raise ComparisonValidationError(
            f"inventory {field} escapes with '..': {path!r}"
        )
    return normalized


def _load_inventory(path: str) -> List[dict]:
    """Load and validate the baseline inventory JSONL file."""
    entries: List[dict] = []
    seen_benchmarks: Set[str] = set()

    with open(path, "r", encoding="utf-8") as fh:
        for line_no, raw in enumerate(fh, start=1):
            line = raw.strip()
            if not line:
                continue

            try:
                entry = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ComparisonValidationError(
                    f"inventory line {line_no}: malformed JSON: {exc}"
                ) from exc

            if not isinstance(entry, dict):
                raise ComparisonValidationError(
                    f"inventory line {line_no}: expected a JSON object"
                )

            if "benchmark_path" not in entry:
                raise ComparisonValidationError(
                    f"inventory line {line_no}: missing benchmark_path"
                )

            benchmark_path = _normalize_relative(
                entry["benchmark_path"], "benchmark_path"
            )

            if "baseline_path" in entry:
                baseline_path = _normalize_relative(
                    entry["baseline_path"], "baseline_path"
                )
                if os.path.dirname(baseline_path) != benchmark_path:
                    raise ComparisonValidationError(
                        f"inventory line {line_no}: dirname(baseline_path) "
                        f"!= benchmark_path"
                    )
            else:
                baseline_path = None

            if benchmark_path in seen_benchmarks:
                raise ComparisonValidationError(
                    f"inventory line {line_no}: duplicate benchmark_path "
                    f"{benchmark_path!r}"
                )
            seen_benchmarks.add(benchmark_path)

            records = {"benchmark_path": benchmark_path}
            if baseline_path is not None:
                records["baseline_path"] = baseline_path
            entries.append(records)

    return entries


def _ensure_under_root(root: str, candidate: str, label: str) -> str:
    abs_root = os.path.abspath(root)
    abs_candidate = os.path.abspath(candidate)
    try:
        if os.path.commonpath([abs_root, abs_candidate]) != abs_root:
            raise ComparisonValidationError(
                f"{label} escapes criterion root: {candidate}"
            )
    except ValueError as exc:
        raise ComparisonValidationError(
            f"{label} escapes criterion root: {candidate}"
        ) from exc
    return abs_candidate


def validate(criterion_root: str, inventory_path: str) -> int:
    """Validate Criterion output against the baseline inventory.

    Returns 0 on success, 1 on failure.
    """
    criterion_root = os.path.abspath(criterion_root)

    if not os.path.isdir(criterion_root):
        print("error: criterion root does not exist", file=sys.stderr)
        return 1

    try:
        entries = _load_inventory(inventory_path)
    except ComparisonValidationError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    except OSError as exc:
        print(f"error: cannot read inventory: {exc}", file=sys.stderr)
        return 1

    if not entries:
        print(
            "error: baseline inventory is empty — no benchmarks were copied",
            file=sys.stderr,
        )
        return 1

    errors: List[str] = []

    for entry in entries:
        benchmark_path = entry["benchmark_path"]
        bench_dir = _ensure_under_root(
            criterion_root,
            os.path.join(criterion_root, benchmark_path),
            "benchmark_path",
        )

        missing: List[str] = []
        for rel_file, description in REQUIRED_FILES:
            full_path = os.path.join(bench_dir, rel_file)
            _ensure_under_root(criterion_root, full_path, rel_file)
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
