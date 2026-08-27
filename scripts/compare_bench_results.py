#!/usr/bin/env python3
"""Compare two bench-result-access JSON files and enforce a regression threshold."""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any


def load_results(path: str) -> dict[str, dict[str, Any]]:
    with open(path, encoding="utf-8") as f:
        payload = json.load(f)
    out: dict[str, dict[str, Any]] = {}
    for row in payload.get("results", []):
        out[row["name"]] = row
    return out


def pct_delta(baseline: float, candidate: float) -> float:
    if baseline == 0:
        return 0.0 if candidate == 0 else float("inf")
    return ((candidate - baseline) / baseline) * 100.0


def compare(
    baseline: dict[str, dict[str, Any]],
    candidate: dict[str, dict[str, Any]],
    *,
    threshold_pct: float,
    required: set[str] | None,
) -> tuple[list[dict[str, Any]], list[str]]:
    names = sorted(set(baseline) | set(candidate))
    if required:
        names = [n for n in names if n in required] + [
            n for n in sorted(required) if n not in baseline or n not in candidate
        ]
        # unique preserve order
        seen: set[str] = set()
        ordered: list[str] = []
        for n in names:
            if n not in seen:
                seen.add(n)
                ordered.append(n)
        names = ordered

    rows: list[dict[str, Any]] = []
    failures: list[str] = []
    for name in names:
        if name not in baseline:
            failures.append(f"missing baseline case: {name}")
            continue
        if name not in candidate:
            failures.append(f"missing candidate case: {name}")
            continue
        b = float(baseline[name]["median_ns"])
        c = float(candidate[name]["median_ns"])
        delta = pct_delta(b, c)
        row = {
            "name": name,
            "baseline_median_ns": b,
            "candidate_median_ns": c,
            "delta_pct": delta,
            "threshold_pct": threshold_pct,
            "regress": delta > threshold_pct,
        }
        rows.append(row)
        if row["regress"]:
            failures.append(
                f"{name}: +{delta:.2f}% exceeds +{threshold_pct:.2f}% "
                f"(baseline={b:.1f}ns candidate={c:.1f}ns)"
            )
    return rows, failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline")
    parser.add_argument("candidate")
    parser.add_argument("--threshold", type=float, default=5.0)
    parser.add_argument(
        "--required",
        action="append",
        default=[],
        help="Case name that must be present and within threshold (repeatable).",
    )
    parser.add_argument("--output", default="-")
    args = parser.parse_args()

    baseline = load_results(args.baseline)
    candidate = load_results(args.candidate)
    required = set(args.required) if args.required else None
    rows, failures = compare(baseline, candidate, threshold_pct=args.threshold, required=required)

    payload = {
        "baseline": args.baseline,
        "candidate": args.candidate,
        "threshold_pct": args.threshold,
        "comparisons": rows,
        "ok": not failures,
        "failures": failures,
    }
    text = json.dumps(payload, indent=2)
    if args.output == "-":
        print(text)
    else:
        with open(args.output, "w", encoding="utf-8") as f:
            f.write(text)
            f.write("\n")

    if failures:
        for msg in failures:
            print(msg, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
