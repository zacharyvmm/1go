//! Comparative microbenchmarks for tag-name classification strategies.
//!
//! These benchmarks measure classification helpers directly — not full
//! parser throughput — so that strategy choices are isolated and visible.
//!
//! # Strategies compared
//!
//! | Strategy | Description |
//! |----------|-------------|
//! | **baseline** | Pure `eq_ignore_ascii_case` linear scan (original PR #24 code) |
//! | **macro**    | `ascii_ci_tag_match!` — exact lowercase `match` + uppercase fallback |
//! | **len-bucket** | Length-bucketed: group candidates by byte length, scan only the matching bucket |
//!
//! Each strategy is tested against realistic tag distributions:
//! lowercase hits, lowercase misses, mixed-case hits, and long custom-element misses.
//!
//! # Results (run `cargo bench -p scah-benches -- speed_bench_tag_classification`)
//!
//! The "bulk_10k" groups compare aggregate throughput for a realistic mix of
//! 10 000 tag lookups against each tag set (void, closes_open_p, scope_barrier).

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

// ── tag sets from the production code ──────────────────────────────

fn void_tags() -> &'static [&'static str] {
    &[
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr",
    ]
}

fn closes_open_p_tags() -> &'static [&'static str] {
    &[
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
    ]
}

fn scope_barrier_tags() -> &'static [&'static str] {
    &["applet", "marquee", "object", "table", "td", "th"]
}

// ── strategy implementations ───────────────────────────────────────

/// Baseline: pure `eq_ignore_ascii_case` linear scan (original PR #24).
#[inline(never)]
fn baseline_scan(name: &str, candidates: &[&str]) -> bool {
    candidates.iter().any(|tag| name.eq_ignore_ascii_case(tag))
}

/// Length-bucketed: pre-group candidates by byte length, scan only the
/// matching bucket. Construction cost is paid once in the benchmark setup,
/// not inside the timed loop.
#[inline(never)]
fn len_bucketed_scan(name: &str, buckets: &[Vec<&str>]) -> bool {
    let len = name.len();
    if let Some(bucket) = buckets.get(len) {
        bucket.iter().any(|tag| name.eq_ignore_ascii_case(tag))
    } else {
        false
    }
}

fn build_len_buckets<'a>(candidates: &[&'a str]) -> Vec<Vec<&'a str>> {
    let max_len = candidates.iter().map(|t| t.len()).max().unwrap_or(0);
    let mut buckets: Vec<Vec<&str>> = (0..=max_len).map(|_| Vec::new()).collect();
    for &tag in candidates {
        buckets[tag.len()].push(tag);
    }
    buckets
}

// ── realistic distributions ────────────────────────────────────────

const LOWERCASE_HITS: &[&str] = &["div", "p", "a", "span", "li", "td", "input", "img"];
const LOWERCASE_MISSES: &[&str] = &["article-card", "custom-element", "svg", "path", "section"];
const MIXED_CASE_HITS: &[&str] = &["DIV", "Li", "TaBlE", "INPUT"];
const LONG_MISSES: &[&str] = &[
    "very-long-custom-element-name-that-will-never-match",
    "another-extremely-long-component-name-xyz",
    "my-application-shell-container-wrapper",
];

fn all_tags() -> Vec<&'static str> {
    LOWERCASE_HITS
        .iter()
        .chain(LOWERCASE_MISSES.iter())
        .chain(MIXED_CASE_HITS.iter())
        .chain(LONG_MISSES.iter())
        .copied()
        .collect()
}

// ── per-strategy bulk benchmarks ───────────────────────────────────

fn bench_bulk_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("tag_classification_bulk_10k");

    let tags: Vec<&str> = all_tags().into_iter().cycle().take(10_000).collect();
    group.throughput(Throughput::Elements(tags.len() as u64));

    // --- void element check ---
    let void_candidates = void_tags();
    group.bench_function("void/baseline_scan", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(baseline_scan(black_box(tag), black_box(void_candidates)));
            }
        })
    });
    group.bench_function("void/macro", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(scah::ascii_ci_tag_match!(
                    black_box(*tag),
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
    let void_buckets = build_len_buckets(void_candidates);
    group.bench_function("void/len_bucketed", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(len_bucketed_scan(black_box(tag), black_box(&void_buckets)));
            }
        })
    });

    // --- closes_open_p check ---
    let cop_candidates = closes_open_p_tags();
    group.bench_function("closes_open_p/baseline_scan", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(baseline_scan(black_box(tag), black_box(cop_candidates)));
            }
        })
    });
    group.bench_function("closes_open_p/macro", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(scah::ascii_ci_tag_match!(
                    black_box(*tag),
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
    let cop_buckets = build_len_buckets(cop_candidates);
    group.bench_function("closes_open_p/len_bucketed", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(len_bucketed_scan(black_box(tag), black_box(&cop_buckets)));
            }
        })
    });

    // --- scope barrier check ---
    let sb_candidates = scope_barrier_tags();
    group.bench_function("scope_barrier/baseline_scan", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(baseline_scan(black_box(tag), black_box(sb_candidates)));
            }
        })
    });
    group.bench_function("scope_barrier/macro", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(scah::ascii_ci_tag_match!(
                    black_box(*tag),
                    "applet",
                    "marquee",
                    "object",
                    "table",
                    "td",
                    "th",
                ));
            }
        })
    });
    let sb_buckets = build_len_buckets(sb_candidates);
    group.bench_function("scope_barrier/len_bucketed", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(len_bucketed_scan(black_box(tag), black_box(&sb_buckets)));
            }
        })
    });

    group.finish();
}

// ── per-tag microbenchmarks for the winning strategy ───────────────

fn bench_macro_micro(c: &mut Criterion) {
    let mut group = c.benchmark_group("tag_classification_micro");

    // Lowercase hits — fast path exercised
    for tag in LOWERCASE_HITS {
        group.bench_with_input(
            BenchmarkId::new("void/lowercase_hit", *tag),
            tag,
            |b, tag| {
                b.iter(|| {
                    black_box(scah::ascii_ci_tag_match!(
                        black_box(*tag),
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
            },
        );
    }

    // Lowercase misses — fast path, false result
    for tag in LOWERCASE_MISSES {
        group.bench_with_input(
            BenchmarkId::new("void/lowercase_miss", *tag),
            tag,
            |b, tag| {
                b.iter(|| {
                    black_box(scah::ascii_ci_tag_match!(
                        black_box(*tag),
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
            },
        );
    }

    // Mixed-case hits — uppercase fallback exercised
    for tag in MIXED_CASE_HITS {
        group.bench_with_input(
            BenchmarkId::new("void/mixed_case_hit", *tag),
            tag,
            |b, tag| {
                b.iter(|| {
                    black_box(scah::ascii_ci_tag_match!(
                        black_box(*tag),
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
            },
        );
    }

    // Long misses — worst-case for byte-by-byte comparison
    for tag in LONG_MISSES {
        group.bench_with_input(BenchmarkId::new("void/long_miss", *tag), tag, |b, tag| {
            b.iter(|| {
                black_box(scah::ascii_ci_tag_match!(
                    black_box(*tag),
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

criterion_group!(benches, bench_bulk_comparison, bench_macro_micro);
criterion_main!(benches);
