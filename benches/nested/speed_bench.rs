#[path = "../support/mod.rs"]
mod support;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use lexbor_css::HtmlDocument;
use lol_html::errors::RewritingError;
use lol_html::{HtmlRewriter, Settings, element, text};
use lxml::HtmlDocument as LxmlDocument;
#[allow(unused_imports)]
use scah::Save;
use scah::{parse, query};
use scraper::{Html, Selector};
use std::error::Error;
use std::fmt;
use std::hint::black_box;
use support::{
    PRODUCT_DESCRIPTION_GLOBAL_SELECTOR, PRODUCT_DESCRIPTION_SELECTOR,
    PRODUCT_RATING_GLOBAL_SELECTOR, PRODUCT_RATING_SELECTOR, PRODUCT_SELECTOR,
    PRODUCT_TITLE_GLOBAL_SELECTOR, PRODUCT_TITLE_SELECTOR, SPEED_BENCH_SIZES,
    generate_product_catalog_html,
};
use tl::ParserOptions;

#[derive(Debug)]
struct StopParsing;

impl fmt::Display for StopParsing {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Stop parsing")
    }
}

impl Error for StopParsing {}

const PRODUCT_XPATH: &str = "//div[@class='product']";
const PRODUCT_TITLE_XPATH: &str = "//div[@class='product']/h1";
const PRODUCT_RATING_XPATH: &str = "//div[@class='product']/span[@class='rating']";
const PRODUCT_DESCRIPTION_XPATH: &str = "//div[@class='product']/p[@class='description']";

fn bench_nested_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("nested_all_selection_comparison");

    for size in SPEED_BENCH_SIZES {
        let content = generate_product_catalog_html(size);
        group.throughput(Throughput::Bytes(content.len() as u64));

        group.bench_with_input(BenchmarkId::new("scah", size), &content, |b, html| {
            b.iter(|| {
                let queries = &[query! {
                    all("div.product", Save::all()) => {
                        first("> h1", Save::all()),
                        first("> span.rating", Save::all()),
                        first("> p.description", Save::all()),
                    }
                }];
                let store = parse(html, queries);

                for product in store.get(PRODUCT_SELECTOR).unwrap() {
                    black_box(product.attribute(&store, "class"));
                    black_box(product.inner_html);
                    black_box(product.text_content(&store));

                    let title = product
                        .get(&store, PRODUCT_TITLE_SELECTOR)
                        .unwrap()
                        .next()
                        .unwrap();
                    black_box(title.inner_html);
                    black_box(title.text_content(&store));

                    let rating = product
                        .get(&store, PRODUCT_RATING_SELECTOR)
                        .unwrap()
                        .next()
                        .unwrap();
                    black_box(rating.inner_html);
                    black_box(rating.text_content(&store));

                    let description = product
                        .get(&store, PRODUCT_DESCRIPTION_SELECTOR)
                        .unwrap()
                        .next()
                        .unwrap();
                    black_box(description.inner_html);
                    black_box(description.text_content(&store));
                }
            })
        });

        group.bench_with_input(BenchmarkId::new("tl", size), &content, |b, html| {
            b.iter(|| {
                let dom = tl::parse(html, ParserOptions::default()).unwrap();
                let parser = dom.parser();
                let products = dom.query_selector(PRODUCT_SELECTOR).unwrap();

                for handle in products {
                    let node = handle.get(parser).unwrap();
                    let tag = node.as_tag().unwrap();
                    black_box(tag.attributes().get("class"));
                    black_box(node.inner_html(parser));
                    black_box(node.inner_text(parser));

                    let title = tag
                        .query_selector(parser, "h1")
                        .unwrap()
                        .next()
                        .unwrap()
                        .get(parser)
                        .unwrap();
                    black_box(title.inner_html(parser));
                    black_box(title.inner_text(parser));

                    let rating = tag
                        .query_selector(parser, "span.rating")
                        .unwrap()
                        .next()
                        .unwrap()
                        .get(parser)
                        .unwrap();
                    black_box(rating.inner_html(parser));
                    black_box(rating.inner_text(parser));

                    let description = tag
                        .query_selector(parser, "p.description")
                        .unwrap()
                        .next()
                        .unwrap()
                        .get(parser)
                        .unwrap();
                    black_box(description.inner_html(parser));
                    black_box(description.inner_text(parser));
                }
            })
        });

        group.bench_with_input(BenchmarkId::new("scraper", size), &content, |b, html| {
            b.iter(|| {
                let document = Html::parse_document(html);
                let product_selector = Selector::parse(PRODUCT_SELECTOR).unwrap();
                let title_selector = Selector::parse("h1").unwrap();
                let rating_selector = Selector::parse("span.rating").unwrap();
                let description_selector = Selector::parse("p.description").unwrap();

                for product in document.select(&product_selector) {
                    black_box(product.attr("class"));
                    black_box(product.inner_html());
                    black_box(product.text().collect::<Vec<&str>>());

                    let title = product.select(&title_selector).next().unwrap();
                    black_box(title.inner_html());
                    black_box(title.text().collect::<Vec<&str>>());

                    let rating = product.select(&rating_selector).next().unwrap();
                    black_box(rating.inner_html());
                    black_box(rating.text().collect::<Vec<&str>>());

                    let description = product.select(&description_selector).next().unwrap();
                    black_box(description.inner_html());
                    black_box(description.text().collect::<Vec<&str>>());
                }
            })
        });

        group.bench_with_input(BenchmarkId::new("lexbor", size), &content, |b, html| {
            b.iter(|| {
                let doc = HtmlDocument::parse(html.as_str()).expect("Failed to parse HTML");

                for product in doc.select(PRODUCT_SELECTOR).iter() {
                    let attrs = product.attributes();
                    black_box(attrs.get("class"));
                    black_box(product.inner_html());
                    black_box(product.text_content());
                }

                for title in doc.select(PRODUCT_TITLE_GLOBAL_SELECTOR).iter() {
                    black_box(title.inner_html());
                    black_box(title.text_content());
                }

                for rating in doc.select(PRODUCT_RATING_GLOBAL_SELECTOR).iter() {
                    black_box(rating.inner_html());
                    black_box(rating.text_content());
                }

                for description in doc.select(PRODUCT_DESCRIPTION_GLOBAL_SELECTOR).iter() {
                    black_box(description.inner_html());
                    black_box(description.text_content());
                }
            })
        });

        group.bench_with_input(BenchmarkId::new("lxml", size), &content, |b, html| {
            b.iter(|| {
                let doc = LxmlDocument::new(html).expect("Failed to parse HTML");

                for product in doc.xpath(PRODUCT_XPATH).iter() {
                    black_box(product.get_attribute("class"));
                    black_box(product.inner_html());
                    black_box(product.text_content());
                }

                for title in doc.xpath(PRODUCT_TITLE_XPATH).iter() {
                    black_box(title.inner_html());
                    black_box(title.text_content());
                }

                for rating in doc.xpath(PRODUCT_RATING_XPATH).iter() {
                    black_box(rating.inner_html());
                    black_box(rating.text_content());
                }

                for description in doc.xpath(PRODUCT_DESCRIPTION_XPATH).iter() {
                    black_box(description.inner_html());
                    black_box(description.text_content());
                }
            })
        });

        group.bench_with_input(BenchmarkId::new("lol_html", size), &content, |b, html| {
            b.iter(|| {
                let mut rewriter = HtmlRewriter::new(
                    Settings {
                        element_content_handlers: vec![
                            element!("div.product", |el| {
                                black_box(el.get_attribute("class"));
                                Ok(())
                            }),
                            text!("div.product > h1", |t| {
                                black_box(t.as_str());
                                Ok(())
                            }),
                            text!("div.product > span.rating", |t| {
                                black_box(t.as_str());
                                Ok(())
                            }),
                            text!("div.product > p.description", |t| {
                                black_box(t.as_str());
                                Ok(())
                            }),
                        ],
                        ..Settings::default()
                    },
                    |_: &[u8]| {},
                );

                rewriter.write(html.as_bytes()).unwrap();
                rewriter.end().unwrap();
            })
        });
    }

    group.finish();
}

fn bench_nested_first(c: &mut Criterion) {
    let mut group = c.benchmark_group("nested_first_selection_comparison");

    for size in SPEED_BENCH_SIZES {
        let content = generate_product_catalog_html(size);
        group.throughput(Throughput::Bytes(content.len() as u64));

        group.bench_with_input(BenchmarkId::new("scah", size), &content, |b, html| {
            b.iter(|| {
                let queries = &[query! {
                    first("div.product", Save::all()) => {
                        first("> h1", Save::all()),
                        first("> span.rating", Save::all()),
                        first("> p.description", Save::all()),
                    }
                }];
                let store = parse(html, queries);

                let product = store.get(PRODUCT_SELECTOR).unwrap().next().unwrap();
                black_box(product.attribute(&store, "class"));
                black_box(product.inner_html);
                black_box(product.text_content(&store));

                let title = product
                    .get(&store, PRODUCT_TITLE_SELECTOR)
                    .unwrap()
                    .next()
                    .unwrap();
                black_box(title.inner_html);
                black_box(title.text_content(&store));

                let rating = product
                    .get(&store, PRODUCT_RATING_SELECTOR)
                    .unwrap()
                    .next()
                    .unwrap();
                black_box(rating.inner_html);
                black_box(rating.text_content(&store));

                let description = product
                    .get(&store, PRODUCT_DESCRIPTION_SELECTOR)
                    .unwrap()
                    .next()
                    .unwrap();
                black_box(description.inner_html);
                black_box(description.text_content(&store));
            })
        });

        group.bench_with_input(BenchmarkId::new("tl", size), &content, |b, html| {
            b.iter(|| {
                let dom = tl::parse(html, ParserOptions::default()).unwrap();
                let parser = dom.parser();
                let node = dom
                    .query_selector(PRODUCT_SELECTOR)
                    .unwrap()
                    .next()
                    .unwrap()
                    .get(parser)
                    .unwrap();
                let tag = node.as_tag().unwrap();

                black_box(tag.attributes().get("class"));
                black_box(node.inner_html(parser));
                black_box(node.inner_text(parser));

                let title = tag
                    .query_selector(parser, "h1")
                    .unwrap()
                    .next()
                    .unwrap()
                    .get(parser)
                    .unwrap();
                black_box(title.inner_html(parser));
                black_box(title.inner_text(parser));

                let rating = tag
                    .query_selector(parser, "span.rating")
                    .unwrap()
                    .next()
                    .unwrap()
                    .get(parser)
                    .unwrap();
                black_box(rating.inner_html(parser));
                black_box(rating.inner_text(parser));

                let description = tag
                    .query_selector(parser, "p.description")
                    .unwrap()
                    .next()
                    .unwrap()
                    .get(parser)
                    .unwrap();
                black_box(description.inner_html(parser));
                black_box(description.inner_text(parser));
            })
        });

        group.bench_with_input(BenchmarkId::new("scraper", size), &content, |b, html| {
            b.iter(|| {
                let document = Html::parse_document(html);
                let product_selector = Selector::parse(PRODUCT_SELECTOR).unwrap();
                let title_selector = Selector::parse("h1").unwrap();
                let rating_selector = Selector::parse("span.rating").unwrap();
                let description_selector = Selector::parse("p.description").unwrap();

                let product = document.select(&product_selector).next().unwrap();
                black_box(product.attr("class"));
                black_box(product.inner_html());
                black_box(product.text().collect::<Vec<&str>>());

                let title = product.select(&title_selector).next().unwrap();
                black_box(title.inner_html());
                black_box(title.text().collect::<Vec<&str>>());

                let rating = product.select(&rating_selector).next().unwrap();
                black_box(rating.inner_html());
                black_box(rating.text().collect::<Vec<&str>>());

                let description = product.select(&description_selector).next().unwrap();
                black_box(description.inner_html());
                black_box(description.text().collect::<Vec<&str>>());
            })
        });

        group.bench_with_input(BenchmarkId::new("lexbor", size), &content, |b, html| {
            b.iter(|| {
                let doc = HtmlDocument::parse(html.as_str()).expect("Failed to parse HTML");

                let product = doc.select(PRODUCT_SELECTOR);
                let product = product.first().unwrap();
                let attrs = product.attributes();
                black_box(attrs.get("class"));
                black_box(product.inner_html());
                black_box(product.text_content());

                let title = doc.select(PRODUCT_TITLE_GLOBAL_SELECTOR);
                let title = title.first().unwrap();
                black_box(title.inner_html());
                black_box(title.text_content());

                let rating = doc.select(PRODUCT_RATING_GLOBAL_SELECTOR);
                let rating = rating.first().unwrap();
                black_box(rating.inner_html());
                black_box(rating.text_content());

                let description = doc.select(PRODUCT_DESCRIPTION_GLOBAL_SELECTOR);
                let description = description.first().unwrap();
                black_box(description.inner_html());
                black_box(description.text_content());
            })
        });

        group.bench_with_input(BenchmarkId::new("lxml", size), &content, |b, html| {
            b.iter(|| {
                let doc = LxmlDocument::new(html).expect("Failed to parse HTML");

                let products = doc.xpath(PRODUCT_XPATH);
                let product = products.iter().next().unwrap();
                black_box(product.get_attribute("class"));
                black_box(product.inner_html());
                black_box(product.text_content());

                let titles = doc.xpath(PRODUCT_TITLE_XPATH);
                let title = titles.iter().next().unwrap();
                black_box(title.inner_html());
                black_box(title.text_content());

                let ratings = doc.xpath(PRODUCT_RATING_XPATH);
                let rating = ratings.iter().next().unwrap();
                black_box(rating.inner_html());
                black_box(rating.text_content());

                let descriptions = doc.xpath(PRODUCT_DESCRIPTION_XPATH);
                let description = descriptions.iter().next().unwrap();
                black_box(description.inner_html());
                black_box(description.text_content());
            })
        });

        group.bench_with_input(BenchmarkId::new("lol_html", size), &content, |b, html| {
            b.iter(|| {
                let mut rewriter = HtmlRewriter::new(
                    Settings {
                        element_content_handlers: vec![
                            element!("div.product", |el| {
                                black_box(el.get_attribute("class"));
                                Ok(())
                            }),
                            text!("div.product > h1", |t| {
                                black_box(t.as_str());
                                Ok(())
                            }),
                            text!("div.product > span.rating", |t| {
                                black_box(t.as_str());
                                Ok(())
                            }),
                            text!("div.product > p.description", |t| {
                                black_box(t.as_str());
                                Err(Box::new(StopParsing))
                            }),
                        ],
                        ..Settings::default()
                    },
                    |_: &[u8]| {},
                );

                match rewriter.write(html.as_bytes()) {
                    Err(RewritingError::ContentHandlerError(err)) => {
                        if err.downcast_ref::<StopParsing>().is_none() {
                            panic!("Unexpected error: {}", err);
                        }
                    }
                    Ok(_) => {
                        rewriter.end().unwrap();
                    }
                    Err(err) => panic!("Unexpected rewriting error: {}", err),
                }
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_nested_all, bench_nested_first);
criterion_main!(benches);
