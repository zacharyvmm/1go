mod support;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse};
use std::hint::black_box;
use support::fixtures::{
    generate_first_match_html, generate_link_list_html, generate_multi_query_html,
    generate_product_catalog_html,
};
use support::validation::{
    assert_first_match_result, assert_has_href_attribute, assert_multi_query_results,
    assert_product_catalog_all, assert_product_catalog_first, assert_save_all_result,
    assert_save_inner_html_result, assert_save_none_result, assert_save_text_result,
};

// ── Bench sizes ────────────────────────────────────────────────────────────

/// Sizes used for synthetic link benchmarks under the full profile.
const SYNTHETIC_SIZES_FULL: &[usize] = &[100, 1_000, 10_000];
/// Sizes used for synthetic link benchmarks under the quick profile.
const SYNTHETIC_SIZES_QUICK: &[usize] = &[100, 1_000];
/// Product catalog sizes.
const PRODUCT_SIZES: &[usize] = &[100, 1_000];
/// First-match element count.
const FIRST_MATCH_COUNT: usize = 10_000;
/// Multi-query element count.
const MULTI_QUERY_COUNT: usize = 1_000;
/// Multi-query query counts.
const MULTI_QUERY_COUNTS: &[usize] = &[1, 4, 16, 32];

fn synthetic_sizes() -> &'static [usize] {
    if is_quick() {
        SYNTHETIC_SIZES_QUICK
    } else {
        SYNTHETIC_SIZES_FULL
    }
}

fn is_quick() -> bool {
    std::env::var("SCAH_BENCH_PROFILE").as_deref() == Ok("quick")
}

// ── Query builder helpers (not timed — used outside measured loops) ────────

/// Build a nested "all" query with the given selectors.
///
/// This is the single authoritative builder for nested-all queries. All
/// prebuilt, query-construction, end-to-end, and validation code paths use
/// this function so the query structure cannot drift between validation and
/// timed code.
fn build_nested_all_query_with(
    product_selector: &'static str,
    title_selector: &'static str,
    rating_selector: &'static str,
    description_selector: &'static str,
) -> Query<'static> {
    Query::all(product_selector, Save::all())
        .expect("parent selector should parse")
        .then(|product| {
            Ok([
                product.all(title_selector, Save::all())?,
                product.all(rating_selector, Save::all())?,
                product.all(description_selector, Save::all())?,
            ])
        })
        .expect("child selectors should parse")
        .build()
}

/// Build a nested "first" query with the given selectors.
fn build_nested_first_query_with(
    product_selector: &'static str,
    title_selector: &'static str,
    rating_selector: &'static str,
    description_selector: &'static str,
) -> Query<'static> {
    Query::first(product_selector, Save::all())
        .expect("parent selector should parse")
        .then(|product| {
            Ok([
                product.first(title_selector, Save::all())?,
                product.first(rating_selector, Save::all())?,
                product.first(description_selector, Save::all())?,
            ])
        })
        .expect("child selectors should parse")
        .build()
}

/// Convenience wrapper using the default product catalog selectors.
fn build_nested_all_query() -> Query<'static> {
    build_nested_all_query_with(
        PRODUCT_SELECTOR,
        PRODUCT_TITLE_SELECTOR,
        PRODUCT_RATING_SELECTOR,
        PRODUCT_DESCRIPTION_SELECTOR,
    )
}

/// Convenience wrapper using the default product catalog selectors.
fn build_nested_first_query() -> Query<'static> {
    build_nested_first_query_with(
        PRODUCT_SELECTOR,
        PRODUCT_TITLE_SELECTOR,
        PRODUCT_RATING_SELECTOR,
        PRODUCT_DESCRIPTION_SELECTOR,
    )
}

/// Build a "save_all" synthetic-link query with the given selector.
///
/// This is the single authoritative builder for synthetic-link Save::all()
/// queries. All prebuilt, consume, end-to-end, and validation code paths use
/// this function so the query structure cannot drift between validation and
/// timed code.
fn build_link_all_query(selector: &'static str) -> Query<'static> {
    Query::all(selector, Save::all())
        .expect("link selector should parse")
        .build()
}

// ── Query building benchmarks ──────────────────────────────────────────────

fn bench_query_building(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_build");

    // --- simple/all ---
    group.bench_function("simple/all", |b| {
        b.iter(|| {
            let query = Query::all(black_box("a"), Save::all())
                .expect("selector should parse")
                .build();
            black_box(query);
        })
    });

    // --- simple/first ---
    group.bench_function("simple/first", |b| {
        b.iter(|| {
            let query = Query::first(black_box("a"), Save::all())
                .expect("selector should parse")
                .build();
            black_box(query);
        })
    });

    // --- nested/all ---
    group.bench_function("nested/all", |b| {
        b.iter(|| {
            let query = build_nested_all_query_with(
                black_box(PRODUCT_SELECTOR),
                black_box(PRODUCT_TITLE_SELECTOR),
                black_box(PRODUCT_RATING_SELECTOR),
                black_box(PRODUCT_DESCRIPTION_SELECTOR),
            );
            black_box(query);
        })
    });

    // --- nested/first ---
    group.bench_function("nested/first", |b| {
        b.iter(|| {
            let query = build_nested_first_query_with(
                black_box(PRODUCT_SELECTOR),
                black_box(PRODUCT_TITLE_SELECTOR),
                black_box(PRODUCT_RATING_SELECTOR),
                black_box(PRODUCT_DESCRIPTION_SELECTOR),
            );
            black_box(query);
        })
    });

    group.finish();
}

// ── Synthetic link parsing benchmarks ──────────────────────────────────────

const LINK_SELECTOR: &str = "a";

fn consume_link_results(store: &scah::Store<'_, '_>) {
    if let Some(elements) = store.get(LINK_SELECTOR) {
        for element in elements {
            black_box(element.attribute(store, "href"));
            black_box(&element.inner_html);
            black_box(&element.text_content(store));
        }
    }
}

fn bench_synthetic_links(c: &mut Criterion) {
    for &size in synthetic_sizes() {
        let html = generate_link_list_html(size);
        let throughput = Throughput::Bytes(html.len() as u64);

        // --- Prebuilt, save_none ---
        {
            let mut group = c.benchmark_group("parse/synthetic_links/prebuilt/all/save_none");
            group.throughput(throughput.clone());
            let queries = &[Query::all(LINK_SELECTOR, Save::none())
                .expect("selector should parse")
                .build()];
            let store = parse(&html, queries).unwrap();
            assert_save_none_result(&store, LINK_SELECTOR, size);

            group.bench_with_input(BenchmarkId::from_parameter(size), &html, |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(queries)).unwrap();
                    black_box(store);
                })
            });
            group.finish();
        }

        // --- Prebuilt, save_inner_html ---
        {
            let mut group = c.benchmark_group("parse/synthetic_links/prebuilt/all/save_inner_html");
            group.throughput(throughput.clone());
            let queries = &[Query::all(LINK_SELECTOR, Save::only_inner_html())
                .expect("selector should parse")
                .build()];
            let store = parse(&html, queries).unwrap();
            assert_save_inner_html_result(&store, LINK_SELECTOR, size);

            group.bench_with_input(BenchmarkId::from_parameter(size), &html, |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(queries)).unwrap();
                    black_box(store);
                })
            });
            group.finish();
        }

        // --- Prebuilt, save_text ---
        {
            let mut group = c.benchmark_group("parse/synthetic_links/prebuilt/all/save_text");
            group.throughput(throughput.clone());
            let queries = &[Query::all(LINK_SELECTOR, Save::only_text_content())
                .expect("selector should parse")
                .build()];
            let store = parse(&html, queries).unwrap();
            assert_save_text_result(&store, LINK_SELECTOR, size);

            group.bench_with_input(BenchmarkId::from_parameter(size), &html, |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(queries)).unwrap();
                    black_box(store);
                })
            });
            group.finish();
        }

        // --- Prebuilt, save_all ---
        {
            let mut group = c.benchmark_group("parse/synthetic_links/prebuilt/all/save_all");
            group.throughput(throughput.clone());
            let queries = &[build_link_all_query(LINK_SELECTOR)];
            let store = parse(&html, queries).unwrap();
            assert_save_all_result(&store, LINK_SELECTOR, size);

            group.bench_with_input(BenchmarkId::from_parameter(size), &html, |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(queries)).unwrap();
                    black_box(store);
                })
            });
            group.finish();
        }

        // --- Consume (parse + full result traversal) ---
        {
            let mut group = c.benchmark_group("parse/synthetic_links/consume/all/save_all");
            group.throughput(throughput.clone());
            let queries = &[build_link_all_query(LINK_SELECTOR)];
            let store = parse(&html, queries).unwrap();
            assert_save_all_result(&store, LINK_SELECTOR, size);
            assert_has_href_attribute(&store, LINK_SELECTOR);

            group.bench_with_input(BenchmarkId::from_parameter(size), &html, |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(queries)).unwrap();
                    consume_link_results(black_box(&store));
                })
            });
            group.finish();
        }

        // --- End-to-end (query construction + parse + consume) ---
        {
            let mut group = c.benchmark_group("parse/synthetic_links/end_to_end/all/save_all");
            group.throughput(throughput.clone());

            // Validate the same builder outside the timed loop
            let validation_queries = &[build_link_all_query(LINK_SELECTOR)];
            let validation_store = parse(&html, validation_queries).unwrap();
            assert_save_all_result(&validation_store, LINK_SELECTOR, size);
            assert_has_href_attribute(&validation_store, LINK_SELECTOR);

            group.bench_with_input(BenchmarkId::from_parameter(size), &html, |b, html| {
                b.iter(|| {
                    let queries = &[build_link_all_query(black_box(LINK_SELECTOR))];
                    let store = parse(black_box(html), black_box(queries)).unwrap();
                    consume_link_results(black_box(&store));
                })
            });
            group.finish();
        }
    }
}

// ── First-match placement benchmarks ───────────────────────────────────────
//
// These benchmarks report **latency** rather than full-document byte throughput
// because `Query::first` can exit early once a match is found. Reporting bytes
// processed would be misleading: early/middle/late cases may not scan the
// entire document.

const FIRST_MATCH_SELECTOR: &str = "a.target";

fn bench_first_match(c: &mut Criterion) {
    let count = FIRST_MATCH_COUNT;

    // --- early ---
    {
        let mut group = c.benchmark_group("parse/first_match/early");
        // No group.throughput(...) — first-match measures latency, not throughput
        let html = generate_first_match_html(count, Some(0));
        let queries = &[Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build()];
        let store = parse(&html, queries).unwrap();
        assert_first_match_result(&store, FIRST_MATCH_SELECTOR, Some(0));

        group.bench_with_input(BenchmarkId::from_parameter(count), &html, |b, html| {
            b.iter(|| {
                let store = parse(black_box(html), black_box(queries)).unwrap();
                black_box(store);
            })
        });
        group.finish();
    }

    // --- middle ---
    {
        let mut group = c.benchmark_group("parse/first_match/middle");
        // No group.throughput(...)
        let mid = count / 2;
        let html = generate_first_match_html(count, Some(mid));
        let queries = &[Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build()];
        let store = parse(&html, queries).unwrap();
        assert_first_match_result(&store, FIRST_MATCH_SELECTOR, Some(mid));

        group.bench_with_input(BenchmarkId::from_parameter(count), &html, |b, html| {
            b.iter(|| {
                let store = parse(black_box(html), black_box(queries)).unwrap();
                black_box(store);
            })
        });
        group.finish();
    }

    // --- late ---
    {
        let mut group = c.benchmark_group("parse/first_match/late");
        // No group.throughput(...)
        let last = count - 1;
        let html = generate_first_match_html(count, Some(last));
        let queries = &[Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build()];
        let store = parse(&html, queries).unwrap();
        assert_first_match_result(&store, FIRST_MATCH_SELECTOR, Some(last));
        group.bench_with_input(BenchmarkId::from_parameter(count), &html, |b, html| {
            b.iter(|| {
                let store = parse(black_box(html), black_box(queries)).unwrap();
                black_box(store);
            })
        });
        group.finish();
    }

    // --- no_match ---
    {
        let mut group = c.benchmark_group("parse/first_match/no_match");
        // No group.throughput(...) — consistent with other first-match cases
        let html = generate_first_match_html(count, None);
        let queries = &[Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build()];
        let store = parse(&html, queries).unwrap();
        assert_first_match_result(&store, FIRST_MATCH_SELECTOR, None);

        group.bench_with_input(BenchmarkId::from_parameter(count), &html, |b, html| {
            b.iter(|| {
                let store = parse(black_box(html), black_box(queries)).unwrap();
                black_box(store);
            })
        });
        group.finish();
    }
}

// ── Nested product catalog benchmarks ──────────────────────────────────────

const PRODUCT_SELECTOR: &str = "div.product";
const PRODUCT_TITLE_SELECTOR: &str = "> h1";
const PRODUCT_RATING_SELECTOR: &str = "> span.rating";
const PRODUCT_DESCRIPTION_SELECTOR: &str = "> p.description";

fn consume_product_results(store: &scah::Store<'_, '_>) {
    // Consume both parent and nested child results
    if let Some(products) = store.get(PRODUCT_SELECTOR) {
        for product in products {
            black_box(product.attribute(store, "class"));
            black_box(&product.inner_html);
            black_box(&product.text_content(store));

            // Consume nested child results for each product
            if let Some(titles) = product.get(store, PRODUCT_TITLE_SELECTOR) {
                for title in titles {
                    black_box(&title.inner_html);
                    black_box(&title.text_content(store));
                }
            }
            if let Some(ratings) = product.get(store, PRODUCT_RATING_SELECTOR) {
                for rating in ratings {
                    black_box(&rating.inner_html);
                    black_box(&rating.text_content(store));
                }
            }
            if let Some(descriptions) = product.get(store, PRODUCT_DESCRIPTION_SELECTOR) {
                for description in descriptions {
                    black_box(&description.inner_html);
                    black_box(&description.text_content(store));
                }
            }
        }
    }
}

fn bench_product_catalog(c: &mut Criterion) {
    for &size in PRODUCT_SIZES {
        let html = generate_product_catalog_html(size);
        let throughput = Throughput::Bytes(html.len() as u64);

        // --- Prebuilt nested_all ---
        {
            let mut group = c.benchmark_group("parse/product_catalog/prebuilt/nested_all/save_all");
            group.throughput(throughput.clone());
            let queries = &[build_nested_all_query()];
            let store = parse(&html, queries).unwrap();
            assert_product_catalog_all(
                &store,
                PRODUCT_SELECTOR,
                PRODUCT_TITLE_SELECTOR,
                PRODUCT_RATING_SELECTOR,
                PRODUCT_DESCRIPTION_SELECTOR,
                size,
            );

            group.bench_with_input(BenchmarkId::from_parameter(size), &html, |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(queries)).unwrap();
                    black_box(store);
                })
            });
            group.finish();
        }

        // --- Prebuilt nested_first ---
        {
            let mut group =
                c.benchmark_group("parse/product_catalog/prebuilt/nested_first/save_all");
            group.throughput(throughput.clone());
            let queries = &[build_nested_first_query()];
            let store = parse(&html, queries).unwrap();
            assert_product_catalog_first(
                &store,
                PRODUCT_SELECTOR,
                PRODUCT_TITLE_SELECTOR,
                PRODUCT_RATING_SELECTOR,
                PRODUCT_DESCRIPTION_SELECTOR,
            );

            group.bench_with_input(BenchmarkId::from_parameter(size), &html, |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(queries)).unwrap();
                    black_box(store);
                })
            });
            group.finish();
        }

        // --- Consume nested_all ---
        {
            let mut group = c.benchmark_group("parse/product_catalog/consume/nested_all/save_all");
            group.throughput(throughput.clone());
            let queries = &[build_nested_all_query()];
            let store = parse(&html, queries).unwrap();
            assert_product_catalog_all(
                &store,
                PRODUCT_SELECTOR,
                PRODUCT_TITLE_SELECTOR,
                PRODUCT_RATING_SELECTOR,
                PRODUCT_DESCRIPTION_SELECTOR,
                size,
            );

            group.bench_with_input(BenchmarkId::from_parameter(size), &html, |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(queries)).unwrap();
                    consume_product_results(black_box(&store));
                })
            });
            group.finish();
        }

        // --- End-to-end nested_all ---
        {
            let mut group =
                c.benchmark_group("parse/product_catalog/end_to_end/nested_all/save_all");
            group.throughput(throughput.clone());

            // Validate the query builder itself before the timed loop.
            // The end-to-end benchmark constructs a fresh query inside the loop,
            // so its output cannot be validated without a separate parse. We
            // validate the builder outside the loop to guard against drift.
            let validation_queries = &[build_nested_all_query()];
            let validation_store = parse(&html, validation_queries).unwrap();
            assert_product_catalog_all(
                &validation_store,
                PRODUCT_SELECTOR,
                PRODUCT_TITLE_SELECTOR,
                PRODUCT_RATING_SELECTOR,
                PRODUCT_DESCRIPTION_SELECTOR,
                size,
            );

            group.bench_with_input(BenchmarkId::from_parameter(size), &html, |b, html| {
                b.iter(|| {
                    let queries = &[build_nested_all_query_with(
                        black_box(PRODUCT_SELECTOR),
                        black_box(PRODUCT_TITLE_SELECTOR),
                        black_box(PRODUCT_RATING_SELECTOR),
                        black_box(PRODUCT_DESCRIPTION_SELECTOR),
                    )];
                    let store = parse(black_box(html), black_box(queries)).unwrap();
                    consume_product_results(black_box(&store));
                })
            });
            group.finish();
        }
    }
}

// ── Multi-query pressure benchmarks ────────────────────────────────────────

fn bench_multi_query(c: &mut Criterion) {
    let element_count = MULTI_QUERY_COUNT;

    for &query_count in MULTI_QUERY_COUNTS {
        let html = generate_multi_query_html(element_count, query_count);
        let throughput = Throughput::Bytes(html.len() as u64);

        let mut group = c.benchmark_group("parse/multi_query/prebuilt");
        group.throughput(throughput);

        // Build queries targeting each class
        let selectors: Vec<String> = (0..query_count).map(|i| format!(".class-{i}")).collect();
        let queries: Vec<Query> = selectors
            .iter()
            .map(|sel| {
                Query::all(sel.as_str(), Save::all())
                    .expect("multi-query selector should parse")
                    .build()
            })
            .collect();

        // Validate each selector independently with class verification
        let store = parse(&html, &queries).unwrap();
        assert_multi_query_results(&store, element_count, query_count);

        group.bench_with_input(
            BenchmarkId::from_parameter(query_count),
            &html,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(&queries)).unwrap();
                    black_box(store);
                })
            },
        );

        group.finish();
    }
}

// ── Criterion harness ──────────────────────────────────────────────────────

criterion_group! {
    name = benches;
    config = support::config::criterion_config();
    targets =
        bench_query_building,
        bench_synthetic_links,
        bench_first_match,
        bench_product_catalog,
        bench_multi_query,
}
criterion_main!(benches);
