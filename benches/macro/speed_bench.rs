use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse, query};
use std::hint::black_box;

fn generate_html(count: usize) -> String {
    let mut html = String::with_capacity(count * 160);
    html.push_str("<html><body><main>");
    for i in 0..count {
        html.push_str(&format!(
            r#"<article class="post"><h1>Title {i}</h1><section><a href="/post/{i}">Post {i}</a><span>Summary {i}</span></section></article>"#
        ));
    }
    html.push_str("</main></body></html>");
    html
}

fn runtime_all_query<'a>() -> scah::Query<'a> {
    Query::all("main > article.post", Save::none())
        .expect("macro benchmark root selector should parse")
        .then(|article| {
            Ok([
                article.first("h1", Save::only_text())?,
                article.all("> section > a[href]", Save::all())?,
                article.first("> section > span", Save::only_text())?,
            ])
        })
        .expect("macro benchmark child selectors should parse")
        .build()
}

fn runtime_first_query<'a>() -> scah::Query<'a> {
    Query::first("main > article.post", Save::none())
        .expect("macro benchmark root selector should parse")
        .then(|article| {
            Ok([
                article.first("h1", Save::only_text())?,
                article.first("> section > a[href]", Save::all())?,
            ])
        })
        .expect("macro benchmark child selectors should parse")
        .build()
}

fn bench_macro_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("macro_query_all_comparison");

    for size in [100, 1_000, 10_000] {
        let content = generate_html(size);
        group.throughput(Throughput::Bytes(content.len() as u64));

        group.bench_with_input(
            BenchmarkId::new("runtime_builder", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let query = runtime_all_query();
                    let queries = [query];
                    let store = parse(html, &queries).unwrap();
                    for article in store.get("main > article.post").unwrap() {
                        black_box(article.get(&store, "> section > a[href]").unwrap().count());
                        black_box(article.get(&store, "h1").unwrap().count());
                        black_box(article.get(&store, "> section > span").unwrap().count());
                    }
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("query_macro", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let query = query! {
                        all("main > article.post", Save::none()) => {
                            first("h1", Save::only_text()),
                            all("> section > a[href]", Save::all()),
                            first("> section > span", Save::only_text()),
                        }
                    };
                    let queries = [query];
                    let store = parse(html, &queries).unwrap();
                    for article in store.get("main > article.post").unwrap() {
                        black_box(article.get(&store, "> section > a[href]").unwrap().count());
                        black_box(article.get(&store, "h1").unwrap().count());
                        black_box(article.get(&store, "> section > span").unwrap().count());
                    }
                })
            },
        );
    }

    group.finish();
}

fn bench_macro_first(c: &mut Criterion) {
    let mut group = c.benchmark_group("macro_query_first_comparison");

    for size in [100, 1_000, 10_000] {
        let content = generate_html(size);
        group.throughput(Throughput::Bytes(content.len() as u64));

        group.bench_with_input(
            BenchmarkId::new("runtime_builder", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let query = runtime_first_query();
                    let queries = [query];
                    let store = parse(html, &queries).unwrap();
                    let root = store
                        .get("main > article.post")
                        .unwrap()
                        .next()
                        .expect("first query should match");
                    black_box(root.attribute(&store, "class"));
                    black_box(root.get(&store, "h1").unwrap().count());
                    black_box(root.get(&store, "> section > a[href]").unwrap().count());
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("query_macro", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let query = query! {
                        first("main > article.post", Save::none()) => {
                            first("h1", Save::only_text()),
                            first("> section > a[href]", Save::all()),
                        }
                    };
                    let queries = [query];
                    let store = parse(html, &queries).unwrap();
                    let root = store
                        .get("main > article.post")
                        .unwrap()
                        .next()
                        .expect("first query should match");
                    black_box(root.attribute(&store, "class"));
                    black_box(root.get(&store, "h1").unwrap().count());
                    black_box(root.get(&store, "> section > a[href]").unwrap().count());
                })
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_macro_all, bench_macro_first);
criterion_main!(benches);
