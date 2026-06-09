//! Benchmarks comparing fused single-pass tape parser vs streaming parser.
//!
//! This benchmark measures the throughput of the fused path against the
//! production streaming parser, which is now the recommended default.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse, parse_fused};
use std::hint::black_box;

/// Generate a form-heavy HTML document with many attributes per element.
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

/// Generate HTML with data-* heavy div elements.
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

fn bench_fused_vs_streaming_forms(c: &mut Criterion) {
    let mut group = c.benchmark_group("fused_vs_streaming_forms");

    for size in [100, 500, 2_000].iter() {
        let content = generate_form_html(*size);
        group.throughput(Throughput::Bytes(content.len() as u64));

        let none_queries = &[Query::all("input", Save::none()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("streaming_save_none", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(none_queries));
                    black_box(store);
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("fused_save_none", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse_fused(black_box(html), black_box(none_queries));
                    black_box(store);
                })
            },
        );

        let all_queries = &[Query::all("input", Save::all()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("streaming_save_all", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(all_queries));
                    consume_results(black_box(&store), "input");
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("fused_save_all", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse_fused(black_box(html), black_box(all_queries));
                    consume_results(black_box(&store), "input");
                })
            },
        );

        let group_queries = &[Query::all("div.form-group", Save::all()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("streaming_form_groups", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(group_queries));
                    consume_results(black_box(&store), "div.form-group");
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("fused_form_groups", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse_fused(black_box(html), black_box(group_queries));
                    consume_results(black_box(&store), "div.form-group");
                })
            },
        );
    }

    group.finish();
}

fn bench_fused_vs_streaming_data_attrs(c: &mut Criterion) {
    let mut group = c.benchmark_group("fused_vs_streaming_data_attrs");

    for size in [100, 500, 2_000].iter() {
        let content = generate_data_attr_html(*size);
        group.throughput(Throughput::Bytes(content.len() as u64));

        let link_queries = &[Query::all("a.link", Save::all()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("streaming_links", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(link_queries));
                    consume_results(black_box(&store), "a.link");
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("fused_links", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse_fused(black_box(html), black_box(link_queries));
                    consume_results(black_box(&store), "a.link");
                })
            },
        );

        let comp_queries = &[Query::all("div.component", Save::all()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("streaming_components", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(comp_queries));
                    consume_results(black_box(&store), "div.component");
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("fused_components", size),
            &content,
            |b, html| {
                b.iter(|| {
                    let store = parse_fused(black_box(html), black_box(comp_queries));
                    consume_results(black_box(&store), "div.component");
                })
            },
        );
    }

    group.finish();
}

fn bench_fused_vs_streaming_micro(c: &mut Criterion) {
    let mut group = c.benchmark_group("fused_vs_streaming_micro");

    let heavy_attrs = r#"div class="container" id="main" data-role="page" data-theme="light" data-transitions="fade" data-url="/home" data-title="Home Page" data-add-back-btn="true" data-back-btn-text="Back" data-dom-cache="true""#;
    group.throughput(Throughput::Bytes(heavy_attrs.len() as u64));

    let div_queries = vec![Query::all("div", Save::all()).unwrap().build()];

    group.bench_function("streaming_heavy_attrs", |b| {
        b.iter(|| {
            let html = format!("<{}>", heavy_attrs);
            let store = parse(black_box(&html), black_box(&div_queries));
            black_box(store);
        })
    });

    group.bench_function("fused_heavy_attrs", |b| {
        b.iter(|| {
            let html = format!("<{}>", heavy_attrs);
            let store = parse_fused(black_box(&html), black_box(&div_queries));
            black_box(store);
        })
    });

    group.bench_function("streaming_many_simple", |b| {
        b.iter(|| {
            let html = "<div class='a' id='b' data-x='1'><span class='c' id='d' data-y='2'>text</span></div>";
            let store = parse(black_box(html), black_box(&div_queries));
            black_box(store);
        })
    });

    group.bench_function("fused_many_simple", |b| {
        b.iter(|| {
            let html = "<div class='a' id='b' data-x='1'><span class='c' id='d' data-y='2'>text</span></div>";
            let store = parse_fused(black_box(html), black_box(&div_queries));
            black_box(store);
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_fused_vs_streaming_forms,
    bench_fused_vs_streaming_data_attrs,
    bench_fused_vs_streaming_micro,
);
criterion_main!(benches);
