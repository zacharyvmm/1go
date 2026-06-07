//! Benchmarks for attribute-heavy HTML parsing (forms, data attributes).
//!
//! This benchmark exercises the SIMD-accelerated attribute tokenizer by
//! using HTML with many attributes per element — the hot path that
//! Phase 3 optimizations target.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse};
use std::hint::black_box;

/// Generate a form-heavy HTML document with many attributes per element.
/// Each input element has type, id, name, class, placeholder, data-* attributes.
fn generate_form_html(input_count: usize) -> String {
    let mut html = String::with_capacity(input_count * 400);
    html.push_str(r#"<html><body><form action="/submit" method="post" class="main-form" id="contact-form" data-validate="true" novalidate>"#);

    for i in 0..input_count {
        html.push_str(&format!(
            r#"<div class="form-group" data-field-{i}="text" data-required="true" data-maxlength="255" data-pattern=".*">
<label for="field_{i}" class="control-label" data-toggle="tooltip" data-placement="top" title="Enter value for field {i}">Field {i}</label>
<input type="text" id="field_{i}" name="field_{i}" class="form-control" placeholder="Enter field {i}" data-index="{i}" data-type="text" autocomplete="off" spellcheck="false" tabindex="{i}" />
</div>"#,
            i = i
        ));
    }

    html.push_str(r#"<button type="submit" class="btn btn-primary" id="submit-btn" data-loading-text="Submitting..." disabled>Submit</button></form></body></html>"#);
    html
}

/// Generate HTML with data-* heavy div elements (common in JS frameworks).
fn generate_data_attr_html(element_count: usize) -> String {
    let mut html = String::with_capacity(element_count * 300);
    html.push_str(r#"<html><body><div id="app" data-framework="scah" data-version="0.17">"#);

    for i in 0..element_count {
        html.push_str(&format!(
            r#"<div class="component" data-component-id="{i}" data-component-type="card" data-state="active" data-index="{i}" data-parent="root" data-level="1" data-visible="true" data-priority="normal">
<span class="title" data-bind="title" data-i18n="card.title.{i}">Card {i}</span>
<a href="/cards/{i}" class="link" data-track-click="true" data-track-category="navigation" data-track-label="card_{i}" target="_blank" rel="noopener">View</a>
</div>"#,
            i = i
        ));
    }

    html.push_str("</div></body></html>");
    html
}

fn consume_results(store: &scah::Store<'_, '_>, query: &str) {
    if let Some(elements) = store.get(query) {
        for element in elements {
            black_box(&element.attributes(store));
            black_box(&element.name);
            black_box(&element.inner_html);
            black_box(&element.text_content(store));
        }
    }
}

fn bench_attribute_heavy(c: &mut Criterion) {
    let mut group = c.benchmark_group("attribute_heavy_forms");

    for size in [100, 500, 2_000].iter() {
        let content = generate_form_html(*size);
        group.throughput(Throughput::Bytes(content.len() as u64));

        // Benchmark: parse with save_none (tokenizer-only throughput)
        let none_queries = &[Query::all("input", Save::none()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("scah_form_save_none", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(none_queries));
                    black_box(store);
                })
            },
        );

        // Benchmark: parse with save_all (full extraction)
        let all_queries = &[Query::all("input", Save::all()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("scah_form_save_all", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(all_queries));
                    consume_results(black_box(&store), "input");
                })
            },
        );

        // Benchmark: query all form-groups with attribute access
        let group_queries = &[Query::all("div.form-group", Save::all()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("scah_form_groups_save_all", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(group_queries));
                    consume_results(black_box(&store), "div.form-group");
                })
            },
        );
    }

    group.finish();
}

fn bench_data_attributes(c: &mut Criterion) {
    let mut group = c.benchmark_group("attribute_heavy_data_attrs");

    for size in [100, 500, 2_000].iter() {
        let content = generate_data_attr_html(*size);
        group.throughput(Throughput::Bytes(content.len() as u64));

        // Benchmark: parse with save_none
        let none_queries = &[Query::all("a.link", Save::none()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("scah_data_attr_save_none", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(none_queries));
                    black_box(store);
                })
            },
        );

        // Benchmark: parse with save_all
        let all_queries = &[Query::all("a.link", Save::all()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("scah_data_attr_save_all", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(all_queries));
                    consume_results(black_box(&store), "a.link");
                })
            },
        );

        // Benchmark: query all component divs
        let comp_queries = &[Query::all("div.component", Save::all()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("scah_data_attr_components", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(comp_queries));
                    consume_results(black_box(&store), "div.component");
                })
            },
        );
    }

    group.finish();
}

fn bench_attribute_tokenizer_micro(c: &mut Criterion) {
    let mut group = c.benchmark_group("attribute_tokenizer_micro");

    // Micro-benchmark: just the tokenizer on a single element with many attributes
    let heavy_attrs = r#"div class="container" id="main" data-role="page" data-theme="light" data-transitions="fade" data-url="/home" data-title="Home Page" data-add-back-btn="true" data-back-btn-text="Back" data-dom-cache="true""#;
    group.throughput(Throughput::Bytes(heavy_attrs.len() as u64));

    let div_queries = vec![Query::all("div", Save::all()).unwrap().build()];

    group.bench_function("scah_heavy_attrs_element_parse", |b| {
        b.iter(|| {
            let html = format!("<{}>", heavy_attrs);
            let store = parse(black_box(&html), black_box(&div_queries));
            black_box(store);
        })
    });

    // Micro-benchmark: single element with no attributes
    group.bench_function("scah_no_attrs_element_parse", |b| {
        b.iter(|| {
            let store = parse(black_box("<div>text</div>"), black_box(&div_queries));
            black_box(store);
        })
    });

    // Micro-benchmark: single element with few attributes
    group.bench_function("scah_few_attrs_element_parse", |b| {
        b.iter(|| {
            let store = parse(
                black_box(r#"<div class="test" id="main">text</div>"#),
                black_box(&div_queries),
            );
            black_box(store);
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_attribute_heavy,
    bench_data_attributes,
    bench_attribute_tokenizer_micro,
);
criterion_main!(benches);
