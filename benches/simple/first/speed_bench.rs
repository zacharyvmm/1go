use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use lexbor_css::HtmlDocument;
use lol_html::errors::RewritingError;
use lol_html::{HtmlRewriter, Settings, element};
use lxml::HtmlDocument as LxmlDocument;
use scah::{Query, Save, parse};
use scraper::{Html, Selector};
use std::error::Error;
use std::fmt;
use std::hint::black_box;
use tl::ParserOptions;

#[derive(Debug)]
struct StopParsing;

impl fmt::Display for StopParsing {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Stop parsing")
    }
}

impl Error for StopParsing {}

const QUERY: &str = black_box("a");
const COMPARISON_SAVE: Save = Save {
    inner_html: true,
    raw_text: false,
    text: true,
    attributes: true,
};

fn generate_html(count: usize) -> String {
    let mut html = String::with_capacity(count * 100);
    html.push_str("<html><body><div id='content'>");
    for i in 0..count {
        // Added some entities (&lt;) and bold tags (<b>) to make text extraction work harder
        html.push_str(&format!(
            r#"<div class="article"><a href="/post/{}"><b>Post</b> &lt;{}&gt;</a></div>"#,
            i, i
        ));
    }
    html.push_str("</div></body></html>");
    html
}

fn bench_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("simple_first_selection_comparison");

    for size in [100, 1_000, 10_000].iter() {
        let content = generate_html(*size);
        group.throughput(Throughput::Bytes(content.len() as u64));

        group.bench_with_input(BenchmarkId::new("scah", size), &content, |b, html| {
            b.iter(|| {
                let queries = &[Query::first(black_box(QUERY), COMPARISON_SAVE)
                    .expect("simple bench selector should parse")
                    .build()];

                let store = parse(html, queries).unwrap();

                let element = store.get(QUERY).unwrap().next().unwrap();

                black_box(&element.attributes(&store));
                black_box(&element.inner_html);
                black_box(&element.text(&store));
            })
        });

        group.bench_with_input(BenchmarkId::new("tl", size), &content, |b, html| {
            b.iter(|| {
                let dom = tl::parse(html, ParserOptions::default()).unwrap();
                let parser = dom.parser();
                let node_handle = dom.query_selector(QUERY).unwrap().next().unwrap();

                if let Some(node) = node_handle.get(parser) {
                    let attributes = node.as_tag().unwrap().attributes();
                    black_box(attributes.get("href"));
                    black_box(node.inner_html(parser));
                    black_box(node.inner_text(parser));
                }
            })
        });

        group.bench_with_input(BenchmarkId::new("scraper", size), &content, |b, html| {
            b.iter(|| {
                let document = Html::parse_document(html);
                let selector = Selector::parse(QUERY).unwrap();

                let element = document.select(&selector).next().unwrap();
                black_box(element.attr("href"));
                black_box(element.inner_html());
                black_box(element.text().collect::<Vec<&str>>());
            })
        });

        group.bench_with_input(BenchmarkId::new("lexbor", size), &content, |b, html| {
            b.iter(|| {
                let doc = HtmlDocument::parse(html.as_str()).expect("Failed to parse HTML");
                let nodes = doc.select(QUERY);

                let node = nodes.first().unwrap();
                // TODO: I need to add attributes and innerhtml for lexbor
                black_box(node.text_content());
                black_box(node.inner_html());
                black_box(node.attributes());
            })
        });

        group.bench_with_input(BenchmarkId::new("lxml", size), &content, |b, html| {
            b.iter(|| {
                let doc = LxmlDocument::new(html).expect("Failed to parse HTML");
                let nodes = doc.select(QUERY);
                let node = nodes.iter().next().unwrap();

                black_box(node.get_attribute("href"));
                black_box(node.inner_html());
                black_box(node.text_content());
            })
        });

        group.bench_with_input(BenchmarkId::new("lol_html", size), &content, |b, html| {
            b.iter(|| {
                let mut rewriter = HtmlRewriter::new(
                    Settings {
                        element_content_handlers: vec![element!(QUERY, |el| {
                            black_box(el.get_attribute("href"));
                            Err(Box::new(StopParsing))
                        })],
                        ..Settings::default()
                    },
                    |_: &[u8]| {},
                );
                let res = rewriter.write(html.as_bytes());
                match res {
                    Err(RewritingError::ContentHandlerError(e)) => {
                        if e.downcast_ref::<StopParsing>().is_some() {
                            return;
                        }
                        panic!("Unexpected error: {}", e);
                    }
                    Ok(_) => {
                        // If we didn't find anything, we must call end() to finish
                        rewriter.end().unwrap();
                    }
                    Err(e) => panic!("Unexpected rewriting error: {}", e),
                }
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_comparison);
criterion_main!(benches);
