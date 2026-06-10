#[path = "../support/mod.rs"]
#[allow(dead_code)]
mod support;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use lexbor_css::HtmlDocument;
use lol_html::{HtmlRewriter, Settings, element};
use lxml::HtmlDocument as LxmlDocument;
use scah::{Query, Save, parse};
use scraper::{Html, Selector};
use std::hint::black_box;
use tl::ParserOptions;

const QUERY: &str = "a";
const SPEC_HTML_FILE: &str = "html.spec.whatwg.org.html";

fn consume_scah_results(store: &scah::Store<'_, '_>) {
    if let Some(elements) = store.get(QUERY) {
        for element in elements {
            black_box(&element.attributes(store));
            black_box(&element.inner_html);
            black_box(&element.text_content(store));
        }
    }
}

fn bench_spec_links(c: &mut Criterion) {
    let mut group = c.benchmark_group("whatwg_html_spec_all_links");
    let content = support::load_bench_data(SPEC_HTML_FILE);
    group.throughput(Throughput::Bytes(content.len() as u64));

    group.bench_function("scah_query_build_only", |b| {
        b.iter(|| {
            let query = Query::all(black_box(QUERY), Save::all())
                .expect("spec selector should parse")
                .build();
            black_box(query);
        })
    });

    let save_none_queries = &[Query::all(QUERY, Save::none())
        .expect("spec selector should parse")
        .build()];
    group.bench_function("scah_parse_prebuilt_save_none", |b| {
        b.iter(|| {
            let store = parse(black_box(&content), black_box(save_none_queries));
            black_box(store);
        })
    });


    let save_inner_html_queries = &[Query::all(QUERY, Save::only_inner_html())
        .expect("spec selector should parse")
        .build()];
    group.bench_function("scah_parse_prebuilt_save_inner_html", |b| {
        b.iter(|| {
            let store = parse(black_box(&content), black_box(save_inner_html_queries));
            black_box(store);
        })
    });


    let save_text_queries = &[Query::all(QUERY, Save::only_text_content())
        .expect("spec selector should parse")
        .build()];
    group.bench_function("scah_parse_prebuilt_save_text", |b| {
        b.iter(|| {
            let store = parse(black_box(&content), black_box(save_text_queries));
            consume_scah_results(black_box(&store));
        })
    });


    let save_all_queries = &[Query::all(QUERY, Save::all())
        .expect("spec selector should parse")
        .build()];
    group.bench_function("scah_parse_prebuilt_save_all", |b| {
        b.iter(|| {
            let store = parse(black_box(&content), black_box(save_all_queries));
            consume_scah_results(black_box(&store));
        })
    });


    group.bench_function("scah", |b| {
        b.iter(|| {
            let queries = &[Query::all(QUERY, Save::all())
                .expect("spec selector should parse")
                .build()];
            let store = parse(&content, queries);

            for element in store.get(QUERY).unwrap() {
                black_box(&element.attributes(&store));
                black_box(&element.inner_html);
                black_box(&element.text_content(&store));
            }
        })
    });


    group.bench_function("tl", |b| {
        b.iter(|| {
            let dom = tl::parse(&content, ParserOptions::default()).unwrap();
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

    group.bench_function("scraper", |b| {
        b.iter(|| {
            let document = Html::parse_document(&content);
            let selector = Selector::parse(QUERY).unwrap();

            for element in document.select(&selector) {
                black_box(element.attr("href"));
                black_box(element.inner_html());
                black_box(element.text().collect::<Vec<&str>>());
            }
        })
    });

    group.bench_function("lexbor", |b| {
        b.iter(|| {
            let doc = HtmlDocument::parse(content.as_str()).expect("Failed to parse HTML");
            let nodes = doc.select(QUERY);

            for node in nodes.iter() {
                black_box(node.text_content());
                black_box(node.inner_html());
                black_box(node.attributes());
            }
        })
    });

    group.bench_function("lxml", |b| {
        b.iter(|| {
            let doc = LxmlDocument::new(&content).expect("Failed to parse HTML");
            let nodes = doc.select(QUERY);

            for node in nodes.iter() {
                black_box(node.get_attribute("href"));
                black_box(node.inner_html());
                black_box(node.text_content());
            }
        })
    });

    group.bench_function("lol_html", |b| {
        b.iter(|| {
            let mut rewriter = HtmlRewriter::new(
                Settings::new()
                    .append_element_content_handler(element!(QUERY, |el| {
                        black_box(el.get_attribute("href"));
                        Ok(())
                    })),
                |_: &[u8]| {},
            );
            rewriter.write(content.as_bytes()).unwrap();
            rewriter.end().unwrap();
        })
    });

    group.finish();
}

criterion_group!(benches, bench_spec_links);
criterion_main!(benches);
