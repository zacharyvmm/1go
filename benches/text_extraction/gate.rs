use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse};
use std::hint::black_box;

const SIZE: usize = 1_000;

fn prose_html(paragraphs: usize) -> String {
    let mut html = String::from("<article>");
    for i in 0..paragraphs {
        html.push_str(&format!(
            "<p data-unused='{i}'>Hello <strong>world</strong> number {i} with &amp; text.</p>"
        ));
    }
    html.push_str("</article>");
    html
}

fn sparse_matches_html(paragraphs: usize) -> String {
    let mut html = String::from("<article>");
    for i in 0..paragraphs {
        if i % 100 == 0 {
            html.push_str(&format!("<p class='hit'>selected {i}</p>"));
        } else {
            html.push_str(&format!(
                "<p data-unused='{i}'>unmatched paragraph {i} with &amp; text</p>"
            ));
        }
    }
    html.push_str("</article>");
    html
}

fn consume_text(store: &scah::Store<'_, '_>, selector: &str) {
    if let Some(elements) = store.get(selector) {
        for element in elements {
            black_box(element.raw_text(store));
            black_box(element.text(store));
        }
    }
}

fn bench_text_extraction_gate(c: &mut Criterion) {
    let prose = prose_html(SIZE);
    let sparse = sparse_matches_html(SIZE);
    let mut group = c.benchmark_group("text_extraction_gate");
    group.sample_size(50);

    group.throughput(Throughput::Bytes(prose.len() as u64));

    let no_content = [Query::all("p", Save::none()).unwrap().build()];
    group.bench_with_input(BenchmarkId::new("no_content", SIZE), &prose, |b, html| {
        b.iter(|| black_box(parse(black_box(html), black_box(&no_content)).unwrap()))
    });

    let inner_html = [Query::all("p", Save::only_inner_html()).unwrap().build()];
    group.bench_with_input(
        BenchmarkId::new("inner_html_only", SIZE),
        &prose,
        |b, html| b.iter(|| black_box(parse(black_box(html), black_box(&inner_html)).unwrap())),
    );

    let no_matches = [Query::all(
        "article > span.missing",
        Save::only_text().without_attributes(),
    )
    .unwrap()
    .build()];
    group.bench_with_input(
        BenchmarkId::new("text_only_no_matches", SIZE),
        &prose,
        |b, html| b.iter(|| black_box(parse(black_box(html), black_box(&no_matches)).unwrap())),
    );

    let text = [Query::all("p", Save::only_text().without_attributes())
        .unwrap()
        .build()];
    group.bench_with_input(
        BenchmarkId::new("text_only_prose", SIZE),
        &prose,
        |b, html| {
            b.iter(|| {
                let store = parse(black_box(html), black_box(&text)).unwrap();
                consume_text(&store, "p");
                black_box(store)
            })
        },
    );

    let raw = [Query::all("p", Save::only_raw_text().without_attributes())
        .unwrap()
        .build()];
    group.bench_with_input(BenchmarkId::new("raw_only", SIZE), &prose, |b, html| {
        b.iter(|| {
            let store = parse(black_box(html), black_box(&raw)).unwrap();
            consume_text(&store, "p");
            black_box(store)
        })
    });

    group.throughput(Throughput::Bytes(sparse.len() as u64));
    let sparse_text = [Query::all("p.hit", Save::only_text().without_attributes())
        .unwrap()
        .build()];
    group.bench_with_input(
        BenchmarkId::new("text_only_sparse_matches", SIZE),
        &sparse,
        |b, html| {
            b.iter(|| {
                let store = parse(black_box(html), black_box(&sparse_text)).unwrap();
                consume_text(&store, "p.hit");
                black_box(store)
            })
        },
    );

    group.finish();
}

criterion_group!(benches, bench_text_extraction_gate);
criterion_main!(benches);
