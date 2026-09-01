use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse};
use std::hint::black_box;

fn flat_div_html(count: usize) -> String {
    let mut html = String::with_capacity(count * 11 + 16);
    html.push_str("<main>");
    for _ in 0..count {
        html.push_str("<div></div>");
    }
    html.push_str("</main>");
    html
}

fn bench_ordinary(c: &mut Criterion) {
    let mut group = c.benchmark_group("ordinary_parser_gate");
    group.sample_size(20);
    let html = flat_div_html(10_000);
    group.throughput(Throughput::Bytes(html.len() as u64));

    for (name, selector) in [("no_match", "never-matches"), ("match", "div")] {
        let queries = [Query::all(selector, Save::none())
            .expect("valid gate selector")
            .build()];
        group.bench_with_input(BenchmarkId::new(name, 10_000), &html, |b, html| {
            b.iter(|| {
                let store = parse(black_box(html), black_box(&queries)).unwrap();
                black_box(store.get(selector).map(|iter| iter.count()));
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_ordinary);
criterion_main!(benches);
