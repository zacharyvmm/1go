//! Benchmarks comparing sequential fused vs parallel fused tape parser.
//!
//! This benchmark measures the throughput improvement from parallelizing
//! the fused tape construction across multiple cores using rayon.
//!
//! Target: 1.5-2.0x additional throughput on multi-core systems for documents >100KB.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse_fused, parse_fused_parallel, parse_fused_parallel_direct};
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

/// Generate a large document with mixed content types.
fn generate_mixed_html(target_size: usize) -> String {
    let mut html = String::with_capacity(target_size);
    html.push_str(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Large Document</title>
</head>
<body>
<header>
    <nav class="main-nav" data-theme="dark" data-responsive="true">
        <ul class="nav-list" role="menubar" aria-label="Main navigation">"#,
    );

    let mut current_size = html.len();
    let mut item_count = 0;

    while current_size < target_size {
        html.push_str(&format!(
            r#"
            <li class="nav-item" data-item-id="{item_count}" data-priority="normal" data-visible="true">
                <a href="/page/{item_count}" class="nav-link" data-track="true" data-track-category="navigation" target="_self">Page {item_count}</a>
            </li>"#,
            item_count = item_count
        ));
        current_size = html.len();
        item_count += 1;

        if item_count % 100 == 0 {
            html.push_str(
                r#"
        </ul>
    </nav>
</header>
<main class="content" role="main" data-section="body">
    <section class="article-list" data-layout="grid" data-columns="3">"#,
            );

            for j in 0..100 {
                html.push_str(&format!(
                    r#"
        <article class="article-card" data-article-id="{j}" data-category="tech" data-featured="false">
            <h2 class="article-title" data-i18n="article.title.{j}">Article {j}</h2>
            <div class="article-meta" data-author="author{j}" data-date="2024-01-{j:02}" data-read-time="{j}min">
                <span class="author" data-user-id="{j}">Author {j}</span>
                <time datetime="2024-01-{j:02}">January {j}, 2024</time>
            </div>
            <p class="article-excerpt" data-truncate="true">Lorem ipsum dolor sit amet, consectetur adipiscing elit.</p>
            <div class="article-tags" data-tag-count="3">
                <span class="tag" data-tag="html">HTML</span>
                <span class="tag" data-tag="css">CSS</span>
                <span class="tag" data-tag="js">JavaScript</span>
            </div>
        </article>"#,
                    j = j
                ));
            }

            html.push_str(
                r#"
    </section>
</main>"#,
            );
            current_size = html.len();
        }
    }

    html.push_str(
        r#"
</body>
</html>"#,
    );
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

fn bench_parallel_vs_sequential_forms(c: &mut Criterion) {
    let mut group = c.benchmark_group("parallel_vs_sequential_forms");

    // 100KB document (~250 form fields)
    let content_100k = generate_form_html(250);
    group.throughput(Throughput::Bytes(content_100k.len() as u64));

    let input_queries = &[Query::all("input", Save::all()).unwrap().build()];
    group.bench_with_input(
        BenchmarkId::new("fused_sequential", "100KB"),
        &content_100k,
        |b, html| {
            b.iter(|| {
                let store = parse_fused(black_box(html), black_box(input_queries));
                consume_results(black_box(&store), "input");
            })
        },
    );

    group.bench_with_input(
        BenchmarkId::new("fused_parallel", "100KB"),
        &content_100k,
        |b, html| {
            b.iter(|| {
                let store = parse_fused_parallel(black_box(html), black_box(input_queries));
                consume_results(black_box(&store), "input");
            })
        },
    );

    // 1MB document (~2500 form fields)
    let content_1m = generate_form_html(2500);
    group.throughput(Throughput::Bytes(content_1m.len() as u64));

    group.bench_with_input(
        BenchmarkId::new("fused_sequential", "1MB"),
        &content_1m,
        |b, html| {
            b.iter(|| {
                let store = parse_fused(black_box(html), black_box(input_queries));
                consume_results(black_box(&store), "input");
            })
        },
    );

    group.bench_with_input(
        BenchmarkId::new("fused_parallel", "1MB"),
        &content_1m,
        |b, html| {
            b.iter(|| {
                let store = parse_fused_parallel(black_box(html), black_box(input_queries));
                consume_results(black_box(&store), "input");
            })
        },
    );

    // 10MB document (~25000 form fields)
    let content_10m = generate_form_html(25000);
    group.throughput(Throughput::Bytes(content_10m.len() as u64));

    group.bench_with_input(
        BenchmarkId::new("fused_sequential", "10MB"),
        &content_10m,
        |b, html| {
            b.iter(|| {
                let store = parse_fused(black_box(html), black_box(input_queries));
                consume_results(black_box(&store), "input");
            })
        },
    );

    group.bench_with_input(
        BenchmarkId::new("fused_parallel", "10MB"),
        &content_10m,
        |b, html| {
            b.iter(|| {
                let store = parse_fused_parallel(black_box(html), black_box(input_queries));
                consume_results(black_box(&store), "input");
            })
        },
    );

    group.finish();
}

fn bench_parallel_vs_sequential_data_attrs(c: &mut Criterion) {
    let mut group = c.benchmark_group("parallel_vs_sequential_data_attrs");

    // 100KB document
    let content_100k = generate_data_attr_html(350);
    group.throughput(Throughput::Bytes(content_100k.len() as u64));

    let link_queries = &[Query::all("a.link", Save::all()).unwrap().build()];
    group.bench_with_input(
        BenchmarkId::new("fused_sequential", "100KB"),
        &content_100k,
        |b, html| {
            b.iter(|| {
                let store = parse_fused(black_box(html), black_box(link_queries));
                consume_results(black_box(&store), "a.link");
            })
        },
    );

    group.bench_with_input(
        BenchmarkId::new("fused_parallel", "100KB"),
        &content_100k,
        |b, html| {
            b.iter(|| {
                let store = parse_fused_parallel(black_box(html), black_box(link_queries));
                consume_results(black_box(&store), "a.link");
            })
        },
    );

    // 1MB document
    let content_1m = generate_data_attr_html(3500);
    group.throughput(Throughput::Bytes(content_1m.len() as u64));

    group.bench_with_input(
        BenchmarkId::new("fused_sequential", "1MB"),
        &content_1m,
        |b, html| {
            b.iter(|| {
                let store = parse_fused(black_box(html), black_box(link_queries));
                consume_results(black_box(&store), "a.link");
            })
        },
    );

    group.bench_with_input(
        BenchmarkId::new("fused_parallel", "1MB"),
        &content_1m,
        |b, html| {
            b.iter(|| {
                let store = parse_fused_parallel(black_box(html), black_box(link_queries));
                consume_results(black_box(&store), "a.link");
            })
        },
    );

    // 10MB document
    let content_10m = generate_data_attr_html(35000);
    group.throughput(Throughput::Bytes(content_10m.len() as u64));

    group.bench_with_input(
        BenchmarkId::new("fused_sequential", "10MB"),
        &content_10m,
        |b, html| {
            b.iter(|| {
                let store = parse_fused(black_box(html), black_box(link_queries));
                consume_results(black_box(&store), "a.link");
            })
        },
    );

    group.bench_with_input(
        BenchmarkId::new("fused_parallel", "10MB"),
        &content_10m,
        |b, html| {
            b.iter(|| {
                let store = parse_fused_parallel(black_box(html), black_box(link_queries));
                consume_results(black_box(&store), "a.link");
            })
        },
    );

    group.finish();
}

fn bench_parallel_vs_sequential_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("parallel_vs_sequential_mixed");

    let mixed_queries = &[Query::all("article.article-card", Save::all())
        .unwrap()
        .build()];

    // 100KB document
    let content_100k = generate_mixed_html(100 * 1024);
    group.throughput(Throughput::Bytes(content_100k.len() as u64));

    group.bench_with_input(
        BenchmarkId::new("fused_sequential", "100KB"),
        &content_100k,
        |b, html| {
            b.iter(|| {
                let store = parse_fused(black_box(html), black_box(mixed_queries));
                consume_results(black_box(&store), "article.article-card");
            })
        },
    );

    group.bench_with_input(
        BenchmarkId::new("fused_parallel", "100KB"),
        &content_100k,
        |b, html| {
            b.iter(|| {
                let store = parse_fused_parallel(black_box(html), black_box(mixed_queries));
                consume_results(black_box(&store), "article.article-card");
            })
        },
    );

    // 1MB document
    let content_1m = generate_mixed_html(1024 * 1024);
    group.throughput(Throughput::Bytes(content_1m.len() as u64));

    group.bench_with_input(
        BenchmarkId::new("fused_sequential", "1MB"),
        &content_1m,
        |b, html| {
            b.iter(|| {
                let store = parse_fused(black_box(html), black_box(mixed_queries));
                consume_results(black_box(&store), "article.article-card");
            })
        },
    );

    group.bench_with_input(
        BenchmarkId::new("fused_parallel", "1MB"),
        &content_1m,
        |b, html| {
            b.iter(|| {
                let store = parse_fused_parallel(black_box(html), black_box(mixed_queries));
                consume_results(black_box(&store), "article.article-card");
            })
        },
    );

    // 10MB document
    let content_10m = generate_mixed_html(10 * 1024 * 1024);
    group.throughput(Throughput::Bytes(content_10m.len() as u64));

    group.bench_with_input(
        BenchmarkId::new("fused_sequential", "10MB"),
        &content_10m,
        |b, html| {
            b.iter(|| {
                let store = parse_fused(black_box(html), black_box(mixed_queries));
                consume_results(black_box(&store), "article.article-card");
            })
        },
    );

    group.bench_with_input(
        BenchmarkId::new("fused_parallel", "10MB"),
        &content_10m,
        |b, html| {
            b.iter(|| {
                let store = parse_fused_parallel(black_box(html), black_box(mixed_queries));
                consume_results(black_box(&store), "article.article-card");
            })
        },
    );

    group.finish();
}

criterion_group!(
    benches,
    bench_parallel_vs_sequential_forms,
    bench_parallel_vs_sequential_data_attrs,
    bench_parallel_vs_sequential_mixed,
);
criterion_main!(benches);
