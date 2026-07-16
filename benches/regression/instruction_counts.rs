//! Instruction-count benchmarks using Gungraun (Cachegrind).
//!
//! These benchmarks measure deterministic CPU-level metrics (instruction count,
//! estimated cycles, cache behavior) rather than wall-clock time. They are
//! Linux-only and require the `linux-instruction-benches` feature.
//!
//! # Distinction from Criterion benchmarks
//!
//! - **Criterion** (`core.rs`): measures real elapsed time (wall clock).
//! - **Gungraun** (this file): measures simulated/deterministic operation counts.
//!
//! Instruction improvements do not always imply elapsed-time improvements.
//! Both tools are useful and complementary: Gungraun provides a lower-noise
//! signal on shared/CI runners, while Criterion captures real-world latency.

mod support;

use gungraun::prelude::*;
use scah::{Query, Save, parse};
use support::fixtures::{generate_first_match_html, generate_link_list_html};

// ── Sizes ──────────────────────────────────────────────────────────────────

const LINK_SIZE: usize = 1_000;
const FIRST_MATCH_COUNT: usize = 10_000;
const LINK_SELECTOR: &str = "a";
const FIRST_MATCH_SELECTOR: &str = "a.target";

// ── Helper ─────────────────────────────────────────────────────────────────

fn parse_bench(html: &str, queries: &[scah::Query]) {
    let store = parse(html, queries).unwrap();
    gungraun::black_box(store);
}

// ── Benchmark functions ────────────────────────────────────────────────────

fn bench_synthetic_links_save_none() {
    let html = generate_link_list_html(LINK_SIZE);
    let queries = &[Query::all(LINK_SELECTOR, Save::none())
        .expect("selector should parse")
        .build()];
    // Validate once
    let store = parse(&html, queries).unwrap();
    assert_eq!(
        store.get(LINK_SELECTOR).map(|e| e.count()).unwrap_or(0),
        LINK_SIZE
    );
    parse_bench(&html, queries);
}

fn bench_synthetic_links_save_all() {
    let html = generate_link_list_html(LINK_SIZE);
    let queries = &[Query::all(LINK_SELECTOR, Save::all())
        .expect("selector should parse")
        .build()];
    let store = parse(&html, queries).unwrap();
    assert_eq!(
        store.get(LINK_SELECTOR).map(|e| e.count()).unwrap_or(0),
        LINK_SIZE
    );
    parse_bench(&html, queries);
}

fn bench_first_match_early() {
    let html = generate_first_match_html(FIRST_MATCH_COUNT, Some(0));
    let queries = &[Query::first(FIRST_MATCH_SELECTOR, Save::all())
        .expect("selector should parse")
        .build()];
    let store = parse(&html, queries).unwrap();
    assert!(store.get(FIRST_MATCH_SELECTOR).is_some());
    parse_bench(&html, queries);
}

fn bench_first_match_late() {
    let html = generate_first_match_html(FIRST_MATCH_COUNT, Some(FIRST_MATCH_COUNT - 1));
    let queries = &[Query::first(FIRST_MATCH_SELECTOR, Save::all())
        .expect("selector should parse")
        .build()];
    let store = parse(&html, queries).unwrap();
    assert!(store.get(FIRST_MATCH_SELECTOR).is_some());
    parse_bench(&html, queries);
}

fn bench_first_match_no_match() {
    let html = generate_first_match_html(FIRST_MATCH_COUNT, None);
    let queries = &[Query::first(FIRST_MATCH_SELECTOR, Save::all())
        .expect("selector should parse")
        .build()];
    let store = parse(&html, queries).unwrap();
    assert!(store.get(FIRST_MATCH_SELECTOR).is_none());
    parse_bench(&html, queries);
}

// ── Harness entry point ────────────────────────────────────────────────────

#[gungraun::main]
fn main() {
    benchmark!(
        "parse/synthetic_links/prebuilt/all/save_none/1000",
        bench_synthetic_links_save_none
    );
    benchmark!(
        "parse/synthetic_links/prebuilt/all/save_all/1000",
        bench_synthetic_links_save_all
    );
    benchmark!("parse/first_match/early/10000", bench_first_match_early);
    benchmark!("parse/first_match/late/10000", bench_first_match_late);
    benchmark!(
        "parse/first_match/no_match/10000",
        bench_first_match_no_match
    );
}
