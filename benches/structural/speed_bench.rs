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

fn query(selector: &'static str) -> Query<'static> {
    Query::all(selector, Save::none())
        .expect("valid structural benchmark selector")
        .build()
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
        (
            "nine_filtered_ordinals",
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
            ],
        ),
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
}

criterion_group!(benches, bench_structural_selectors);
criterion_main!(benches);
