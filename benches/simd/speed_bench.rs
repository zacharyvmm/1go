//! SIMD-focused benchmarks for reader primitives and parser hot paths.
//!
//! These benchmarks intentionally exercise APIs that select AVX2 on x86_64
//! and NEON on aarch64/Apple Silicon, with scalar baselines beside them.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Reader, Save, StructuralIndex, parse, parse_tape, parse_fused, parse_fused_parallel, parse_fused_parallel_direct, parse_fused_parallel_adaptive};
use scah_reader::simd::{
    CpuFeatures, find_attribute_boundary_scalar, skip_whitespace_simd as skip_whitespace_simd_impl,
};
use std::hint::black_box;

const SIZES: [usize; 3] = [1_024, 16_384, 262_144];

fn scalar_skip_whitespace(input: &[u8], start: usize) -> usize {
    let mut pos = start;
    while pos < input.len() && matches!(input[pos], b' ' | b'\t' | b'\n' | b'\r') {
        pos += 1;
    }
    pos
}

fn index_structural_scalar(input: &[u8]) -> Vec<u32> {
    let mut indexes = Vec::with_capacity(input.len() / 16);
    for (pos, byte) in input.iter().copied().enumerate() {
        if matches!(byte, b'<' | b'>' | b'"' | b'\'') {
            indexes.push(pos as u32);
        }
    }
    indexes
}

fn generate_whitespace_run(len: usize) -> Vec<u8> {
    let mut input = Vec::with_capacity(len + 1);
    for i in 0..len {
        input.push(match i % 4 {
            0 => b' ',
            1 => b'\t',
            2 => b'\n',
            _ => b'\r',
        });
    }
    input.push(b'<');
    input
}

fn generate_long_attribute(boundary_after: usize) -> Vec<u8> {
    let mut input = Vec::with_capacity(boundary_after + 16);
    input.extend(std::iter::repeat_n(b'a', boundary_after));
    input.push(b'=');
    input.extend_from_slice(br#""value">"#);
    input
}

fn generate_attribute_html(element_count: usize) -> String {
    let mut html = String::with_capacity(element_count * 360);
    html.push_str(r#"<html><body><main id="app">"#);

    for i in 0..element_count {
        html.push_str(&format!(
            r#"<article class="card item-{i}" id="card-{i}" data-index="{i}" data-kind="product" data-visible="true" data-priority="normal" aria-label="Product card {i}">
<a href="/products/{i}" class="card-link" data-track-click="true" data-track-label="product-{i}">Product {i}</a>
<p class="description" data-copy="summary">Fast searchable content for product {i}</p>
</article>"#,
        ));
    }

    html.push_str("</main></body></html>");
    html
}

fn consume_store(store: &scah::Store<'_, '_>, selector: &str) {
    if let Some(elements) = store.get(selector) {
        for element in elements {
            black_box(element.name);
            black_box(element.attributes(store));
            black_box(element.inner_html);
            black_box(element.text_content(store));
        }
    }
}

fn bench_simd_whitespace_skip(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_whitespace_skip");
    let features = CpuFeatures::get();
    group.bench_function("cpu_features", |b| b.iter(|| black_box(features)));

    for len in SIZES {
        let input = generate_whitespace_run(len);
        group.throughput(Throughput::Bytes(input.len() as u64));

        group.bench_with_input(BenchmarkId::new("scalar_loop", len), &input, |b, input| {
            b.iter(|| black_box(scalar_skip_whitespace(black_box(input), 0)))
        });

        group.bench_with_input(
            BenchmarkId::new("simd_function", len),
            &input,
            |b, input| b.iter(|| black_box(skip_whitespace_simd_impl(black_box(input), 0))),
        );

        group.bench_with_input(
            BenchmarkId::new("reader_scalar", len),
            &input,
            |b, input| {
                b.iter(|| {
                    let mut reader = Reader::from_bytes(black_box(input));
                    reader.skip_whitespace();
                    black_box(reader.get_position())
                })
            },
        );

        group.bench_with_input(BenchmarkId::new("reader_simd", len), &input, |b, input| {
            b.iter(|| {
                let mut reader = Reader::from_bytes_with_simd(black_box(input));
                reader.skip_whitespace_simd();
                black_box(reader.get_position())
            })
        });
    }

    group.finish();
}

fn bench_simd_attribute_boundary(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_attribute_boundary");

    for len in SIZES {
        let input = generate_long_attribute(len);
        group.throughput(Throughput::Bytes(input.len() as u64));

        group.bench_with_input(
            BenchmarkId::new("scalar_function", len),
            &input,
            |b, input| b.iter(|| black_box(find_attribute_boundary_scalar(black_box(input), 0))),
        );

        group.bench_with_input(
            BenchmarkId::new("reader_scalar", len),
            &input,
            |b, input| {
                let reader = Reader::from_bytes(input);
                b.iter(|| black_box(reader.find_attribute_boundary_from(black_box(0))))
            },
        );

        group.bench_with_input(BenchmarkId::new("reader_simd", len), &input, |b, input| {
            let reader = Reader::from_bytes_with_simd(input);
            b.iter(|| black_box(reader.find_attribute_boundary_from(black_box(0))))
        });
    }

    group.finish();
}

fn bench_simd_structural_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_structural_index");

    for count in [128, 2_048, 16_384] {
        let html = generate_attribute_html(count);
        group.throughput(Throughput::Bytes(html.len() as u64));

        group.bench_with_input(BenchmarkId::new("scalar_local", count), &html, |b, html| {
            b.iter(|| black_box(index_structural_scalar(black_box(html.as_bytes()))))
        });

        group.bench_with_input(
            BenchmarkId::new("structural_index", count),
            &html,
            |b, html| b.iter(|| black_box(StructuralIndex::build(black_box(html.as_bytes())))),
        );
    }

    group.finish();
}

fn bench_simd_tape_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_tape_pipeline");
    let queries = vec![Query::all("article.card", Save::all()).unwrap().build()];

    for count in [128, 2_048, 16_384] {
        let html = generate_attribute_html(count);
        group.throughput(Throughput::Bytes(html.len() as u64));

        group.bench_with_input(
            BenchmarkId::new("classic_parse", count),
            &html,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(&queries));
                    consume_store(black_box(&store), "article.card");
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("structural_index", count),
            &html,
            |b, html| b.iter(|| black_box(StructuralIndex::build(black_box(html.as_bytes())))),
        );

        group.bench_with_input(BenchmarkId::new("tape_parse", count), &html, |b, html| {
            b.iter(|| {
                let store = parse_tape(black_box(html), black_box(&queries));
                consume_store(black_box(&store), "article.card");
            })
        });

        group.bench_with_input(BenchmarkId::new("fused_parse", count), &html, |b, html| {
            b.iter(|| {
                let store = parse_fused(black_box(html), black_box(&queries));
                consume_store(black_box(&store), "article.card");
            })
        });

        group.bench_with_input(BenchmarkId::new("fused_parallel_parse", count), &html, |b, html| {
            b.iter(|| {
                let store = parse_fused_parallel(black_box(html), black_box(&queries));
                consume_store(black_box(&store), "article.card");
            })
        });

        group.bench_with_input(BenchmarkId::new("fused_parallel_direct_parse", count), &html, |b, html| {
            b.iter(|| {
                let store = parse_fused_parallel_direct(black_box(html), black_box(&queries));
                consume_store(black_box(&store), "article.card");
            })
        });

        group.bench_with_input(BenchmarkId::new("fused_parallel_adaptive_parse", count), &html, |b, html| {
            b.iter(|| {
                let store = parse_fused_parallel_adaptive(black_box(html), black_box(&queries));
                consume_store(black_box(&store), "article.card");
            })
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_simd_whitespace_skip,
    bench_simd_attribute_boundary,
    bench_simd_structural_index,
    bench_simd_tape_pipeline,
);
criterion_main!(benches);
