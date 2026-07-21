#!/usr/bin/env python3
"""Focused Python gate benchmarks (low wall-clock)."""

from __future__ import annotations

import argparse
import json
import platform
import statistics
import sys
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Callable

from scah import Query, Save, parse


@dataclass
class SampleStats:
    name: str
    median_ns: float
    min_ns: float
    p25_ns: float
    p75_ns: float
    mad_ns: float
    iterations: int
    samples: int


def percentile(sorted_vals: list[float], p: float) -> float:
    if not sorted_vals:
        return 0.0
    k = (len(sorted_vals) - 1) * p
    f = int(k)
    c = min(f + 1, len(sorted_vals) - 1)
    if f == c:
        return sorted_vals[f]
    return sorted_vals[f] + (sorted_vals[c] - sorted_vals[f]) * (k - f)


def calibrate(fn: Callable[[], None], target_s: float = 0.05) -> int:
    iterations = 1
    while True:
        start = time.perf_counter()
        for _ in range(iterations):
            fn()
        elapsed = time.perf_counter() - start
        if elapsed >= target_s or iterations >= 500_000:
            return iterations
        iterations *= 2


def measure(name: str, fn: Callable[[], None], samples: int, target_s: float) -> SampleStats:
    iters = calibrate(fn, target_s)
    for _ in range(2):
        for _ in range(iters):
            fn()
    per_op: list[float] = []
    for _ in range(samples):
        start = time.perf_counter_ns()
        for _ in range(iters):
            fn()
        per_op.append((time.perf_counter_ns() - start) / iters)
    ordered = sorted(per_op)
    med = statistics.median(ordered)
    return SampleStats(
        name=name,
        median_ns=med,
        min_ns=ordered[0],
        p25_ns=percentile(ordered, 0.25),
        p75_ns=percentile(ordered, 0.75),
        mad_ns=statistics.median([abs(v - med) for v in ordered]),
        iterations=iters,
        samples=samples,
    )


def make_html(n: int) -> str:
    return "".join(f'<a href="/{i}" class="c" id="i{i}">t{i}</a>' for i in range(n))


def consume_hits(hits) -> int:
    total = 0
    for el in hits:
        total += len(el.name)
    return total


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--samples", type=int, default=7)
    ap.add_argument("--output", required=True)
    ap.add_argument("--skip-100k", action="store_true")
    args = ap.parse_args()
    target = 0.05
    results: list[SampleStats] = []

    for n in [100, 1_000, 10_000] + ([] if args.skip_100k else [100_000]):
        store = parse(make_html(n), [Query.all("a", Save.all()).build()])

        def lookup(store=store):
            hits = store.get("a")
            assert hits is not None
            consume_hits(hits)

        results.append(measure(f"lookup_{n}", lookup, args.samples, target))
        print(f"done lookup_{n}", flush=True)

        if n == 1_000:
            subset = store.get("a")[:1_000]

            def names(subset=subset):
                for el in subset:
                    _ = el.name

            def attrs(subset=subset):
                for el in subset:
                    _ = el.attributes

            results.append(measure("field_name_1k_from_1000", names, args.samples, target))
            results.append(measure("attrs_1k_from_1000", attrs, args.samples, target))
            print("done fields/attrs", flush=True)

    parents = parse(
        "".join(f"<div><span>c{i}</span></div>" for i in range(500)),
        [Query.all("div", Save.all()).all("span", Save.all()).build()],
    ).get("div")
    assert parents is not None

    def nested(parents=parents):
        for p in parents:
            assert p.get("span")

    results.append(measure("nested_lookup", nested, args.samples, target))
    print("done nested", flush=True)

    for label, size in (("parse_10kb", 10_000), ("parse_100kb", 100_000), ("parse_1mb", 1_000_000)):
        html = ("<div>" + ("<a>x</a>" * (size // 8)) + "</div>")[:size]
        q = Query.all("a", Save.none()).build()

        def parse_case(html=html, q=q):
            parse(html, [q])

        results.append(measure(label, parse_case, args.samples, target))
        print(f"done {label}", flush=True)

    payload = {
        "language": "python",
        "python": sys.version,
        "platform": platform.platform(),
        "machine": platform.machine(),
        "samples": args.samples,
        "results": [asdict(r) for r in results],
    }
    Path(args.output).write_text(json.dumps(payload, indent=2) + "\n")
    print(f"wrote {args.output} ({len(results)} cases)", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
