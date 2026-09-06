use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse};
use std::hint::black_box;

fn structural_html(count: usize) -> String {
    let mut html = String::with_capacity(count * 30 + 32);
    html.push_str("<main><ul>");
    for index in 0..count {
        let class = index % 9;
        html.push_str("<li class='hit f");
        html.push(char::from(b'0' + class as u8));
        html.push_str("'></li>");
    }
    html.push_str("</ul></main>");
    html
}

fn overlapping_filter_html(count: usize) -> String {
    let mut html = String::with_capacity(count * 55 + 32);
    html.push_str("<main><ul>");
    for _ in 0..count {
        html.push_str("<li class='f0 f1 f2 f3 f4 f5 f6 f7 f8'></li>");
    }
    html.push_str("</ul></main>");
    html
}

fn many_parent_html(count: usize) -> String {
    let mut html = String::with_capacity(count * 25 + 16);
    html.push_str("<main>");
    for _ in 0..count {
        html.push_str("<div><span></span></div>");
    }
    html.push_str("</main>");
    html
}

fn query(selector: &'static str) -> Query<'static> {
    Query::all(selector, Save::none())
        .expect("valid structural benchmark selector")
        .build()
}

fn nine_filtered_queries() -> Vec<Query<'static>> {
    vec![
        query("li:nth-child(2n+1 of .f0)"),
        query("li:nth-child(2n+1 of .f1)"),
        query("li:nth-child(2n+1 of .f2)"),
        query("li:nth-child(2n+1 of .f3)"),
        query("li:nth-child(2n+1 of .f4)"),
        query("li:nth-child(2n+1 of .f5)"),
        query("li:nth-child(2n+1 of .f6)"),
        query("li:nth-child(2n+1 of .f7)"),
        query("li:nth-child(2n+1 of .f8)"),
    ]
}

fn bench_structural_selectors(c: &mut Criterion) {
    let mut group = c.benchmark_group("structural_selector_hot_paths");
    group.sample_size(20);
    let html = structural_html(10_000);
    group.throughput(Throughput::Bytes(html.len() as u64));

    let cases = [
        ("root", vec![query(":root")]),
        ("first_child", vec![query("li:first-child")]),
        ("nth_child", vec![query("li:nth-child(2n+1)")]),
        ("nth_of_type", vec![query("li:nth-of-type(2n+1)")]),
        (
            "filtered_nth_child",
            vec![query("li:nth-child(2n+1 of .hit)")],
        ),
        ("nine_filtered_ordinals", nine_filtered_queries()),
    ];

    for (name, queries) in cases {
        group.bench_with_input(BenchmarkId::new(name, 10_000), &queries, |b, queries| {
            b.iter(|| {
                let store = parse(black_box(&html), black_box(queries)).unwrap();
                black_box(store);
            });
        });
    }
    group.finish();

    let html = overlapping_filter_html(10_000);
    let queries = nine_filtered_queries();
    let mut group = c.benchmark_group("structural_overlapping_filters");
    group.sample_size(20);
    group.throughput(Throughput::Bytes(html.len() as u64));
    group.bench_function("nine_matching_filters_per_element", |b| {
        b.iter(|| {
            let store = parse(black_box(&html), black_box(&queries)).unwrap();
            black_box(store);
        });
    });
    group.finish();

    let html = many_parent_html(10_000);
    let queries = vec![query("span:nth-of-type(1)")];
    let mut group = c.benchmark_group("structural_many_small_parents");
    group.sample_size(20);
    group.throughput(Throughput::Bytes(html.len() as u64));
    group.bench_function("named_nth_of_type", |b| {
        b.iter(|| {
            let store = parse(black_box(&html), black_box(&queries)).unwrap();
            black_box(store);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_structural_selectors);
criterion_main!(benches);
