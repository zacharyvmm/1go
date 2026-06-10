//! Benchmarks for bare ID selector performance.
//!
//! Each element has a unique `id`. The query `#link-0` matches exactly one
//! element, testing ID lookup speed (single-element match path).

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use lexbor_css::HtmlDocument;
use lol_html::{HtmlRewriter, Settings, element};
use lxml::HtmlDocument as LxmlDocument;
use scah::{Query, Save, parse};
use scraper::{Html, Selector};
use std::hint::black_box;
use tl::ParserOptions;

const QUERY: &str = "#link-0";

fn generate_html(count: usize) -> String {
    let mut html = String::with_capacity(count * 100);
    html.push_str("<html><body><div id='content'>");
    for i in 0..count {
        html.push_str(&format!(
            r#"<div class="article"><a id="link-{i}" href="/post/{i}"><b>Post</b> &lt;{i}&gt;</a></div>"#,
            i = i
        ));
    }
    html.push_str("</div></body></html>");
    html
}

fn consume_scah_results(store: &scah::Store<'_, '_>) {
    if let Some(elements) = store.get(QUERY) {
        for element in elements {
            black_box(&element.attributes(store));
            black_box(&element.inner_html);
            black_box(&element.text_content(store));
        }
    }
}

fn bench_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("simple_all_id_selection");

    for size in [100, 1_000, 10_000].iter() {
        let content = generate_html(*size);
        group.throughput(Throughput::Bytes(content.len() as u64));

        group.bench_with_input(
            BenchmarkId::new("scah_query_build_only", size),
            size,
            |b, _| {
                b.iter(|| {
                    let query = Query::all(black_box(QUERY), Save::all())
                        .expect("id bench selector should parse")
                        .build();
                    black_box(query);
                })
            },
        );

        let save_none_queries = &[Query::all(QUERY, Save::none())
            .expect("id bench selector should parse")
            .build()];
        group.bench_with_input(
            BenchmarkId::new("scah_parse_prebuilt_save_none", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(save_none_queries));
                    black_box(store);
                })
            },
        );

        let save_inner_html_queries = &[Query::all(QUERY, Save::only_inner_html())
            .expect("id bench selector should parse")
            .build()];
        group.bench_with_input(
            BenchmarkId::new("scah_parse_prebuilt_save_inner_html", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(save_inner_html_queries));
                    black_box(store);
                })
            },
        );

        let save_text_queries = &[Query::all(QUERY, Save::only_text_content())
            .expect("id bench selector should parse")
            .build()];
        group.bench_with_input(
            BenchmarkId::new("scah_parse_prebuilt_save_text", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(save_text_queries));
                    consume_scah_results(black_box(&store));
                })
            },
        );

        let save_all_queries = &[Query::all(QUERY, Save::all())
            .expect("id bench selector should parse")
            .build()];
        group.bench_with_input(
            BenchmarkId::new("scah_parse_prebuilt_save_all", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(save_all_queries));
                    consume_scah_results(black_box(&store));
                })
            },
        );

        group.bench_with_input(BenchmarkId::new("scah", size), &content, |b, html| {
            b.iter(|| {
                let queries = &[Query::all(QUERY, Save::all())
                    .expect("id bench selector should parse")
                    .build()];
                let store = parse(html, queries);

                for element in store.get(QUERY).unwrap() {
                    black_box(&element.attributes(&store));
                    black_box(&element.inner_html);
                    black_box(&element.text_content(&store));
                }
            })
        });

        group.bench_with_input(BenchmarkId::new("tl", size), &content, |b, html| {
            b.iter(|| {
                let dom = tl::parse(html, ParserOptions::default()).unwrap();
                let parser = dom.parser();
                let query = dom.query_selector(QUERY).unwrap();

                for node_handle in query {
                    if let Some(node) = node_handle.get(parser) {
                        let attributes = node.as_tag().unwrap().attributes();
                        black_box(attributes.get("href"));
                        black_box(node.inner_html(parser));
                        black_box(node.inner_text(parser));
                    }
                }
            })
        });

        group.bench_with_input(BenchmarkId::new("scraper", size), &content, |b, html| {
            b.iter(|| {
                let document = Html::parse_document(html);
                let selector = Selector::parse(QUERY).unwrap();

                for element in document.select(&selector) {
                    black_box(element.attr("href"));
                    black_box(element.inner_html());
                    black_box(element.text().collect::<Vec<&str>>());
                }
            })
        });

        group.bench_with_input(BenchmarkId::new("lexbor", size), &content, |b, html| {
            b.iter(|| {
                let doc = HtmlDocument::parse(html.as_str()).expect("Failed to parse HTML");
                let nodes = doc.select(QUERY);

                for node in nodes.iter() {
                    black_box(node.text_content());
                    black_box(node.inner_html());
                    black_box(node.attributes());
                }
            })
        });

        group.bench_with_input(BenchmarkId::new("lxml", size), &content, |b, html| {
            b.iter(|| {
                let doc = LxmlDocument::new(html).expect("Failed to parse HTML");
                let nodes = doc.select(QUERY);

                for node in nodes.iter() {
                    black_box(node.get_attribute("href"));
                    black_box(node.inner_html());
                    black_box(node.text_content());
                }
            })
        });

        group.bench_with_input(BenchmarkId::new("lol_html", size), &content, |b, html| {
            b.iter(|| {
                let mut rewriter = HtmlRewriter::new(
                    Settings::new()
                        .append_element_content_handler(element!(QUERY, |el| {
                            black_box(el.get_attribute("href"));
                            Ok(())
                        })),
                    |_: &[u8]| {},
                );
                rewriter.write(html.as_bytes()).unwrap();
                rewriter.end().unwrap();
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_comparison);
criterion_main!(benches);
