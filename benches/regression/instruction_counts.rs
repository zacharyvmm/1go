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
//!
//! # Measured boundaries
//!
//! Each benchmark measures exactly **one** parse. Fixture generation, query
//! construction, and correctness validation all happen inside Gungraun setup
//! functions and are excluded from instruction counts.
//!
//! # Runtime requirements
//!
//! Execution requires Linux, Valgrind, and `gungraun-runner`. Compilation only
//! requires the `linux-instruction-benches` feature.

mod support;

use gungraun::{library_benchmark, library_benchmark_group, main};
use scah::{Query, Save, parse};
use std::hint::black_box;
use support::fixtures::{generate_first_match_html, generate_link_list_html};
use support::validation::{
    assert_first_match_result, assert_save_all_result, assert_save_none_result,
};

// ── Sizes ──────────────────────────────────────────────────────────────────

const LINK_SIZE: usize = 1_000;
const FIRST_MATCH_COUNT: usize = 10_000;
const LINK_SELECTOR: &str = "a";
const FIRST_MATCH_SELECTOR: &str = "a.target";

// ── Input types ────────────────────────────────────────────────────────────

struct ParseInput {
    html: String,
    queries: Vec<Query<'static>>,
}

// ── Setup functions (outside measured region) ──────────────────────────────

fn setup_synthetic_links_save_none() -> ParseInput {
    let html = generate_link_list_html(LINK_SIZE);

    let queries = vec![
        Query::all(LINK_SELECTOR, Save::none())
            .expect("selector should parse")
            .build(),
    ];

    // Validate correctness outside the measured region
    let store = parse(&html, &queries).unwrap();
    assert_save_none_result(&store, LINK_SELECTOR, LINK_SIZE);

    ParseInput { html, queries }
}

fn setup_synthetic_links_save_all() -> ParseInput {
    let html = generate_link_list_html(LINK_SIZE);

    let queries = vec![
        Query::all(LINK_SELECTOR, Save::all())
            .expect("selector should parse")
            .build(),
    ];

    let store = parse(&html, &queries).unwrap();
    assert_save_all_result(&store, LINK_SELECTOR, LINK_SIZE);

    ParseInput { html, queries }
}

fn setup_first_match_early() -> ParseInput {
    let html = generate_first_match_html(FIRST_MATCH_COUNT, Some(0));

    let queries = vec![
        Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build(),
    ];

    let store = parse(&html, &queries).unwrap();
    assert_first_match_result(&store, FIRST_MATCH_SELECTOR, true, Some("Post 0"));

    ParseInput { html, queries }
}

fn setup_first_match_late() -> ParseInput {
    let html = generate_first_match_html(FIRST_MATCH_COUNT, Some(FIRST_MATCH_COUNT - 1));

    let queries = vec![
        Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build(),
    ];

    let store = parse(&html, &queries).unwrap();
    let expected_text = format!("Post {}", FIRST_MATCH_COUNT - 1);
    assert_first_match_result(&store, FIRST_MATCH_SELECTOR, true, Some(&expected_text));

    ParseInput { html, queries }
}

fn setup_first_match_no_match() -> ParseInput {
    let html = generate_first_match_html(FIRST_MATCH_COUNT, None);

    let queries = vec![
        Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build(),
    ];

    let store = parse(&html, &queries).unwrap();
    assert_first_match_result(&store, FIRST_MATCH_SELECTOR, false, None);

    ParseInput { html, queries }
}

// ── Benchmark functions (one measured parse each) ──────────────────────────

#[library_benchmark]
#[bench::synthetic_links_save_none(setup_synthetic_links_save_none())]
fn bench_synthetic_links_save_none(input: ParseInput) {
    let store = parse(black_box(&input.html), black_box(&input.queries)).unwrap();
    black_box(store);
}

#[library_benchmark]
#[bench::synthetic_links_save_all(setup_synthetic_links_save_all())]
fn bench_synthetic_links_save_all(input: ParseInput) {
    let store = parse(black_box(&input.html), black_box(&input.queries)).unwrap();
    black_box(store);
}

#[library_benchmark]
#[bench::first_match_early(setup_first_match_early())]
fn bench_first_match_early(input: ParseInput) {
    let store = parse(black_box(&input.html), black_box(&input.queries)).unwrap();
    black_box(store);
}

#[library_benchmark]
#[bench::first_match_late(setup_first_match_late())]
fn bench_first_match_late(input: ParseInput) {
    let store = parse(black_box(&input.html), black_box(&input.queries)).unwrap();
    black_box(store);
}

#[library_benchmark]
#[bench::first_match_no_match(setup_first_match_no_match())]
fn bench_first_match_no_match(input: ParseInput) {
    let store = parse(black_box(&input.html), black_box(&input.queries)).unwrap();
    black_box(store);
}

// ── Harness entry point ────────────────────────────────────────────────────

library_benchmark_group!(
    name = instruction_group;
    benchmarks =
        bench_synthetic_links_save_none,
        bench_synthetic_links_save_all,
        bench_first_match_early,
        bench_first_match_late,
        bench_first_match_no_match
);

main!(library_benchmark_groups = instruction_group);
