use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, QueryMultiplexer, Reader, Save, XHtmlParser, parse};
use std::hint::black_box;

fn generate_flat_nonmatching_html(count: usize) -> String {
    let mut html = String::with_capacity(count * 11 + 16);
    html.push_str("<main>");
    for _ in 0..count {
        html.push_str("<div></div>");
    }
    html.push_str("</main>");
    html
}

fn build_nonmatching_queries(runner_count: usize) -> Vec<Query<'static>> {
    (0..runner_count)
        .map(|_| {
            Query::all("never-matches", Save::none())
                .expect("nonmatching selector")
                .build()
        })
        .collect()
}

fn generate_nested_div_html(depth: usize) -> String {
    let mut html = String::with_capacity(depth * 11 + 16);
    html.push_str("<main>");
    html.push_str(&"<div>".repeat(depth));
    html.push_str(&"</div>".repeat(depth));
    html.push_str("</main>");
    html
}

fn build_descendant_obligation_query() -> Query<'static> {
    // Every nested `div` advances a non-terminal descendant cursor, but the
    // terminal predicate never matches, so this isolates cursor pressure from
    // result-store writes.
    Query::all("div never-matches", Save::none())
        .expect("descendant obligation selector")
        .build()
}

fn build_sibling_obligation_query() -> Query<'static> {
    Query::all("main", Save::none())
        .expect("sibling obligation parent selector")
        .then(|main| {
            Ok([
                main.all("div ~ p.never-one", Save::none())?,
                main.all("div ~ p.never-two", Save::none())?,
                main.all("div ~ p.never-three", Save::none())?,
                main.all("div ~ p.never-four", Save::none())?,
            ])
        })
        .expect("sibling obligation continuations")
        .build()
}

fn generate_flat_divs_then_p(div_count: usize) -> String {
    let mut html = String::with_capacity(div_count * 12 + 32);
    html.push_str("<main>");
    for _ in 0..div_count {
        html.push_str("<div></div>");
    }
    html.push_str("<p></p></main>");
    html
}

fn generate_alternating_div_p(pairs: usize) -> String {
    let mut html = String::with_capacity(pairs * 20 + 32);
    html.push_str("<main>");
    for _ in 0..pairs {
        html.push_str("<div></div><p></p>");
    }
    html.push_str("</main>");
    html
}

fn generate_large_source_subtree(depth: usize) -> String {
    let mut html = String::with_capacity(depth * 19 + 48);
    html.push_str("<main><div>");
    html.push_str(&"<section>".repeat(depth));
    html.push_str(&"</section>".repeat(depth));
    html.push_str("</div><p></p></main>");
    html
}

/// Deep nesting under a child obligation that can only match at one depth.
///
/// Shape: `main > article > section^{depth} > ...` with query
/// `main > article > p.never-matches`. The `p` child cursor is eligible only at
/// the article's direct-child depth and should skip all deeper subtree events.
fn generate_child_obligation_deep_html(depth: usize) -> String {
    let mut html = String::with_capacity(depth * 20 + 48);
    html.push_str("<main><article>");
    html.push_str(&"<section>".repeat(depth));
    html.push_str(&"</section>".repeat(depth));
    html.push_str("</article></main>");
    html
}

fn build_child_obligation_query() -> Query<'static> {
    Query::all("main > article > p.never-matches", Save::none())
        .expect("child obligation selector")
        .build()
}

fn generate_simultaneous_retirement_html() -> &'static str {
    "<main><h1 class=\"early\"></h1></main>"
}

fn build_first_queries(first_count: usize) -> Vec<Query<'static>> {
    (0..first_count)
        .map(|_| {
            Query::first(".early", Save::none())
                .expect("first selector")
                .build()
        })
        .collect()
}

/// One early element retires every `First` runner; the large unmatched div tail
/// then exercises sparse active-set dispatch without O(n²) distinct-selector matching.
fn generate_retired_runner_html(tail_divs: usize) -> String {
    let mut html = String::with_capacity(tail_divs * 11 + 48);
    html.push_str("<main><h1 class=\"early\"></h1>");
    for _ in 0..tail_divs {
        html.push_str("<div></div>");
    }
    html.push_str("</main>");
    html
}

fn build_retired_runner_queries(first_count: usize) -> (Vec<Query<'static>>, &'static str) {
    // Identical First selectors intentionally: each runner still occupies a
    // stable slot, matches once on the shared early element, then retires for
    // the remainder of the parse.
    let mut queries: Vec<Query<'static>> = (0..first_count)
        .map(|_| {
            Query::first(".early", Save::none())
                .expect("first selector")
                .build()
        })
        .collect();
    let all_selector: &'static str = "div";
    queries.push(
        Query::all(all_selector, Save::none())
            .expect("surviving all selector")
            .build(),
    );
    (queries, all_selector)
}

fn bench_sibling_selectors(c: &mut Criterion) {
    let mut group = c.benchmark_group("sibling_combinator_hot_paths");

    for size in [1_000, 10_000] {
        let subsequent_html = generate_flat_divs_then_p(size);
        let subsequent_queries = &[Query::all("div ~ p", Save::none())
            .expect("subsequent sibling selector")
            .build()];
        group.throughput(Throughput::Bytes(subsequent_html.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("subsequent_div_tilde_p", size),
            &subsequent_html,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(subsequent_queries)).unwrap();
                    black_box(store.get("div ~ p").map(|iter| iter.count()));
                })
            },
        );

        let adjacent_html = generate_alternating_div_p(size);
        let adjacent_queries = &[Query::all("div + p", Save::none())
            .expect("adjacent sibling selector")
            .build()];
        group.throughput(Throughput::Bytes(adjacent_html.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("adjacent_div_plus_p", size),
            &adjacent_html,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(adjacent_queries)).unwrap();
                    black_box(store.get("div + p").map(|iter| iter.count()));
                })
            },
        );
    }

    // Multi-query retirement: an early First runner exits while sibling runners remain.
    let retirement_html = r#"
        <main>
          <section>
            <aside>
              <h1></h1>
            </aside>
            <footer></footer>
          </section>
          <p></p>
        </main>
    "#;
    let retirement_queries = [
        Query::first("h1", Save::none()).unwrap().build(),
        Query::all("section + p", Save::none()).unwrap().build(),
        Query::all("aside + footer", Save::none()).unwrap().build(),
    ];
    group.bench_function("multi_query_early_first_retirement", |b| {
        b.iter(|| {
            let store = parse(black_box(retirement_html), black_box(&retirement_queries)).unwrap();
            black_box(store.get("section + p").map(|iter| iter.count()));
            black_box(store.get("aside + footer").map(|iter| iter.count()));
        })
    });

    group.finish();
}

fn bench_ordinary_hot_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("ordinary_hot_path");

    for size in [1_000usize, 10_000] {
        let html = generate_flat_nonmatching_html(size);
        let queries = build_nonmatching_queries(1);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("no_match_one_runner", size),
            &html,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(&queries)).unwrap();
                    black_box(store);
                })
            },
        );
    }

    let html = generate_flat_nonmatching_html(10_000);
    for runner_count in [8usize, 64] {
        let queries = build_nonmatching_queries(runner_count);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(
            BenchmarkId::new(format!("no_match_{runner_count}_runners"), 10_000),
            &html,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(&queries)).unwrap();
                    black_box(store);
                })
            },
        );
    }

    group.finish();
}

fn bench_cursor_obligations(c: &mut Criterion) {
    let mut group = c.benchmark_group("cursor_obligation_hot_paths");
    group.sample_size(10);

    let flat_html = generate_flat_nonmatching_html(10_000);
    let one_cursor_queries = build_nonmatching_queries(1);
    group.throughput(Throughput::Bytes(flat_html.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("one_active_cursor", 10_000),
        &flat_html,
        |b, html| {
            b.iter(|| {
                let store = parse(black_box(html), black_box(&one_cursor_queries)).unwrap();
                black_box(store);
            })
        },
    );

    for depth in [64usize, 256, 1_024] {
        let html = generate_nested_div_html(depth);
        let query = build_descendant_obligation_query();
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("descendant_obligations", depth),
            &html,
            |b, html| {
                b.iter(|| {
                    let store =
                        parse(black_box(html), black_box(std::slice::from_ref(&query))).unwrap();
                    black_box(store);
                })
            },
        );
    }

    for source_count in [1_000usize, 10_000] {
        let html = generate_flat_divs_then_p(source_count);
        let query = build_sibling_obligation_query();
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("sibling_obligations", source_count),
            &html,
            |b, html| {
                b.iter(|| {
                    let store =
                        parse(black_box(html), black_box(std::slice::from_ref(&query))).unwrap();
                    black_box(store);
                })
            },
        );
    }

    group.finish();
}

fn bench_child_obligation_hot_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("child_obligation_hot_path");
    group.sample_size(10);
    let query = build_child_obligation_query();

    for depth in [64usize, 256, 1_024, 4_096] {
        let html = generate_child_obligation_deep_html(depth);
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("deep_wrong_depth", depth),
            &html,
            |b, html| {
                b.iter(|| {
                    let store =
                        parse(black_box(html), black_box(std::slice::from_ref(&query))).unwrap();
                    black_box(store);
                })
            },
        );
    }

    group.finish();
}

/// Full public `parse()` cost for many identical `First` runners.
///
/// Includes query executor construction, multiplexer/parser setup, tokenization,
/// matching, retirement, and draining. Useful for user-visible total cost; not an
/// isolated retirement microbenchmark.
fn bench_many_first_queries_end_to_end(c: &mut Criterion) {
    let mut group = c.benchmark_group("many_first_queries_end_to_end");
    group.sample_size(10);
    let html = generate_simultaneous_retirement_html();
    group.throughput(Throughput::Bytes(html.len() as u64));

    for first_count in [64usize, 256, 1_024] {
        let queries = Box::leak(build_first_queries(first_count).into_boxed_slice());
        group.bench_with_input(
            BenchmarkId::from_parameter(first_count),
            &first_count,
            |b, _| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(queries as &[Query])).unwrap();
                    black_box(store);
                })
            },
        );
    }

    group.finish();
}

/// Times only the close event that retires every prepared `First` runner.
///
/// Setup advances through `<main>` and `<h1 class="early">`; the measured
/// iteration closes `h1` and performs the dense-to-sparse active-set transition.
fn bench_isolated_first_retirement_close(c: &mut Criterion) {
    let mut group = c.benchmark_group("isolated_first_retirement_close");
    group.sample_size(20);
    const HTML: &str = "<main><h1 class=\"early\"></h1></main>";

    for first_count in [64usize, 256, 1_024] {
        let queries = Box::leak(build_first_queries(first_count).into_boxed_slice());
        group.bench_with_input(
            BenchmarkId::from_parameter(first_count),
            &first_count,
            |b, _| {
                b.iter_batched(
                    || {
                        let mut parser =
                            XHtmlParser::new(QueryMultiplexer::new(queries as &[Query]));
                        let mut reader = Reader::new(HTML);
                        assert!(parser.next(&mut reader), "open <main>");
                        assert!(parser.next(&mut reader), "open <h1>");
                        (parser, reader)
                    },
                    |(mut parser, mut reader)| {
                        black_box(parser.next(&mut reader)); // </h1> retires all First runners
                        black_box(parser);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_redundant_general_sibling_sources(c: &mut Criterion) {
    let mut group = c.benchmark_group("redundant_general_sibling_sources");

    for source_count in [1_000usize, 10_000] {
        let html = generate_flat_divs_then_p(source_count);
        let queries = &[Query::all("div ~ p", Save::none())
            .expect("redundant general-sibling source selector")
            .build()];
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("div_tilde_p", source_count),
            &html,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(queries)).unwrap();
                    black_box(store);
                })
            },
        );
    }

    group.finish();
}

fn bench_large_source_subtree(c: &mut Criterion) {
    let mut group = c.benchmark_group("large_source_subtree_sibling");
    group.sample_size(10);

    for depth in [128usize, 1_024, 4_096] {
        let html = generate_large_source_subtree(depth);
        let queries = &[Query::all("div + p", Save::none())
            .expect("large source-subtree sibling selector")
            .build()];
        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("adjacent_div_plus_p", depth),
            &html,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(queries)).unwrap();
                    black_box(store);
                })
            },
        );
    }

    group.finish();
}

fn bench_many_retired_runners(c: &mut Criterion) {
    let mut group = c.benchmark_group("many_retired_runners");
    const TAIL_DIVS: usize = 5_000;

    // Identical HTML for control and candidates: one early match target + tail.
    let html = generate_retired_runner_html(TAIL_DIVS);
    let control_queries = &[Query::all("div", Save::none())
        .expect("control all selector")
        .build()];
    group.throughput(Throughput::Bytes(html.len() as u64));
    group.bench_function("control_one_all", |b| {
        b.iter(|| {
            let store = parse(black_box(&html), black_box(control_queries)).unwrap();
            black_box(store.get("div").map(|iter| iter.count()));
        })
    });

    for first_count in [64usize, 256, 1_024] {
        let (queries, all_selector) = build_retired_runner_queries(first_count);
        let queries = Box::leak(queries.into_boxed_slice());

        group.throughput(Throughput::Bytes(html.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(first_count),
            &html,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(queries as &[Query])).unwrap();
                    black_box(store.get(all_selector).map(|iter| iter.count()));
                })
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_sibling_selectors,
    bench_ordinary_hot_path,
    bench_cursor_obligations,
    bench_child_obligation_hot_path,
    bench_many_retired_runners,
    bench_many_first_queries_end_to_end,
    bench_isolated_first_retirement_close,
    bench_redundant_general_sibling_sources,
    bench_large_source_subtree,
);
criterion_main!(benches);
