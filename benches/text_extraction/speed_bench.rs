//! Benchmarks for the dual text-extraction modes (`raw_text` / `text`).
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse, parse_without_text_capture};
use std::hint::black_box;

fn prose_html(paragraphs: usize) -> String {
    let mut html = String::from("<article>");
    for i in 0..paragraphs {
        html.push_str(&format!(
            "<p>Hello <strong>world</strong> number {i} with   spaced   words.</p>"
        ));
    }
    html.push_str("</article>");
    html
}

fn whitespace_html(blocks: usize) -> String {
    let mut html = String::from("<section>");
    for i in 0..blocks {
        html.push_str(&format!(
            "<div>\n  <p>Line {i}</p>\n  <p>More   text\nagain</p>\n</div>"
        ));
    }
    html.push_str("</section>");
    html
}

/// Ordinary prose with occasional character references (sparse entity workload).
fn sparse_entities_html(count: usize) -> String {
    let mut html = String::from("<div>");
    for i in 0..count {
        html.push_str(&format!(
            "<p>A&nbsp;&amp;&#x20;B &lt;{i}&gt; &quot;quote&quot;</p>"
        ));
    }
    html.push_str("</div>");
    html
}

/// Entity-dense input that stresses ampersand detection, named lookup,
/// numeric decoding, multi-code-point entities, and scratch-buffer reuse.
fn dense_entities_html(count: usize) -> String {
    let mut html = String::from("<div>");
    for _ in 0..count {
        html.push_str("<p>&amp;&nbsp;&copy;&#65;&#x41;&NotEqualTilde;&quot;&lt;&gt;</p>");
    }
    html.push_str("</div>");
    html
}

fn no_entity_html(count: usize) -> String {
    let mut html = String::from("<div>");
    for i in 0..count {
        html.push_str(&format!(
            "<p>Plain text paragraph number {i} without entities.</p>"
        ));
    }
    html.push_str("</div>");
    html
}

fn hidden_heavy_html(count: usize) -> String {
    let mut html = String::from("<section>");
    for i in 0..count {
        html.push_str(&format!(
            "<div>visible {i}<div hidden>secret {i}</div><script>x={i}</script></div>"
        ));
    }
    html.push_str("</section>");
    html
}

fn preformatted_heavy_html(count: usize) -> String {
    let mut html = String::from("<section>");
    for i in 0..count {
        html.push_str(&format!("<pre>\n  line {i}\n    indented\n</pre>"));
    }
    html.push_str("</section>");
    html
}

fn table_heavy_html(rows: usize) -> String {
    let mut html = String::from("<table>");
    for r in 0..rows {
        html.push_str("<tr>");
        for c in 0..4 {
            html.push_str(&format!("<td>R{r}C{c}</td>"));
        }
        html.push_str("</tr>");
    }
    html.push_str("</table>");
    html
}

fn nested_html(depth: usize) -> String {
    let mut html = String::new();
    for i in 0..depth {
        html.push_str(&format!("<div class=\"n{i}\">"));
    }
    html.push_str("leaf text");
    for _ in 0..depth {
        html.push_str("</div>");
    }
    html
}

/// Matched-only void input for `Save::none()` (attributes retained, no content).
/// Contains only elements selected by the void benchmark query (`input`), so
/// unmatched siblings do not dilute the measurement.
fn matched_void_html(count: usize) -> String {
    let mut html = String::from("<div>");
    for i in 0..count {
        html.push_str(&format!("<input id=\"i{i}\" type=\"text\">"));
    }
    html.push_str("</div>");
    html
}

fn consume_text(store: &scah::Store<'_, '_>, selector: &str) {
    if let Some(elements) = store.get(selector) {
        for element in elements {
            black_box(element.raw_text(store));
            black_box(element.text(store));
        }
    }
}

fn bench_text_modes(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_extraction_modes");
    group.sample_size(100);

    for size in [100usize, 1_000].iter().copied() {
        let prose = prose_html(size);
        group.throughput(Throughput::Bytes(prose.len() as u64));

        let none_q = &[Query::all("p", Save::none()).unwrap().build()];
        group.bench_with_input(BenchmarkId::new("no_content", size), &prose, |b, html| {
            b.iter(|| {
                let store = parse_without_text_capture(black_box(html), black_box(none_q)).unwrap();
                black_box(store);
            })
        });

        // Same prose document, selector that matches nothing — isolates scan-path
        // overhead from matched-result / SavedElement layout costs for Save::none().
        let none_no_match_q = &[Query::all("article > span", Save::none()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("no_content_no_matches", size),
            &prose,
            |b, html| {
                b.iter(|| {
                    let store =
                        parse_without_text_capture(black_box(html), black_box(none_no_match_q))
                            .unwrap();
                    black_box(store);
                })
            },
        );

        let raw_q = &[Query::all("p", Save::only_raw_text()).unwrap().build()];
        group.bench_with_input(BenchmarkId::new("raw_only", size), &prose, |b, html| {
            b.iter(|| {
                let store = parse(black_box(html), black_box(raw_q)).unwrap();
                consume_text(&store, "p");
                black_box(store);
            })
        });

        let text_q = &[Query::all("p", Save::only_text()).unwrap().build()];
        group.bench_with_input(
            BenchmarkId::new("text_only_prose", size),
            &prose,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(text_q)).unwrap();
                    consume_text(&store, "p");
                    black_box(store);
                })
            },
        );

        let both_q = &[Query::all("p", Save::all()).unwrap().build()];
        group.bench_with_input(BenchmarkId::new("both", size), &prose, |b, html| {
            b.iter(|| {
                let store = parse(black_box(html), black_box(both_q)).unwrap();
                consume_text(&store, "p");
                black_box(store);
            })
        });

        let ws = whitespace_html(size);
        group.throughput(Throughput::Bytes(ws.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("text_only_whitespace", size),
            &ws,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(text_q)).unwrap();
                    consume_text(&store, "p");
                    black_box(store);
                })
            },
        );

        let sparse_entities = sparse_entities_html(size);
        group.throughput(Throughput::Bytes(sparse_entities.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("text_only_sparse_entities", size),
            &sparse_entities,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(text_q)).unwrap();
                    consume_text(&store, "p");
                    black_box(store);
                })
            },
        );

        let no_entities = no_entity_html(size);
        group.throughput(Throughput::Bytes(no_entities.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("text_only_no_entities", size),
            &no_entities,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(text_q)).unwrap();
                    consume_text(&store, "p");
                    black_box(store);
                })
            },
        );

        let dense_entities = dense_entities_html(size);
        group.throughput(Throughput::Bytes(dense_entities.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("text_only_dense_entities", size),
            &dense_entities,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(text_q)).unwrap();
                    consume_text(&store, "p");
                    black_box(store);
                })
            },
        );

        let hidden = hidden_heavy_html(size);
        group.throughput(Throughput::Bytes(hidden.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("hidden_heavy", size),
            &hidden,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(text_q)).unwrap();
                    consume_text(&store, "div");
                    black_box(store);
                })
            },
        );

        let pre = preformatted_heavy_html(size);
        let pre_q = &[Query::all("pre", Save::only_text()).unwrap().build()];
        group.throughput(Throughput::Bytes(pre.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("preformatted_heavy", size),
            &pre,
            |b, html| {
                b.iter(|| {
                    let store = parse(black_box(html), black_box(pre_q)).unwrap();
                    consume_text(&store, "pre");
                    black_box(store);
                })
            },
        );

        let table = table_heavy_html(size);
        let table_q = &[Query::all("table", Save::only_text()).unwrap().build()];
        group.throughput(Throughput::Bytes(table.len() as u64));
        group.bench_with_input(BenchmarkId::new("table_heavy", size), &table, |b, html| {
            b.iter(|| {
                let store = parse(black_box(html), black_box(table_q)).unwrap();
                consume_text(&store, "table");
                black_box(store);
            })
        });

        let inner_q = &[Query::all("p", Save::only_inner_html()).unwrap().build()];
        group.throughput(Throughput::Bytes(prose.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("inner_html_only", size),
            &prose,
            |b, html| {
                b.iter(|| {
                    let store =
                        parse_without_text_capture(black_box(html), black_box(inner_q)).unwrap();
                    black_box(store);
                })
            },
        );

        // Same prose document, but a selector that matches nothing — isolates
        // parser/store overhead from matched-result / SavedElement layout costs.
        let no_match_q = &[Query::all("article > span", Save::only_inner_html())
            .unwrap()
            .build()];
        group.bench_with_input(
            BenchmarkId::new("inner_html_no_matches", size),
            &prose,
            |b, html| {
                b.iter(|| {
                    let store =
                        parse_without_text_capture(black_box(html), black_box(no_match_q)).unwrap();
                    black_box(store);
                })
            },
        );

        let voids = matched_void_html(size);
        let void_q = &[Query::all("input", Save::none()).unwrap().build()];
        group.throughput(Throughput::Bytes(voids.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("matched_void_no_content", size),
            &voids,
            |b, html| {
                b.iter(|| {
                    let store =
                        parse_without_text_capture(black_box(html), black_box(void_q)).unwrap();
                    black_box(store);
                })
            },
        );
    }

    let nested = nested_html(32);
    let nested_q = &[Query::all("div", Save::all()).unwrap().build()];
    group.bench_function("overlapping_nested", |b| {
        b.iter(|| {
            let store = parse(black_box(&nested), black_box(nested_q)).unwrap();
            consume_text(&store, "div");
            black_box(store);
        })
    });

    let first_html = format!(
        "<div id=\"hit\">important text</div>{}",
        "<span>filler</span>".repeat(5_000)
    );
    let first_q = &[Query::first("#hit", Save::only_text()).unwrap().build()];
    group.bench_function("first_with_text", |b| {
        b.iter(|| {
            let store = parse(black_box(&first_html), black_box(first_q)).unwrap();
            consume_text(&store, "#hit");
            black_box(store);
        })
    });

    group.finish();

    // ── Diagnostic controls for no-text scan overhead ──────────────
    {
        let mut diag = c.benchmark_group("text_extraction_scan_diagnostics");
        diag.sample_size(100);

        // Startup / tiny-document fixed costs.
        let empty = "";
        let one_tag = "<p>x</p>";
        let ten_tags = "<div><p>1</p><p>2</p><p>3</p><p>4</p><p>5</p><p>6</p><p>7</p><p>8</p><p>9</p><p>10</p></div>";
        let no_match_q = &[Query::all("article > span", Save::none()).unwrap().build()];

        diag.bench_function("parse_empty_document", |b| {
            b.iter(|| {
                let store =
                    parse_without_text_capture(black_box(empty), black_box(no_match_q)).unwrap();
                black_box(store);
            })
        });
        diag.bench_function("parse_one_tag_no_match", |b| {
            b.iter(|| {
                let store =
                    parse_without_text_capture(black_box(one_tag), black_box(no_match_q)).unwrap();
                black_box(store);
            })
        });
        diag.bench_function("parse_ten_tags_no_match", |b| {
            b.iter(|| {
                let store =
                    parse_without_text_capture(black_box(ten_tags), black_box(no_match_q)).unwrap();
                black_box(store);
            })
        });

        // Known HTML tags vs custom tags (same structure, no matches).
        for size in [100usize, 1_000, 10_000].iter().copied() {
            let known = {
                let mut html = String::from("<section>");
                for i in 0..size {
                    html.push_str(&format!("<p>item {i}</p>"));
                }
                html.push_str("</section>");
                html
            };
            let custom = {
                let mut html = String::from("<section>");
                for i in 0..size {
                    html.push_str(&format!("<x-item>item {i}</x-item>"));
                }
                html.push_str("</section>");
                html
            };
            let known_q = &[Query::all("article > span", Save::none()).unwrap().build()];
            let custom_q = &[Query::all("article > span", Save::none()).unwrap().build()];

            diag.throughput(Throughput::Bytes(known.len() as u64));
            diag.bench_with_input(
                BenchmarkId::new("known_tags_no_match", size),
                &known,
                |b, html| {
                    b.iter(|| {
                        let store = parse_without_text_capture(black_box(html), black_box(known_q))
                            .unwrap();
                        black_box(store);
                    })
                },
            );
            diag.throughput(Throughput::Bytes(custom.len() as u64));
            diag.bench_with_input(
                BenchmarkId::new("custom_tags_no_match", size),
                &custom,
                |b, html| {
                    b.iter(|| {
                        let store =
                            parse_without_text_capture(black_box(html), black_box(custom_q))
                                .unwrap();
                        black_box(store);
                    })
                },
            );
        }

        diag.finish();
    }
}

criterion_group!(benches, bench_text_modes);
criterion_main!(benches);
