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
//! Each benchmark performs exactly one parse. Fixture construction,
//! query construction, validation, and setup-input destruction are excluded
//! from measurement. Parsed Store destruction remains included to match the
//! Criterion parse benchmark.
//!
//! The setup input is returned from the benchmark function for teardown so its
//! large fixture allocations are destroyed outside the measured region.
//! The parsed `Store` is intentionally dropped inside the measured function,
//! matching the Criterion parse benchmark's per-iteration drop behavior.
//!
//! # Runtime requirements
//!
//! Execution requires Linux, Valgrind, and `gungraun-runner`. Compilation only
//! requires the `linux-instruction-benches` feature.

#[cfg(target_os = "linux")]
mod support;

#[cfg(target_os = "linux")]
use gungraun::{library_benchmark, library_benchmark_group, main};
#[cfg(target_os = "linux")]
use scah::{Query, Save, parse};
#[cfg(target_os = "linux")]
use std::hint::black_box;
#[cfg(target_os = "linux")]
use support::fixtures::{generate_first_match_html, generate_link_list_html};
#[cfg(target_os = "linux")]
use support::validation::{
    assert_first_match_result, assert_save_all_result, assert_save_none_result,
};

// ── Sizes ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
const LINK_SIZE: usize = 1_000;
#[cfg(target_os = "linux")]
const FIRST_MATCH_COUNT: usize = 10_000;
#[cfg(target_os = "linux")]
const LINK_SELECTOR: &str = "a";
#[cfg(target_os = "linux")]
const FIRST_MATCH_SELECTOR: &str = "a.target";

// ── Input types ────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
struct ParseInput {
    html: String,
    queries: Vec<Query<'static>>,
}

// ── Setup functions (outside measured region) ──────────────────────────────

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
fn setup_first_match_early() -> ParseInput {
    let html = generate_first_match_html(FIRST_MATCH_COUNT, Some(0));

    let queries = vec![
        Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build(),
    ];

    let store = parse(&html, &queries).unwrap();
    assert_first_match_result(&store, FIRST_MATCH_SELECTOR, Some(0));

    ParseInput { html, queries }
}

#[cfg(target_os = "linux")]
fn setup_first_match_late() -> ParseInput {
    let html = generate_first_match_html(FIRST_MATCH_COUNT, Some(FIRST_MATCH_COUNT - 1));

    let queries = vec![
        Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build(),
    ];

    let store = parse(&html, &queries).unwrap();
    assert_first_match_result(&store, FIRST_MATCH_SELECTOR, Some(FIRST_MATCH_COUNT - 1));

    ParseInput { html, queries }
}

#[cfg(target_os = "linux")]
fn setup_first_match_no_match() -> ParseInput {
    let html = generate_first_match_html(FIRST_MATCH_COUNT, None);

    let queries = vec![
        Query::first(FIRST_MATCH_SELECTOR, Save::all())
            .expect("selector should parse")
            .build(),
    ];

    let store = parse(&html, &queries).unwrap();
    assert_first_match_result(&store, FIRST_MATCH_SELECTOR, None);

    ParseInput { html, queries }
}

// ── Teardown function ──────────────────────────────────────────────────────

/// Destroy the setup input outside the measured instruction-count region.
///
/// Gungraun calls this teardown function after measurement concludes, so the
/// large fixture allocations (HTML string, query vector) are deallocated
/// outside the profiled window.
#[cfg(target_os = "linux")]
fn drop_parse_input(input: ParseInput) {
    drop(input);
}

// ── Benchmark functions (one measured parse each) ──────────────────────────
//
// Each benchmark returns the setup input so the teardown function can destroy
// it outside the measured region. The parsed Store is intentionally dropped
// inside the measured function, matching the Criterion parse benchmark which
// drops the Store at the end of each `b.iter(|| ...)` iteration.

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::synthetic_links_save_none(
    args = (),
    setup = setup_synthetic_links_save_none,
    teardown = drop_parse_input,
)]
fn bench_synthetic_links_save_none(input: ParseInput) -> ParseInput {
    let store = parse(black_box(&input.html), black_box(&input.queries)).unwrap();
    black_box(store);
    input
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::synthetic_links_save_all(
    args = (),
    setup = setup_synthetic_links_save_all,
    teardown = drop_parse_input,
)]
fn bench_synthetic_links_save_all(input: ParseInput) -> ParseInput {
    let store = parse(black_box(&input.html), black_box(&input.queries)).unwrap();
    black_box(store);
    input
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::first_match_early(
    args = (),
    setup = setup_first_match_early,
    teardown = drop_parse_input,
)]
fn bench_first_match_early(input: ParseInput) -> ParseInput {
    let store = parse(black_box(&input.html), black_box(&input.queries)).unwrap();
    black_box(store);
    input
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::first_match_late(
    args = (),
    setup = setup_first_match_late,
    teardown = drop_parse_input,
)]
fn bench_first_match_late(input: ParseInput) -> ParseInput {
    let store = parse(black_box(&input.html), black_box(&input.queries)).unwrap();
    black_box(store);
    input
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::first_match_no_match(
    args = (),
    setup = setup_first_match_no_match,
    teardown = drop_parse_input,
)]
fn bench_first_match_no_match(input: ParseInput) -> ParseInput {
    let store = parse(black_box(&input.html), black_box(&input.queries)).unwrap();
    black_box(store);
    input
}

// ── Harness entry point ────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = instruction_group;
    benchmarks =
        bench_synthetic_links_save_none,
        bench_synthetic_links_save_all,
        bench_first_match_early,
        bench_first_match_late,
        bench_first_match_no_match
);

#[cfg(target_os = "linux")]
main!(library_benchmark_groups = instruction_group);

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("instruction-count benchmarks require Linux, Valgrind, and gungraun-runner");
}
