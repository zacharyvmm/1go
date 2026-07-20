//! Adversarial nested-selector workloads for production parse throughput.
//!
//! Measures ordinary `scah::parse` performance on synthetic HTML at depths
//! 8, 32, 128, and 512. Cursor-count reporting lives in `scah-cursor-benches`.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::Query;
use scah::Save;
use scah::parse;
use std::hint::black_box;

const DEPTHS: &[u16] = &[8, 32, 128, 512];

/// Case 1: descendant domination — `div p` with one deepest `p`.
fn html_descendant_domination(depth: u16) -> String {
    format!(
        "{opens}<p>x</p>{closes}",
        opens = "<div>".repeat(depth as usize),
        closes = "</div>".repeat(depth as usize),
    )
}

/// Case 2: child prefix + descendant — `div > div p`.
fn html_child_prefix_descendant(depth: u16) -> String {
    html_descendant_domination(depth)
}

/// Case 3: mixed — `main > div p` with deep nest under main>div.
fn html_mixed_main_div_p(depth: u16) -> String {
    format!(
        "<main>{opens}<p>x</p>{closes}</main>",
        opens = "<div>".repeat(depth as usize),
        closes = "</div>".repeat(depth as usize),
    )
}

/// Case 4: non-dominance control — `div > p`, each level has a direct-child `p`.
fn html_non_dominance(depth: u16) -> String {
    let mut html = String::from("<div>");
    for _ in 0..depth {
        html.push_str("<p></p><div>");
    }
    html.push_str("<p></p>");
    html.push_str(&"</div>".repeat((depth + 1) as usize));
    html
}

/// Case 5: `.then()` output scopes with shared descendant `p` matches.
fn html_then_scopes(depth: u16) -> String {
    html_descendant_domination(depth)
}

/// Sequential child-scoped `First` ownership under sibling articles.
fn html_then_first_sequential(count: u16) -> String {
    let mut html = String::new();
    for _ in 0..count {
        html.push_str(
            "<article>\
                <div><p>x</p></div>\
                <section>\
                    <span>tail</span>\
                    <span>tail</span>\
                </section>\
            </article>",
        );
    }
    html
}

/// Nested child-scoped `First` ownership with overlapping open articles.
fn html_then_first_nested(depth: u16) -> String {
    let mut html = String::new();
    for _ in 0..depth {
        html.push_str("<article><div><p>x</p></div>");
    }
    for _ in 0..depth {
        html.push_str("<aside>tail</aside></article>");
    }
    html
}

fn bench_case<F>(
    c: &mut Criterion,
    group_name: &str,
    html_fn: F,
    selector: &str,
    build_query: fn(&str) -> Query<'_>,
) where
    F: Fn(u16) -> String,
{
    let mut group = c.benchmark_group(group_name);
    group.sample_size(10);

    for &depth in DEPTHS {
        let html = html_fn(depth);
        group.throughput(Throughput::Bytes(html.len() as u64));

        group.bench_with_input(
            BenchmarkId::new("parse", depth),
            &(html, selector),
            |b, (html, selector)| {
                let query = build_query(selector);
                b.iter(|| {
                    let store = parse(black_box(html), std::slice::from_ref(&query)).unwrap();
                    black_box(store.get(selector).unwrap().count());
                });
            },
        );
    }

    group.finish();
}

fn build_all(selector: &str) -> Query<'_> {
    Query::all(selector, Save::none()).unwrap().build()
}

fn build_then_div_p(_selector: &str) -> Query<'_> {
    Query::all("div", Save::none())
        .unwrap()
        .then(|div| Ok([div.all("p", Save::none())?]))
        .unwrap()
        .build()
}

fn build_then_article_first_div_p(_selector: &str) -> Query<'_> {
    Query::all("article", Save::none())
        .unwrap()
        .then(|article| Ok([article.first("div > p", Save::none())?]))
        .unwrap()
        .build()
}

fn bench_descendant_domination(c: &mut Criterion) {
    bench_case(
        c,
        "cursor_domination/descendant_div_p",
        html_descendant_domination,
        "div p",
        build_all,
    );
}

fn bench_child_prefix_descendant(c: &mut Criterion) {
    bench_case(
        c,
        "cursor_domination/child_prefix_div_gt_div_p",
        html_child_prefix_descendant,
        "div > div p",
        build_all,
    );
}

fn bench_mixed_main_div_p(c: &mut Criterion) {
    bench_case(
        c,
        "cursor_domination/mixed_main_gt_div_p",
        html_mixed_main_div_p,
        "main > div p",
        build_all,
    );
}

fn bench_non_dominance(c: &mut Criterion) {
    bench_case(
        c,
        "cursor_domination/non_dominance_div_gt_p",
        html_non_dominance,
        "div > p",
        build_all,
    );
}

fn bench_then_scopes(c: &mut Criterion) {
    bench_case(
        c,
        "cursor_domination/then_div_all_p",
        html_then_scopes,
        "div",
        build_then_div_p,
    );
}

fn bench_then_first_sequential(c: &mut Criterion) {
    bench_case(
        c,
        "cursor_domination/then_article_first_div_gt_p_sequential",
        html_then_first_sequential,
        "article",
        build_then_article_first_div_p,
    );
}

fn bench_then_first_nested(c: &mut Criterion) {
    bench_case(
        c,
        "cursor_domination/then_article_first_div_gt_p_nested",
        html_then_first_nested,
        "article",
        build_then_article_first_div_p,
    );
}

criterion_group!(
    benches,
    bench_descendant_domination,
    bench_child_prefix_descendant,
    bench_mixed_main_div_p,
    bench_non_dominance,
    bench_then_scopes,
    bench_then_first_sequential,
    bench_then_first_nested
);
criterion_main!(benches);
