//! Focused main-vs-PR gate benchmarks for no-text scan overhead.
//! Temporary measurement harness — uses only Save APIs available on main.
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse_without_text_capture};
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

fn known_tags_html(count: usize) -> String {
    let mut html = String::from("<section>");
    for i in 0..count {
        html.push_str(&format!("<p>item {i}</p>"));
    }
    html.push_str("</section>");
    html
}

fn custom_tags_html(count: usize) -> String {
    let mut html = String::from("<section>");
    for i in 0..count {
        html.push_str(&format!("<x-item>item {i}</x-item>"));
    }
    html.push_str("</section>");
    html
}

fn bench_gates(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_extraction_modes");
    group.sample_size(100);

    for size in [1000usize].iter().copied() {
        let prose = prose_html(size);
        group.throughput(Throughput::Bytes(prose.len() as u64));

        let none_q = &[Query::all("p", Save::none()).unwrap().build()];
        group.bench_with_input(BenchmarkId::new("no_content", size), &prose, |b, html| {
            b.iter(|| {
                let store = parse_without_text_capture(black_box(html), black_box(none_q)).unwrap();
                black_box(store);
            })
        });

        let none_no_match_q = &[Query::all("article > span", Save::none()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("no_content_no_matches", size),
            &prose,
            |b, html| {
                b.iter(|| {
                    let store =
                        parse_without_text_capture(black_box(html), black_box(none_no_match_q))
                            .unwrap();
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
                    let store =
                        parse_without_text_capture(black_box(html), black_box(inner_q)).unwrap();
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
                    let store =
                        parse_without_text_capture(black_box(html), black_box(no_match_q)).unwrap();
                    black_box(store);
                })
            },
        );
    }
    group.finish();

    let mut diag = c.benchmark_group("text_extraction_scan_diagnostics");
    diag.sample_size(100);

    let empty = "";
    let one_tag = "<p>x</p>";
    let ten_tags = "<div><p>1</p><p>2</p><p>3</p><p>4</p><p>5</p><p>6</p><p>7</p><p>8</p><p>9</p><p>10</p></div>";
    let no_match_q = &[Query::all("article > span", Save::none()).unwrap().build()];

    diag.bench_function("parse_empty_document", |b| {
        b.iter(|| {
            let store =
                parse_without_text_capture(black_box(empty), black_box(no_match_q)).unwrap();
            black_box(store);
        })
    });
    diag.bench_function("parse_one_tag_no_match", |b| {
        b.iter(|| {
            let store =
                parse_without_text_capture(black_box(one_tag), black_box(no_match_q)).unwrap();
            black_box(store);
        })
    });
    diag.bench_function("parse_ten_tags_no_match", |b| {
        b.iter(|| {
            let store =
                parse_without_text_capture(black_box(ten_tags), black_box(no_match_q)).unwrap();
            black_box(store);
        })
    });

    for size in [1000usize].iter().copied() {
        let known = known_tags_html(size);
        let custom = custom_tags_html(size);
        let q = &[Query::all("article > span", Save::none()).unwrap().build()];
        diag.throughput(Throughput::Bytes(known.len() as u64));
        diag.bench_with_input(
            BenchmarkId::new("known_tags_no_match", size),
            &known,
            |b, html| {
                b.iter(|| {
                    let store = parse_without_text_capture(black_box(html), black_box(q)).unwrap();
                    black_box(store);
                })
            },
        );
        diag.throughput(Throughput::Bytes(custom.len() as u64));
        diag.bench_with_input(
            BenchmarkId::new("custom_tags_no_match", size),
            &custom,
            |b, html| {
                b.iter(|| {
                    let store = parse_without_text_capture(black_box(html), black_box(q)).unwrap();
                    black_box(store);
                })
            },
        );
    }
    diag.finish();
}

criterion_group!(benches, bench_gates);
criterion_main!(benches);
