//! Benchmarks comparing all scah parsing modes on real-world WHATWG spec HTML.
//!
//! Uses 15MB of real HTML to give each mode a fair shake:
//! - `parse` (streaming): the production default
//! - `parse_tape` (3-stage tape): SIMD scan + tape build + re-tokenization
//! - `parse_fused` (fused tape): SIMD scan with pre-tokenized attributes
//! - `parse_fused_parallel`: parallel fused (multi-chunk)
//!
//! Queries are chosen to exercise different parser aspects and avoid the
//! simple_tag_parser fast path where appropriate.

#[path = "../support/mod.rs"]
mod support;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use scah::{
    Query, Save,
    parse, parse_fused, parse_fused_parallel,
    parse_fused_parallel_direct, parse_fused_parallel_adaptive,
    parse_tape,
};
use std::hint::black_box;

const SPEC_HTML_FILE: &str = "html.spec.whatwg.org.html";

// ---------------------------------------------------------------------------
// Query 1: "a[href]" — tag + attribute selector.
// This does NOT hit simple_tag_parser (attribute filter present), so all
// modes run their full pipeline.  Tests attribute parsing throughput.
// ---------------------------------------------------------------------------
fn bench_links_with_href(c: &mut Criterion) {
    let mut group = c.benchmark_group("whatwg_spec_all_links_href");
    let content = support::load_bench_data(SPEC_HTML_FILE);
    group.throughput(Throughput::Bytes(content.len() as u64));

    let queries_save_none = &[Query::all("a[href]", Save::none())
        .expect("selector should parse")
        .build()];

    group.bench_function("parse_streaming_save_none", |b| {
        b.iter(|| {
            let store = parse(black_box(&content), black_box(queries_save_none));
            black_box(store);
        })
    });

    group.bench_function("parse_tape_3stage_save_none", |b| {
        b.iter(|| {
            let store = parse_tape(black_box(&content), black_box(queries_save_none));
            black_box(store);
        })
    });

    group.bench_function("parse_fused_save_none", |b| {
        b.iter(|| {
            let store = parse_fused(black_box(&content), black_box(queries_save_none));
            black_box(store);
        })
    });

    group.bench_function("parse_fused_parallel_save_none", |b| {
        b.iter(|| {
            let store = parse_fused_parallel(black_box(&content), black_box(queries_save_none));
            black_box(store);
        })
    });

    let queries_save_all = &[Query::all("a[href]", Save::all())
        .expect("selector should parse")
        .build()];

    group.bench_function("parse_streaming_save_all", |b| {
        b.iter(|| {
            let store = parse(black_box(&content), black_box(queries_save_all));
            // Touch results to prevent dead code elimination
            if let Some(elems) = store.get("a[href]") {
                for el in elems {
                    black_box(el.attributes(&store));
                    black_box(el.inner_html);
                    black_box(el.text_content(&store));
                }
            }
        })
    });

    group.bench_function("parse_tape_3stage_save_all", |b| {
        b.iter(|| {
            let store = parse_tape(black_box(&content), black_box(queries_save_all));
            if let Some(elems) = store.get("a[href]") {
                for el in elems {
                    black_box(el.attributes(&store));
                    black_box(el.inner_html);
                    black_box(el.text_content(&store));
                }
            }
        })
    });

    group.bench_function("parse_fused_save_all", |b| {
        b.iter(|| {
            let store = parse_fused(black_box(&content), black_box(queries_save_all));
            if let Some(elems) = store.get("a[href]") {
                for el in elems {
                    black_box(el.attributes(&store));
                    black_box(el.inner_html);
                    black_box(el.text_content(&store));
                }
            }
        })
    });

    group.bench_function("parse_fused_parallel_save_all", |b| {
        b.iter(|| {
            let store = parse_fused_parallel(black_box(&content), black_box(queries_save_all));
            if let Some(elems) = store.get("a[href]") {
                for el in elems {
                    black_box(el.attributes(&store));
                    black_box(el.inner_html);
                    black_box(el.text_content(&store));
                }
            }
        })
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Query 2: "code" — bare tag name.
// This DOES hit simple_tag_parser for parse_tape, showing the fast-path
// advantage (and the unfairness of the existing spec bench).
// ---------------------------------------------------------------------------
fn bench_code_elements(c: &mut Criterion) {
    let mut group = c.benchmark_group("whatwg_spec_all_code");
    let content = support::load_bench_data(SPEC_HTML_FILE);
    group.throughput(Throughput::Bytes(content.len() as u64));

    let queries = &[Query::all("code", Save::all())
        .expect("selector should parse")
        .build()];

    group.bench_function("parse_streaming", |b| {
        b.iter(|| {
            let store = parse(black_box(&content), black_box(queries));
            if let Some(elems) = store.get("code") {
                for el in elems {
                    black_box(el.attributes(&store));
                    black_box(el.inner_html);
                    black_box(el.text_content(&store));
                }
            }
        })
    });

    group.bench_function("parse_tape_3stage", |b| {
        b.iter(|| {
            let store = parse_tape(black_box(&content), black_box(queries));
            if let Some(elems) = store.get("code") {
                for el in elems {
                    black_box(el.attributes(&store));
                    black_box(el.inner_html);
                    black_box(el.text_content(&store));
                }
            }
        })
    });

    group.bench_function("parse_fused", |b| {
        b.iter(|| {
            let store = parse_fused(black_box(&content), black_box(queries));
            if let Some(elems) = store.get("code") {
                for el in elems {
                    black_box(el.attributes(&store));
                    black_box(el.inner_html);
                    black_box(el.text_content(&store));
                }
            }
        })
    });

    group.bench_function("parse_fused_parallel", |b| {
        b.iter(|| {
            let store = parse_fused_parallel(black_box(&content), black_box(queries));
            if let Some(elems) = store.get("code") {
                for el in elems {
                    black_box(el.attributes(&store));
                    black_box(el.inner_html);
                    black_box(el.text_content(&store));
                }
            }
        })
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Query 3: "ol > li" — child combinator with class-less tags.
// The child combinator prevents simple_tag_parser from matching.
// Tests the QueryMultiplexer's internal state machine under load.
// ---------------------------------------------------------------------------
fn bench_ordered_list_items(c: &mut Criterion) {
    let mut group = c.benchmark_group("whatwg_spec_ol_li");
    let content = support::load_bench_data(SPEC_HTML_FILE);
    group.throughput(Throughput::Bytes(content.len() as u64));

    let queries = &[Query::all("ol > li", Save::all())
        .expect("selector should parse")
        .build()];

    group.bench_function("parse_streaming", |b| {
        b.iter(|| {
            let store = parse(black_box(&content), black_box(queries));
            if let Some(elems) = store.get("ol > li") {
                for el in elems {
                    black_box(el.inner_html);
                    black_box(el.text_content(&store));
                }
            }
        })
    });

    group.bench_function("parse_tape_3stage", |b| {
        b.iter(|| {
            let store = parse_tape(black_box(&content), black_box(queries));
            if let Some(elems) = store.get("ol > li") {
                for el in elems {
                    black_box(el.inner_html);
                    black_box(el.text_content(&store));
                }
            }
        })
    });

    group.bench_function("parse_fused", |b| {
        b.iter(|| {
            let store = parse_fused(black_box(&content), black_box(queries));
            if let Some(elems) = store.get("ol > li") {
                for el in elems {
                    black_box(el.inner_html);
                    black_box(el.text_content(&store));
                }
            }
        })
    });

    group.bench_function("parse_fused_parallel", |b| {
        b.iter(|| {
            let store = parse_fused_parallel(black_box(&content), black_box(queries));
            if let Some(elems) = store.get("ol > li") {
                for el in elems {
                    black_box(el.inner_html);
                    black_box(el.text_content(&store));
                }
            }
        })
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Query 4: large doc — full parse + fused_parallel_direct + adaptive
// Only for the largest query to keep benchmark time reasonable.
// ---------------------------------------------------------------------------
fn bench_large_parallel(c: &mut Criterion) {
    let mut group = c.benchmark_group("whatwg_spec_parallel_modes");
    let content = support::load_bench_data(SPEC_HTML_FILE);
    group.throughput(Throughput::Bytes(content.len() as u64));

    let queries = &[Query::all("a[href]", Save::none())
        .expect("selector should parse")
        .build()];

    group.bench_function("parse_streaming_baseline", |b| {
        b.iter(|| {
            let store = parse(black_box(&content), black_box(queries));
            black_box(store);
        })
    });

    group.bench_function("parse_fused", |b| {
        b.iter(|| {
            let store = parse_fused(black_box(&content), black_box(queries));
            black_box(store);
        })
    });

    group.bench_function("parse_fused_parallel", |b| {
        b.iter(|| {
            let store = parse_fused_parallel(black_box(&content), black_box(queries));
            black_box(store);
        })
    });

    group.bench_function("parse_fused_parallel_direct", |b| {
        b.iter(|| {
            let store = parse_fused_parallel_direct(black_box(&content), black_box(queries));
            black_box(store);
        })
    });

    group.bench_function("parse_fused_parallel_adaptive", |b| {
        b.iter(|| {
            let store = parse_fused_parallel_adaptive(black_box(&content), black_box(queries));
            black_box(store);
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_links_with_href,
    bench_code_elements,
    bench_ordered_list_items,
    bench_large_parallel,
);
criterion_main!(benches);
