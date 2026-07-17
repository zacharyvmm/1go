// Validation failure tests for SCaH regression benchmarks.
//
// These tests demonstrate that each validator correctly rejects corrupted
// results. They run as a standard `#[test]` target so `cargo test` discovers
// them independently of the Criterion harness.

#[path = "../support/fixtures.rs"]
mod fixtures;
#[path = "../support/validation.rs"]
mod validation;

use scah::{Query, Save, Store};

fn parse_html<'a>(html: &'a str, queries: &'a [Query<'a>]) -> Store<'a, 'a> {
    scah::parse(html, queries).unwrap()
}

fn build_all_queries(selector: &str, save: Save) -> Vec<Query<'_>> {
    vec![Query::all(selector, save).unwrap().build()]
}
/// Build a nested_all query matching the product catalog selectors.
fn build_nested_all_query() -> Query<'static> {
    Query::all("div.product", Save::all())
        .expect("parent selector should parse")
        .then(|product| {
            Ok([
                product.all("> h1", Save::all())?,
                product.all("> span.rating", Save::all())?,
                product.all("> p.description", Save::all())?,
            ])
        })
        .expect("child selectors should parse")
        .build()
}

/// Validator must reject wrong element count.
#[test]
#[should_panic(expected = "expected 99 matches")]
fn reject_wrong_element_count() {
    let html = fixtures::generate_link_list_html(100);
    let queries = build_all_queries("a", Save::all());
    let store = parse_html(&html, &queries);
    // Pass wrong expected count to trigger the panic.
    validation::assert_save_all_result(&store, "a", 99);
}

/// Validator must reject a middle element with missing inner HTML.
#[test]
#[should_panic(expected = "wrong inner HTML")]
fn reject_missing_inner_html_middle() {
    let mut html = String::with_capacity(100 * 120 + 200);
    html.push_str("<html><body><div id='content'>");
    for i in 0..100 {
        if i == 50 {
            html.push_str(&format!(
                r#"<div class="article"><a href="/post/{i}">Post &lt;{i}&gt;</a></div>"#
            ));
        } else {
            html.push_str(&format!(
                r#"<div class="article"><a href="/post/{i}"><b>Post</b> &lt;{i}&gt;</a></div>"#
            ));
        }
    }
    html.push_str("</div></body></html>");

    let queries = build_all_queries("a", Save::only_inner_html());
    let store = parse_html(&html, &queries);
    validation::assert_save_inner_html_result(&store, "a", 100);
}

/// Validator must reject a middle element with missing text content.
#[test]
#[should_panic(expected = "wrong text content")]
fn reject_missing_text_middle() {
    let mut html = String::with_capacity(100 * 120 + 200);
    html.push_str("<html><body><div id='content'>");
    for i in 0..100 {
        if i == 50 {
            html.push_str(&format!(
                r#"<div class="article"><a href="/post/{i}"><b>Post</b> Wrong Text</a></div>"#
            ));
        } else {
            html.push_str(&format!(
                r#"<div class="article"><a href="/post/{i}"><b>Post</b> &lt;{i}&gt;</a></div>"#
            ));
        }
    }
    html.push_str("</div></body></html>");

    let queries = build_all_queries("a", Save::only_text_content());
    let store = parse_html(&html, &queries);
    validation::assert_save_text_result(&store, "a", 100);
}

/// Validator must reject a result associated with the wrong index (wrong href).
#[test]
#[should_panic(expected = "expected href=")]
fn reject_wrong_index_href() {
    let mut html = String::with_capacity(100 * 120 + 200);
    html.push_str("<html><body><div id='content'>");
    for i in 0..100 {
        if i == 50 {
            html.push_str(&format!(
                r#"<div class="article"><a href="/post/0"><b>Post</b> &lt;{i}&gt;</a></div>"#
            ));
        } else {
            html.push_str(&format!(
                r#"<div class="article"><a href="/post/{i}"><b>Post</b> &lt;{i}&gt;</a></div>"#
            ));
        }
    }
    html.push_str("</div></body></html>");

    let queries = build_all_queries("a", Save::all());
    let store = parse_html(&html, &queries);
    validation::assert_save_all_result(&store, "a", 100);
}

/// Validator must reject corrupted parent saved data with correct children.
#[test]
#[should_panic(expected = "wrong parent inner HTML")]
fn reject_corrupted_parent_saved_data() {
    let mut html = String::with_capacity(10 * 200 + 200);
    html.push_str(r#"<html><body><section id="products">"#);
    for i in 1..=10 {
        let rating = ((i - 1) % 5) + 1;
        if i == 5 {
            // Keep class="product" so the selector matches, but corrupt the inner
            // HTML by changing the h1 content so expected_product_inner_html fails.
            html.push_str(&format!(
                r#"<div class="product"><h1>Wrong Title</h1><span class="rating">{rating}/5</span><p class="description">Description #{i}</p></div>"#
            ));
        } else {
            html.push_str(&format!(
                r#"<div class="product"><h1>Product #{i}</h1><span class="rating">{rating}/5</span><p class="description">Description #{i}</p></div>"#
            ));
        }
    }
    html.push_str("</section></body></html>");

    let queries = &[build_nested_all_query()];
    let store = parse_html(&html, queries);
    validation::assert_product_catalog_all(
        &store,
        "div.product",
        "> h1",
        "> span.rating",
        "> p.description",
        10,
    );
}
/// Validator must reject a multi-query element returned under the wrong selector.
#[test]
#[should_panic(expected = "expected class class-")]
fn reject_multi_query_wrong_selector() {
    let mut html = String::with_capacity(100 * 120 + 200);
    html.push_str("<html><body><div id='content'>");
    for i in 0..100 {
        let class_idx = i % 4;
        if i == 4 {
            // Swap: element 4 gets class-1 instead of class-0 (mismatches its data-index).
            html.push_str(&format!(
                r#"<div class="item class-1" data-index="{i}"><span>Item {i}</span></div>"#
            ));
        } else if i == 5 {
            // Swap: element 5 gets class-0 instead of class-1 (balances counts).
            html.push_str(&format!(
                r#"<div class="item class-0" data-index="{i}"><span>Item {i}</span></div>"#
            ));
        } else {
            html.push_str(&format!(
                r#"<div class="item class-{class_idx}" data-index="{i}"><span>Item {i}</span></div>"#
            ));
        }
    }
    html.push_str("</div></body></html>");

    let selectors: Vec<String> = (0..4).map(|i| format!(".class-{i}")).collect();
    let queries: Vec<Query<'_>> = selectors
        .iter()
        .map(|sel| Query::all(sel.as_str(), Save::all()).unwrap().build())
        .collect();
    let store = parse_html(&html, &queries);
    validation::assert_multi_query_results(&store, 100, 4);
}
