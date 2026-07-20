//! Instrumented cursor-statistics tooling for `scah`.
//!
//! This package depends on `scah` with `bench-internals` enabled. Production
//! throughput comparisons must use `scah-benches`, which compiles `scah`
//! without that feature.

use scah::Query;
use scah::Save;
use scah::bench_internals::{CursorStatsSnapshot, parse_with_cursor_stats};

pub const DEPTHS: &[u16] = &[8, 32, 128, 512];

#[derive(Clone, Copy)]
pub enum QueryKind {
    All,
    ThenDivAllP,
    ThenArticleFirstDivGtP,
}

#[derive(Clone, Copy)]
pub struct CursorCase {
    pub name: &'static str,
    pub selector: &'static str,
    pub html: fn(u16) -> String,
    pub query_kind: QueryKind,
    pub max_resident: fn(u16) -> Option<usize>,
}

/// Case 1: descendant domination — `div p` with one deepest `p`.
pub fn html_descendant_domination(depth: u16) -> String {
    format!(
        "{opens}<p>x</p>{closes}",
        opens = "<div>".repeat(depth as usize),
        closes = "</div>".repeat(depth as usize),
    )
}

/// Case 2: child prefix + descendant — `div > div p`.
pub fn html_child_prefix_descendant(depth: u16) -> String {
    html_descendant_domination(depth)
}

/// Case 3: mixed — `main > div p` with deep nest under main>div.
pub fn html_mixed_main_div_p(depth: u16) -> String {
    format!(
        "<main>{opens}<p>x</p>{closes}</main>",
        opens = "<div>".repeat(depth as usize),
        closes = "</div>".repeat(depth as usize),
    )
}

/// Case 4: non-dominance control — `div > p`, each level has a direct-child `p`.
pub fn html_non_dominance(depth: u16) -> String {
    let mut html = String::from("<div>");
    for _ in 0..depth {
        html.push_str("<p></p><div>");
    }
    html.push_str("<p></p>");
    html.push_str(&"</div>".repeat((depth + 1) as usize));
    html
}

/// Case 5: `.then()` output scopes with shared descendant `p` matches.
pub fn html_then_scopes(depth: u16) -> String {
    html_descendant_domination(depth)
}

/// Case 6: sequential child-scoped `First` ownership under sibling articles.
///
/// Trailing `<section>` content keeps the winner resident after the selected
/// `div` closes, exercising delayed same-scope admission until `</article>`.
pub fn html_then_first_sequential(count: u16) -> String {
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

/// Case 7: nested child-scoped `First` ownership with overlapping open articles.
pub fn html_then_first_nested(depth: u16) -> String {
    let mut html = String::new();
    for _ in 0..depth {
        html.push_str("<article><div><p>x</p></div>");
    }
    for _ in 0..depth {
        html.push_str("<aside>tail</aside></article>");
    }
    html
}

pub fn build_query(kind: QueryKind, selector: &str) -> Query<'_> {
    match kind {
        QueryKind::All => Query::all(selector, Save::none()).unwrap().build(),
        QueryKind::ThenDivAllP => Query::all("div", Save::none())
            .unwrap()
            .then(|div| Ok([div.all("p", Save::none())?]))
            .unwrap()
            .build(),
        QueryKind::ThenArticleFirstDivGtP => Query::all("article", Save::none())
            .unwrap()
            .then(|article| Ok([article.first("div > p", Save::none())?]))
            .unwrap()
            .build(),
    }
}

pub fn cursor_cases() -> &'static [CursorCase] {
    &[
        CursorCase {
            name: "descendant_div_p",
            selector: "div p",
            html: html_descendant_domination,
            query_kind: QueryKind::All,
            max_resident: |_| Some(3),
        },
        CursorCase {
            name: "child_prefix_div_gt_div_p",
            selector: "div > div p",
            html: html_child_prefix_descendant,
            query_kind: QueryKind::All,
            max_resident: |depth| Some(depth as usize + 3),
        },
        CursorCase {
            name: "mixed_main_gt_div_p",
            selector: "main > div p",
            html: html_mixed_main_div_p,
            query_kind: QueryKind::All,
            max_resident: |_| None,
        },
        CursorCase {
            name: "non_dominance_div_gt_p",
            selector: "div > p",
            html: html_non_dominance,
            query_kind: QueryKind::All,
            max_resident: |_| None,
        },
        CursorCase {
            name: "then_div_all_p",
            selector: "div",
            html: html_then_scopes,
            query_kind: QueryKind::ThenDivAllP,
            max_resident: |_| None,
        },
        CursorCase {
            name: "then_article_first_div_gt_p_sequential",
            selector: "article",
            html: html_then_first_sequential,
            query_kind: QueryKind::ThenArticleFirstDivGtP,
            // Constant across sibling article counts: one open article owns one
            // winner plus a fixed handful of live obligations.
            max_resident: |_| Some(5),
        },
        CursorCase {
            name: "then_article_first_div_gt_p_nested",
            selector: "article",
            html: html_then_first_nested,
            query_kind: QueryKind::ThenArticleFirstDivGtP,
            // One completed winner + one article obligation per open nest, plus
            // a small constant for root / transient peers: 2*depth + 3.
            max_resident: |depth| Some(depth as usize * 2 + 3),
        },
    ]
}

pub fn measure_case(case: &CursorCase, depth: u16) -> CursorStatsSnapshot {
    let html = (case.html)(depth);
    let query = build_query(case.query_kind, case.selector);
    let queries = [query];
    parse_with_cursor_stats(&html, &queries)
        .expect("instrumented parse succeeds")
        .1
}
