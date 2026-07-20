#!/usr/bin/env python3
"""Microbenchmarks for result-access paths (lookup / attrs / fields)."""

from __future__ import annotations

import statistics
import time
from typing import Callable

from scah import Query, Save, parse


def make_html(n: int) -> str:
    return "".join(f'<a href="/{i}" class="c" id="i{i}">t{i}</a>' for i in range(n))


def timed_median(fn: Callable[[], None], samples: int = 5, warmup: int = 2) -> float:
    for _ in range(warmup):
        fn()
    times: list[float] = []
    for _ in range(samples):
        start = time.perf_counter()
        fn()
        times.append(time.perf_counter() - start)
    return statistics.median(times)


def main() -> None:
    cases: list[tuple[str, Callable[[], None]]] = []

    for n in (100, 1_000, 10_000):
        html = make_html(n)
        q = Query.all("a", Save.all()).build()
        store = parse(html, [q])

        def lookup(store=store):
            hits = store.get("a")
            assert hits is not None

        def iterate_names(store=store):
            hits = store.get("a")
            assert hits is not None
            for el in hits:
                _ = el.name

        def attrs(store=store):
            hits = store.get("a")
            assert hits is not None
            # Materialize attributes for up to 1_000 elements.
            for el in hits[:1_000]:
                _ = el.attributes

        def fields(store=store):
            hits = store.get("a")
            assert hits is not None
            for el in hits[:1_000]:
                _ = el.name
                _ = el.id
                _ = el.class_name
                _ = el.inner_html
                _ = el.text_content

        cases.append((f"lookup_{n}", lookup))
        if n == 10_000:
            cases.append((f"iterate_name_{n}", iterate_names))
        if n >= 1_000:
            cases.append((f"attrs_1k_from_{n}", attrs))
            cases.append((f"fields_1k_from_{n}", fields))

    # Nested query construction
    def nested_query():
        Query.all("div", Save.all()).all("section", Save.none()).all("a", Save.all()).build()

    def then_query():
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

    cases.append(("query_nested", nested_query))
    cases.append(("query_then", then_query))

    # Parse sizes
    for label, size in (("parse_10kb", 10_000), ("parse_100kb", 100_000), ("parse_1mb", 1_000_000)):
        html = ("<div>" + ("<a>x</a>" * (size // 8)) + "</div>")[:size]
        q = Query.all("a", Save.none()).build()

        def parse_case(html=html, q=q):
            parse(html, [q])

        cases.append((label, parse_case))

    print("case,median_seconds")
    for name, fn in cases:
        median = timed_median(fn)
        print(f"{name},{median:.9f}")


if __name__ == "__main__":
    main()
