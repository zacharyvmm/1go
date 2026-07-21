#!/usr/bin/env python3
"""Calibrated result-access benchmarks for the Python scah binding.

Emits machine-readable JSON. Lookup / property phases never nest store.get()
inside timed field or attribute work.
"""

from __future__ import annotations

import argparse
import json
import platform
import statistics
import sys
import time
from dataclasses import asdict, dataclass
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


def mad(values: list[float], med: float) -> float:
    return statistics.median([abs(v - med) for v in values]) if values else 0.0


def calibrate(fn: Callable[[], None], target_s: float = 0.1) -> int:
    iterations = 1
    while True:
        start = time.perf_counter()
        for _ in range(iterations):
            fn()
        elapsed = time.perf_counter() - start
        if elapsed >= target_s or iterations >= 1_000_000:
            return iterations
        iterations *= 2


def measure(
    name: str,
    fn: Callable[[], None],
    *,
    samples: int,
    iterations: int | None,
    target_s: float,
) -> SampleStats:
    iters = iterations if iterations is not None else calibrate(fn, target_s)
    # Warmup
    for _ in range(min(3, samples)):
        for _ in range(iters):
            fn()

    per_op: list[float] = []
    for _ in range(samples):
        start = time.perf_counter_ns()
        for _ in range(iters):
            fn()
        elapsed = time.perf_counter_ns() - start
        per_op.append(elapsed / iters)

    ordered = sorted(per_op)
    med = statistics.median(ordered)
    return SampleStats(
        name=name,
        median_ns=med,
        min_ns=ordered[0],
        p25_ns=percentile(ordered, 0.25),
        p75_ns=percentile(ordered, 0.75),
        mad_ns=mad(ordered, med),
        iterations=iters,
        samples=samples,
    )


def make_html(n: int) -> str:
    return "".join(f'<a href="/{i}" class="c" id="i{i}" data-k="{i}">t{i}</a>' for i in range(n))


def consume_hits(hits) -> int:
    total = 0
    for el in hits:
        total += len(el.name)
    return total


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=15)
    parser.add_argument("--iterations", default="auto")
    parser.add_argument("--target-ms", type=float, default=100.0)
    parser.add_argument("--output", type=str, default="-")
    parser.add_argument(
        "--smoke",
        action="store_true",
        help="Run every case once with minimal samples (CI correctness).",
    )
    args = parser.parse_args()

    samples = 2 if args.smoke else args.samples
    iterations: int | None
    if args.iterations == "auto":
        iterations = 1 if args.smoke else None
    else:
        iterations = int(args.iterations)
    target_s = 0.01 if args.smoke else args.target_ms / 1000.0

    results: list[SampleStats] = []

    for n in (100, 1_000, 10_000, 100_000):
        if args.smoke and n > 1_000:
            continue
        html = make_html(n)
        q = Query.all("a", Save.all()).build()
        store = parse(html, [q])

        def lookup(store=store):
            hits = store.get("a")
            assert hits is not None
            consume_hits(hits)

        results.append(
            measure(f"lookup_{n}", lookup, samples=samples, iterations=iterations, target_s=target_s)
        )

        hits = store.get("a")
        assert hits is not None

        def iterate(hits=hits):
            for _el in hits:
                pass

        results.append(
            measure(
                f"iterate_{n}",
                iterate,
                samples=samples,
                iterations=iterations,
                target_s=target_s,
            )
        )

        if n >= 1_000:
            subset = hits[:1_000]

            def names(subset=subset):
                for el in subset:
                    _ = el.name

            def ids(subset=subset):
                for el in subset:
                    _ = el.id

            def classes(subset=subset):
                for el in subset:
                    _ = el.class_name

            def texts(subset=subset):
                for el in subset:
                    _ = el.text_content

            def inners(subset=subset):
                for el in subset:
                    _ = el.inner_html

            def one_attr(subset=subset):
                for el in subset:
                    _ = el.get_attribute("href")

            def attrs(subset=subset):
                for el in subset:
                    _ = el.attributes

            def materialize(subset=subset):
                for el in subset:
                    _ = {
                        "name": el.name,
                        "id": el.id,
                        "class": el.class_name,
                        "attributes": el.attributes,
                        "inner_html": el.inner_html,
                        "text_content": el.text_content,
                    }

            for label, fn in (
                (f"field_name_1k_from_{n}", names),
                (f"field_id_1k_from_{n}", ids),
                (f"field_class_1k_from_{n}", classes),
                (f"field_text_1k_from_{n}", texts),
                (f"field_inner_1k_from_{n}", inners),
                (f"field_attr_href_1k_from_{n}", one_attr),
                (f"attrs_1k_from_{n}", attrs),
                (f"materialize_1k_from_{n}", materialize),
            ):
                results.append(
                    measure(label, fn, samples=samples, iterations=iterations, target_s=target_s)
                )

    # Nested lookup
    nested_html = "".join(f"<div><span>c{i}</span></div>" for i in range(1_000 if not args.smoke else 50))
    nested_q = Query.all("div", Save.all()).all("span", Save.all()).build()
    nested_store = parse(nested_html, [nested_q])
    parents = nested_store.get("div")
    assert parents is not None

    def nested_lookup(parents=parents):
        for parent in parents:
            children = parent.get("span")
            assert children

    results.append(
        measure(
            "nested_lookup",
            nested_lookup,
            samples=samples,
            iterations=iterations,
            target_s=target_s,
        )
    )

    # Query construction
    def query_simple():
        Query.all("a", Save.all()).build()

    def query_nested():
        Query.all("div", Save.all()).all("section", Save.none()).all("a", Save.all()).build()

    def query_then():
        (
            Query.all("div", Save.all())
            .then(
                lambda root: [
                    root.all("a", Save.all()),
                    root.all("span", Save.all()),
                    root.all("p", Save.all()),
                ]
            )
            .build()
        )

    for label, fn in (
        ("query_simple", query_simple),
        ("query_nested", query_nested),
        ("query_then", query_then),
    ):
        results.append(
            measure(label, fn, samples=samples, iterations=iterations, target_s=target_s)
        )

    # Parsing
    for label, size in (("parse_10kb", 10_000), ("parse_100kb", 100_000), ("parse_1mb", 1_000_000)):
        if args.smoke and size > 10_000:
            continue
        html = ("<div>" + ("<a>x</a>" * (size // 8)) + "</div>")[:size]
        q = Query.all("a", Save.none()).build()

        def parse_case(html=html, q=q):
            parse(html, [q])

        results.append(
            measure(label, parse_case, samples=samples, iterations=iterations, target_s=target_s)
        )

    payload = {
        "language": "python",
        "binding": "scah",
        "python": sys.version,
        "platform": platform.platform(),
        "machine": platform.machine(),
        "samples": samples,
        "iteration_strategy": "auto-calibrated >= target" if iterations is None else f"fixed={iterations}",
        "target_ms": args.target_ms,
        "results": [asdict(r) for r in results],
    }

    text = json.dumps(payload, indent=2)
    if args.output == "-":
        print(text)
    else:
        with open(args.output, "w", encoding="utf-8") as f:
            f.write(text)
            f.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
