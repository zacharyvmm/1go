//! Microbenchmarks for `ascii_ci_tag_match!` tag classification.
//!
//! These benchmarks measure the helper macro directly — not full parser
//! throughput — so that tag-matching strategy choices are isolated and
//! visible. Each group exercises a realistic tag distribution against
//! the void-element, close-scope, and scope-barrier tag sets.
//!
//! Strategy chosen: exact lowercase `match` + uppercase fallback.
//! See `crates/scah/src/support/macros.rs`.

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use scah::ascii_ci_tag_match;

// ── tag sets from the production code ──────────────────────────────

/// Void elements (from `XHtmlElement::is_self_closing`)
const VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Tags that close an open `<p>` (from `closes_open_p`)
const CLOSES_OPEN_P_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "div",
    "dl",
    "fieldset",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
];

/// Tags in the default scope barrier (from `is_scope_barrier` ScopeKind::Default)
const DEFAULT_SCOPE_BARRIER_TAGS: &[&str] = &["applet", "marquee", "object", "table", "td", "th"];

// ── realistic tag name distributions ───────────────────────────────

/// Common lowercase HTML tags (hit case)
const LOWERCASE_HITS: &[&str] = &["div", "p", "a", "span", "li", "td", "input", "img"];

/// Custom element / uncommon lowercase tags (miss case)
const LOWERCASE_MISSES: &[&str] = &["article-card", "custom-element", "svg", "path", "section"];

/// Mixed-case variants of common tags (uppercase fallback path)
const MIXED_CASE_HITS: &[&str] = &["DIV", "Li", "TaBlE", "INPUT"];

/// Long custom element names (worst-case for byte-by-byte comparison)
const LONG_MISSES: &[&str] = &[
    "very-long-custom-element-name-that-will-never-match",
    "another-extremely-long-component-name-xyz",
    "my-application-shell-container-wrapper",
];

// ── benchmarks ─────────────────────────────────────────────────────

fn bench_void_elements(c: &mut Criterion) {
    let mut group = c.benchmark_group("tag_classification_void_elements");

    // Lowercase hits — fast path exercised
    for tag in LOWERCASE_HITS {
        group.bench_with_input(BenchmarkId::new("lowercase_hit", *tag), tag, |b, tag| {
            b.iter(|| {
                black_box(ascii_ci_tag_match!(
                    black_box(tag),
                    "area",
                    "base",
                    "br",
                    "col",
                    "embed",
                    "hr",
                    "img",
                    "input",
                    "link",
                    "meta",
                    "param",
                    "source",
                    "track",
                    "wbr",
                ))
            })
        });
    }

    // Lowercase misses — fast path exercised, false result
    for tag in LOWERCASE_MISSES {
        group.bench_with_input(BenchmarkId::new("lowercase_miss", *tag), tag, |b, tag| {
            b.iter(|| {
                black_box(ascii_ci_tag_match!(
                    black_box(tag),
                    "area",
                    "base",
                    "br",
                    "col",
                    "embed",
                    "hr",
                    "img",
                    "input",
                    "link",
                    "meta",
                    "param",
                    "source",
                    "track",
                    "wbr",
                ))
            })
        });
    }

    // Mixed-case hits — uppercase fallback path
    for tag in MIXED_CASE_HITS {
        group.bench_with_input(BenchmarkId::new("mixed_case_hit", *tag), tag, |b, tag| {
            b.iter(|| {
                black_box(ascii_ci_tag_match!(
                    black_box(tag),
                    "area",
                    "base",
                    "br",
                    "col",
                    "embed",
                    "hr",
                    "img",
                    "input",
                    "link",
                    "meta",
                    "param",
                    "source",
                    "track",
                    "wbr",
                ))
            })
        });
    }

    // Long misses
    for tag in LONG_MISSES {
        group.bench_with_input(BenchmarkId::new("long_miss", *tag), tag, |b, tag| {
            b.iter(|| {
                black_box(ascii_ci_tag_match!(
                    black_box(tag),
                    "area",
                    "base",
                    "br",
                    "col",
                    "embed",
                    "hr",
                    "img",
                    "input",
                    "link",
                    "meta",
                    "param",
                    "source",
                    "track",
                    "wbr",
                ))
            })
        });
    }

    group.finish();
}

fn bench_closes_open_p(c: &mut Criterion) {
    let mut group = c.benchmark_group("tag_classification_closes_open_p");

    // Lowercase hits
    for tag in LOWERCASE_HITS {
        group.bench_with_input(BenchmarkId::new("lowercase", *tag), tag, |b, tag| {
            b.iter(|| {
                black_box(ascii_ci_tag_match!(
                    black_box(tag),
                    "address",
                    "article",
                    "aside",
                    "blockquote",
                    "div",
                    "dl",
                    "fieldset",
                    "footer",
                    "form",
                    "h1",
                    "h2",
                    "h3",
                    "h4",
                    "h5",
                    "h6",
                    "header",
                    "hr",
                    "main",
                    "nav",
                    "ol",
                    "p",
                    "pre",
                    "section",
                    "table",
                    "ul",
                ))
            })
        });
    }

    // Lowercase misses
    for tag in LOWERCASE_MISSES {
        group.bench_with_input(BenchmarkId::new("lowercase_miss", *tag), tag, |b, tag| {
            b.iter(|| {
                black_box(ascii_ci_tag_match!(
                    black_box(tag),
                    "address",
                    "article",
                    "aside",
                    "blockquote",
                    "div",
                    "dl",
                    "fieldset",
                    "footer",
                    "form",
                    "h1",
                    "h2",
                    "h3",
                    "h4",
                    "h5",
                    "h6",
                    "header",
                    "hr",
                    "main",
                    "nav",
                    "ol",
                    "p",
                    "pre",
                    "section",
                    "table",
                    "ul",
                ))
            })
        });
    }

    // Mixed-case hits
    for tag in MIXED_CASE_HITS {
        group.bench_with_input(BenchmarkId::new("mixed_case", *tag), tag, |b, tag| {
            b.iter(|| {
                black_box(ascii_ci_tag_match!(
                    black_box(tag),
                    "address",
                    "article",
                    "aside",
                    "blockquote",
                    "div",
                    "dl",
                    "fieldset",
                    "footer",
                    "form",
                    "h1",
                    "h2",
                    "h3",
                    "h4",
                    "h5",
                    "h6",
                    "header",
                    "hr",
                    "main",
                    "nav",
                    "ol",
                    "p",
                    "pre",
                    "section",
                    "table",
                    "ul",
                ))
            })
        });
    }

    group.finish();
}

fn bench_scope_barrier(c: &mut Criterion) {
    let mut group = c.benchmark_group("tag_classification_scope_barrier");

    // Small tag set — default scope barrier has only 6 entries
    for tag in LOWERCASE_HITS {
        group.bench_with_input(BenchmarkId::new("lowercase", *tag), tag, |b, tag| {
            b.iter(|| {
                black_box(ascii_ci_tag_match!(
                    black_box(tag),
                    "applet",
                    "marquee",
                    "object",
                    "table",
                    "td",
                    "th",
                ))
            })
        });
    }

    for tag in MIXED_CASE_HITS {
        group.bench_with_input(BenchmarkId::new("mixed_case", *tag), tag, |b, tag| {
            b.iter(|| {
                black_box(ascii_ci_tag_match!(
                    black_box(tag),
                    "applet",
                    "marquee",
                    "object",
                    "table",
                    "td",
                    "th",
                ))
            })
        });
    }

    group.finish();
}

/// Bulk throughput: classify many tags in a loop to measure aggregate cost.
fn bench_bulk_tag_classification(c: &mut Criterion) {
    let mut group = c.benchmark_group("tag_classification_bulk");

    let tags: Vec<&str> = LOWERCASE_HITS
        .iter()
        .chain(LOWERCASE_MISSES.iter())
        .chain(MIXED_CASE_HITS.iter())
        .chain(LONG_MISSES.iter())
        .copied()
        .cycle()
        .take(10_000)
        .collect();

    group.throughput(Throughput::Elements(tags.len() as u64));
    group.bench_function("void_check_10k", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(ascii_ci_tag_match!(
                    black_box(tag),
                    "area",
                    "base",
                    "br",
                    "col",
                    "embed",
                    "hr",
                    "img",
                    "input",
                    "link",
                    "meta",
                    "param",
                    "source",
                    "track",
                    "wbr",
                ));
            }
        })
    });

    group.throughput(Throughput::Elements(tags.len() as u64));
    group.bench_function("closes_open_p_10k", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(ascii_ci_tag_match!(
                    black_box(tag),
                    "address",
                    "article",
                    "aside",
                    "blockquote",
                    "div",
                    "dl",
                    "fieldset",
                    "footer",
                    "form",
                    "h1",
                    "h2",
                    "h3",
                    "h4",
                    "h5",
                    "h6",
                    "header",
                    "hr",
                    "main",
                    "nav",
                    "ol",
                    "p",
                    "pre",
                    "section",
                    "table",
                    "ul",
                ));
            }
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_void_elements,
    bench_closes_open_p,
    bench_scope_barrier,
    bench_bulk_tag_classification,
);
criterion_main!(benches);
