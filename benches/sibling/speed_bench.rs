use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse};
use std::hint::black_box;

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

criterion_group!(benches, bench_sibling_selectors);
criterion_main!(benches);
