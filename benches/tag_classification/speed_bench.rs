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
//! | **len-bucket** | Length-bucketed: group candidates by byte length, scan only the matching bucket |
//! | **production** | `TagFlags::classify` followed by the parser's actual bitset query |
//!
//! Each strategy is tested against realistic tag distributions:
//! lowercase hits, lowercase misses, mixed-case hits, and long custom-element misses.
//!
//! # Results (run `cargo bench -p scah-benches -- speed_bench_tag_classification`)
//!
//! The "bulk_10k" groups compare aggregate throughput for a realistic mix of
//! 10 000 tag lookups against each tag set (void, closes_open_p, scope_barrier).

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use scah::bench_internals::{ScopeKind, TagFlags};
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
    group.bench_function("void/production", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(TagFlags::classify(black_box(*tag)).is_void());
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
    group.bench_function("closes_open_p/production", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(TagFlags::classify(black_box(*tag)).closes_open_p());
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
    group.bench_function("scope_barrier/production", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(TagFlags::classify(black_box(*tag)).is_scope_barrier(ScopeKind::Default));
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

/// Historical combined classifier (parser + text bits in one `TagFlags`-shaped
/// lookup) retained only for microbenchmark A/B against the split design.
#[inline]
fn classify_combined_old(name: &str) -> u32 {
    let flags = classify_combined_old_lowercase(name);
    if flags != 0 || !name.as_bytes().iter().any(u8::is_ascii_uppercase) {
        return flags;
    }
    if name.len() > 10 {
        return 0;
    }
    let mut lowercase = [0_u8; 10];
    for (output, input) in lowercase.iter_mut().zip(name.bytes()) {
        *output = input.to_ascii_lowercase();
    }
    let lowercase = unsafe { std::str::from_utf8_unchecked(&lowercase[..name.len()]) };
    classify_combined_old_lowercase(lowercase)
}

#[inline]
fn classify_combined_old_lowercase(name: &str) -> u32 {
    // Bit layout mirrors the pre-split TagFlags TEXT_* packing used on the PR
    // before parser/text separation (not the production TagFlags type).
    const VOID: u32 = 1 << 0;
    const CLOSES_P: u32 = 1 << 1;
    const P: u32 = 1 << 2;
    const BUTTON: u32 = 1 << 3;
    const LI: u32 = 1 << 4;
    const DT_DD: u32 = 1 << 5;
    const OPTION: u32 = 1 << 6;
    const OPTGROUP: u32 = 1 << 7;
    const TR: u32 = 1 << 8;
    const CELL: u32 = 1 << 9;
    const TABLE_SCOPE: u32 = 1 << 10;
    const DEFAULT_BARRIER: u32 = 1 << 11;
    const LIST_BARRIER: u32 = 1 << 12;
    const TABLE_BARRIER: u32 = 1 << 13;
    const HTML_TEMPLATE: u32 = 1 << 14;
    const RAW_SCRIPT: u32 = 1 << 15;
    const RAW_STYLE: u32 = 1 << 16;
    const RAW_TEXTAREA: u32 = 1 << 17;
    const RAW_TITLE: u32 = 1 << 18;
    const TEXT_BLOCK: u32 = 1 << 19;
    const TEXT_BREAK: u32 = 1 << 20;
    const TEXT_ROW: u32 = 1 << 21;
    const TEXT_CELL: u32 = 1 << 22;
    const TEXT_SUPPRESSED: u32 = 1 << 23;
    const TEXT_PREFORMATTED: u32 = 1 << 24;

    match name {
        "area" | "base" | "col" | "embed" | "img" | "input" | "link" | "meta" | "param"
        | "source" | "track" | "wbr" => VOID,
        "br" => VOID | TEXT_BREAK,
        "hr" => VOID | CLOSES_P | TEXT_BREAK,
        "address" | "article" | "aside" | "blockquote" | "div" | "dl" | "fieldset" | "footer"
        | "form" | "header" | "main" | "nav" | "section" => CLOSES_P | TEXT_BLOCK,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => CLOSES_P | TEXT_BLOCK,
        "p" => CLOSES_P | P | TEXT_BLOCK,
        "ol" | "ul" => CLOSES_P | LIST_BARRIER | TEXT_BLOCK,
        "table" => CLOSES_P | DEFAULT_BARRIER | TABLE_BARRIER | TEXT_BLOCK,
        "button" => BUTTON,
        "li" => LI | TEXT_BLOCK,
        "dt" | "dd" => DT_DD | TEXT_BLOCK,
        "option" => OPTION,
        "optgroup" => OPTGROUP,
        "tr" => TR | TABLE_SCOPE | TEXT_ROW | TEXT_BLOCK,
        "td" | "th" => CELL | DEFAULT_BARRIER | TEXT_CELL,
        "thead" | "tbody" | "tfoot" | "colgroup" => TABLE_SCOPE | TEXT_BLOCK,
        "caption" => TABLE_SCOPE | TEXT_BLOCK,
        "applet" | "marquee" | "object" => DEFAULT_BARRIER,
        "html" => HTML_TEMPLATE | TABLE_BARRIER,
        "template" => HTML_TEMPLATE | TABLE_BARRIER | TEXT_SUPPRESSED,
        "script" => RAW_SCRIPT | TEXT_SUPPRESSED,
        "style" => RAW_STYLE | TEXT_SUPPRESSED,
        "textarea" => RAW_TEXTAREA | TEXT_PREFORMATTED,
        "title" => RAW_TITLE,
        "pre" => CLOSES_P | TEXT_BLOCK | TEXT_PREFORMATTED,
        "body" | "details" | "dialog" | "figcaption" | "figure" | "hgroup" | "legend" | "menu"
        | "summary" => TEXT_BLOCK,
        _ => 0,
    }
}

/// Parser-only classifier matching `main` @ 30750d8 (embedded for A/B).
#[inline]
fn classify_parser_old(name: &str) -> TagFlags {
    // After the split, production `TagFlags::classify` *is* the main parser-only
    // table. Keep an explicit call site name for the microbenchmark legend.
    TagFlags::classify(name)
}

/// Prose-document tag mix from `prose_html()` open/close names.
const PROSE_TAG_MIX: &[&str] = &[
    "article",
    "p",
    "strong",
    "p",
    "strong",
    "p",
    "strong",
    "div",
    "span",
    "a",
    "br",
    "custom-element",
    "P",
    "DIV",
    "Br",
];

fn bench_parser_vs_text_classify(c: &mut Criterion) {
    let mut group = c.benchmark_group("tag_classification_parser_vs_text");
    group.sample_size(100);

    let tags: Vec<&str> = PROSE_TAG_MIX.iter().copied().cycle().take(10_000).collect();
    group.throughput(Throughput::Elements(tags.len() as u64));

    group.bench_function("parser_tag_classify_old", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(classify_parser_old(black_box(tag)));
            }
        })
    });
    group.bench_function("parser_tag_classify_new", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(TagFlags::classify(black_box(tag)));
            }
        })
    });
    group.bench_function("combined_old_single_flags", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(classify_combined_old(black_box(tag)));
            }
        })
    });
    group.bench_function("combined_text_tag_classify", |b| {
        b.iter(|| {
            for tag in &tags {
                black_box(scah::bench_internals::ClassifiedTag::classify(black_box(
                    tag,
                )));
            }
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_bulk_comparison,
    bench_parser_vs_text_classify
);
criterion_main!(benches);
