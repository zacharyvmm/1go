use gungraun::{library_benchmark, library_benchmark_group, main};
use lol_html::errors::RewritingError;
use lxml::HtmlDocument as LxmlDocument;
use std::error::Error;
use std::fmt;
use std::hint::black_box;

#[derive(Debug)]
struct StopParsing;

impl fmt::Display for StopParsing {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Stop parsing")
    }
}

impl Error for StopParsing {}

const MAX_ELEMENT_LEN: usize = 1000;
const QUERY: &str = black_box("a");

fn setup_html() -> String {
    let mut html = String::new();
    html.push_str("<html><body><div id='content'>");
    for i in 0..MAX_ELEMENT_LEN {
        html.push_str(&format!(
            r#"<div class="article"><a href="/post/{}">Post {}</a></div>"#,
            i, i
        ));
    }
    html.push_str("</div></body></html>");
    html
}

use scah::{Query, Save, parse};
#[library_benchmark]
#[bench::scah(setup_html())]
fn bench_scah(html: String) {
    let queries = &[Query::first(QUERY, Save::all())
        .expect("simple bench selector should parse")
        .build()];

    let store = parse(&html, queries);

    let element = store.get(QUERY).unwrap().next().unwrap();

    black_box(&element.attributes(&store));
    black_box(&element.inner_html);
    black_box(&element.text_content(&store));
}

use scraper::{Html, Selector};
#[library_benchmark]
#[bench::scraper(setup_html())]
fn bench_scraper(html: String) {
    let document = Html::parse_document(&html);

    let selector = Selector::parse(QUERY).unwrap();

    let element = document.select(&selector).next().unwrap();
    black_box(element.attr("href"));
    black_box(element.inner_html());
    black_box(element.text().collect::<Vec<&str>>());
}

use tl::ParserOptions;
#[library_benchmark]
#[bench::tl(setup_html())]
fn bench_tl(html: String) {
    let dom = tl::parse(&html, ParserOptions::default()).unwrap();
    let parser = dom.parser();

    let node_handle = dom.query_selector(QUERY).unwrap().next().unwrap();

    if let Some(node) = node_handle.get(parser) {
        let attributes = node.as_tag().unwrap().attributes();
        black_box(attributes.get("href"));
        black_box(node.inner_html(parser));
        black_box(node.inner_text(parser));
    }
}

use lexbor_css::HtmlDocument;
#[library_benchmark]
#[bench::lexbor(setup_html())]
fn bench_lexbor(html: String) {
    let doc = HtmlDocument::new(html.as_str()).expect("Failed to parse HTML");
    let nodes = doc.select(QUERY);

    let node = nodes.first().unwrap();
    black_box(node.text_content());
    black_box(node.inner_html());
    black_box(node.attributes());
}

#[library_benchmark]
#[bench::lxml(setup_html())]
fn bench_lxml(html: String) {
    let doc = LxmlDocument::new(&html).expect("Failed to parse HTML");
    let nodes = doc.select(QUERY);
    let node = nodes.iter().next().unwrap();

    black_box(node.get_attribute("href"));
    black_box(node.inner_html());
    black_box(node.text_content());
}

use lol_html::{HtmlRewriter, Settings, element};
#[library_benchmark]
#[bench::lol_html(setup_html())]
fn bench_lol_html(html: String) {
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
            rewriter.end().unwrap();
        }
        Err(e) => panic!("Unexpected rewriting error: {}", e),
    }
}

library_benchmark_group!(
    name = comparison_group;
    benchmarks = bench_scah, bench_tl, bench_scraper, bench_lexbor, bench_lxml, bench_lol_html
);

main!(library_benchmark_groups = comparison_group);
