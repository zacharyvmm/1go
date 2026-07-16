mod support;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse};
use std::hint::black_box;
use support::fixtures::{
    generate_first_match_html, generate_link_list_html, generate_multi_query_html,
    generate_product_catalog_html,
};
use support::validation::{
    assert_first_match_result, assert_has_href_attribute, assert_match_count,
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
            let query = Query::all("div.product", Save::all())
                .expect("parent selector should parse")
                .then(|product| {
                    Ok([
                        product.all("> h1", Save::all())?,
                        product.all("> span.rating", Save::all())?,
                        product.all("> p.description", Save::all())?,
                    ])
                })
                .expect("child selectors should parse")
                .build();
            black_box(query);
        })
    });

    // --- nested/first ---
    group.bench_function("nested/first", |b| {
        b.iter(|| {
            let query = Query::first("div.product", Save::all())
                .expect("parent selector should parse")
                .then(|product| {
                    Ok([
                        product.first("> h1", Save::all())?,
                        product.first("> span.rating", Save::all())?,
                        product.first("> p.description", Save::all())?,
                    ])
                })
                .expect("child selectors should parse")
                .build();
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
            // Validate
            let store = parse(&html, queries).unwrap();
            assert_match_count(&store, LINK_SELECTOR, size);

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
            assert_match_count(&store, LINK_SELECTOR, size);

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
            assert_match_count(&store, LINK_SELECTOR, size);

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
            let queries = &[Query::all(LINK_SELECTOR, Save::all())
                .expect("selector should parse")
                .build()];
            let store = parse(&html, queries).unwrap();
            assert_match_count(&store, LINK_SELECTOR, size);

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
            let queries = &[Query::all(LINK_SELECTOR, Save::all())
                .expect("selector should parse")
                .build()];
            let store = parse(&html, queries).unwrap();
            assert_match_count(&store, LINK_SELECTOR, size);
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

            group.bench_with_input(BenchmarkId::from_parameter(size), &html, |b, html| {
                b.iter(|| {
                    let queries = &[Query::all(black_box(LINK_SELECTOR), Save::all())
                        .expect("selector should parse")
                        .build()];
                    let store = parse(black_box(html), black_box(queries)).unwrap();
                    consume_link_results(black_box(&store));
                })
            });
            group.finish();
        }
    }
}

// ── First-match placement benchmarks ───────────────────────────────────────

const FIRST_MATCH_SELECTOR: &str = "a.target";

fn bench_first_match(c: &mut Criterion) {
    let count = FIRST_MATCH_COUNT;

    // --- early ---
    {
        let mut group = c.benchmark_group("parse/first_match/early");
        let html = generate_first_match_html(count, Some(0));
        group.throughput(Throughput::Bytes(html.len() as u64));
        let queries = &[Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build()];
        let store = parse(&html, queries).unwrap();
        assert_first_match_result(&store, FIRST_MATCH_SELECTOR, true, Some("Post 0"));

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
        let mid = count / 2;
        let html = generate_first_match_html(count, Some(mid));
        group.throughput(Throughput::Bytes(html.len() as u64));
        let queries = &[Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build()];
        let store = parse(&html, queries).unwrap();
        assert_first_match_result(
            &store,
            FIRST_MATCH_SELECTOR,
            true,
            Some(&format!("Post {mid}")),
        );

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
        let last = count - 1;
        let html = generate_first_match_html(count, Some(last));
        group.throughput(Throughput::Bytes(html.len() as u64));
        let queries = &[Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build()];
        let store = parse(&html, queries).unwrap();
        assert_first_match_result(
            &store,
            FIRST_MATCH_SELECTOR,
            true,
            Some(&format!("Post {last}")),
        );

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
        let html = generate_first_match_html(count, None);
        group.throughput(Throughput::Bytes(html.len() as u64));
        let queries = &[Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build()];
        let store = parse(&html, queries).unwrap();
        assert_first_match_result(&store, FIRST_MATCH_SELECTOR, false, None);

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
    // Consume parent results: iterate over matched product divs and access fields
    if let Some(elements) = store.get(PRODUCT_SELECTOR) {
        for element in elements {
            black_box(element.attribute(store, "class"));
            black_box(&element.inner_html);
            black_box(&element.text_content(store));
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
            let queries = &[Query::all(PRODUCT_SELECTOR, Save::all())
                .expect("parent selector should parse")
                .then(|product| {
                    Ok([
                        product.all(PRODUCT_TITLE_SELECTOR, Save::all())?,
                        product.all(PRODUCT_RATING_SELECTOR, Save::all())?,
                        product.all(PRODUCT_DESCRIPTION_SELECTOR, Save::all())?,
                    ])
                })
                .expect("child selectors should parse")
                .build()];
            let store = parse(&html, queries).unwrap();
            assert_match_count(&store, PRODUCT_SELECTOR, size);

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
            let queries = &[Query::first(PRODUCT_SELECTOR, Save::all())
                .expect("parent selector should parse")
                .then(|product| {
                    Ok([
                        product.first(PRODUCT_TITLE_SELECTOR, Save::all())?,
                        product.first(PRODUCT_RATING_SELECTOR, Save::all())?,
                        product.first(PRODUCT_DESCRIPTION_SELECTOR, Save::all())?,
                    ])
                })
                .expect("child selectors should parse")
                .build()];
            let store = parse(&html, queries).unwrap();
            assert_match_count(&store, PRODUCT_SELECTOR, 1);

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
            let queries = &[Query::all(PRODUCT_SELECTOR, Save::all())
                .expect("parent selector should parse")
                .then(|product| {
                    Ok([
                        product.all(PRODUCT_TITLE_SELECTOR, Save::all())?,
                        product.all(PRODUCT_RATING_SELECTOR, Save::all())?,
                        product.all(PRODUCT_DESCRIPTION_SELECTOR, Save::all())?,
                    ])
                })
                .expect("child selectors should parse")
                .build()];
            let store = parse(&html, queries).unwrap();
            assert_match_count(&store, PRODUCT_SELECTOR, size);

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

            group.bench_with_input(BenchmarkId::from_parameter(size), &html, |b, html| {
                b.iter(|| {
                    let queries = &[Query::all(PRODUCT_SELECTOR, Save::all())
                        .expect("parent selector should parse")
                        .then(|product| {
                            Ok([
                                product.all(PRODUCT_TITLE_SELECTOR, Save::all())?,
                                product.all(PRODUCT_RATING_SELECTOR, Save::all())?,
                                product.all(PRODUCT_DESCRIPTION_SELECTOR, Save::all())?,
                            ])
                        })
                        .expect("child selectors should parse")
                        .build()];
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

        // Validate: each selector matches element_count / query_count elements
        let store = parse(&html, &queries).unwrap();
        // Validate: total across all selectors equals element_count
        let total: usize = selectors
            .iter()
            .map(|s| store.get(s).map(|e| e.count()).unwrap_or(0))
            .sum();
        assert_eq!(
            total, element_count,
            "total matches across all selectors should equal element_count"
        );

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
