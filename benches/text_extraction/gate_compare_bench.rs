//! Primary main-vs-PR gate benchmark for the standard parse() API.
//! Compatible with both main and PR branches.
//! Evaluates standard parse() throughput and latency against main base.
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse};
use std::hint::black_box;

fn prose_html(paragraphs: usize) -> String {
    let mut html = String::from("<article>");
    for i in 0..paragraphs {
        html.push_str(&format!(
            "<p>Hello <strong>world</strong> number {i} with   spaced   words.</p>"
        ));
    }
    html.push_str("</article>");
    html
}

fn matched_void_html(count: usize) -> String {
    let mut html = String::from("<div>");
    for i in 0..count {
        html.push_str(&format!("<input id=\"i{i}\" type=\"text\">"));
    }
    html.push_str("</div>");
    html
}

fn bench_gates(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_extraction_parse");
    group.sample_size(100);

    for size in [1000usize].iter().copied() {
        let prose = prose_html(size);
        group.throughput(Throughput::Bytes(prose.len() as u64));

        let none_q = &[Query::all("p", Save::none()).unwrap().build()];
        group.bench_with_input(BenchmarkId::new("no_content", size), &prose, |b, html| {
            b.iter(|| {
                let store = parse(black_box(html), black_box(none_q)).unwrap();
                black_box(store);
            })
        });

        let none_no_match_q = &[Query::all("article > span", Save::none()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("no_content_no_matches", size),
            &prose,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(none_no_match_q)).unwrap();
                    black_box(store);
                })
            },
        );

        let inner_q = &[Query::all("p", Save::only_inner_html()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("inner_html_only", size),
            &prose,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(inner_q)).unwrap();
                    black_box(store);
                })
            },
        );

        let no_match_q = &[Query::all("article > span", Save::only_inner_html())
            .unwrap()
            .build()];
        group.bench_with_input(
            BenchmarkId::new("inner_html_no_matches", size),
            &prose,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(no_match_q)).unwrap();
                    black_box(store);
                })
            },
        );

        let voids = matched_void_html(size);
        let void_q = &[Query::all("input", Save::none()).unwrap().build()];
        group.throughput(Throughput::Bytes(voids.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("matched_void_no_content", size),
            &voids,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(void_q)).unwrap();
                    black_box(store);
                })
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_gates);
criterion_main!(benches);
