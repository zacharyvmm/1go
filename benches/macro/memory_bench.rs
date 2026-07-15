use gungraun::{library_benchmark, library_benchmark_group, main};
use scah::{Query, Save, parse, query};
use std::hint::black_box;

const MAX_ELEMENT_LEN: usize = 1000;

fn setup_html() -> String {
    let mut html = String::with_capacity(MAX_ELEMENT_LEN * 160);
    html.push_str("<html><body><main>");
    for i in 0..MAX_ELEMENT_LEN {
        html.push_str(&format!(
            r#"<article class="post"><h1>Title {i}</h1><section><a href="/post/{i}">Post {i}</a><span>Summary {i}</span></section></article>"#
        ));
    }
    html.push_str("</main></body></html>");
    html
}

#[library_benchmark]
#[bench::runtime_all(setup_html())]
fn bench_runtime_all(html: String) {
    let query = Query::all("main > article.post", Save::none())
        .expect("macro benchmark root selector should parse")
        .then(|article| {
            Ok([
                article.first("h1", Save::only_text_content())?,
                article.all("> section > a[href]", Save::all())?,
                article.first("> section > span", Save::only_text_content())?,
            ])
        })
        .expect("macro benchmark child selectors should parse")
        .build();

    let queries = [query];
    let store = parse(&html, &queries).unwrap();
    for article in store.get("main > article.post").unwrap() {
        black_box(article.get(&store, "h1").unwrap().count());
        black_box(article.get(&store, "> section > a[href]").unwrap().count());
        black_box(article.get(&store, "> section > span").unwrap().count());
    }
}

#[library_benchmark]
#[bench::macro_all(setup_html())]
fn bench_macro_all(html: String) {
    let query = query! {
        all("main > article.post", Save::none()) => {
            first("h1", Save::only_text_content()),
            all("> section > a[href]", Save::all()),
            first("> section > span", Save::only_text_content()),
        }
    };

    let queries = [query];
    let store = parse(&html, &queries).unwrap();
    for article in store.get("main > article.post").unwrap() {
        black_box(article.get(&store, "h1").unwrap().count());
        black_box(article.get(&store, "> section > a[href]").unwrap().count());
        black_box(article.get(&store, "> section > span").unwrap().count());
    }
}

#[library_benchmark]
#[bench::runtime_first(setup_html())]
fn bench_runtime_first(html: String) {
    let query = Query::first("main > article.post", Save::none())
        .expect("macro benchmark root selector should parse")
        .then(|article| {
            Ok([
                article.first("h1", Save::only_text_content())?,
                article.first("> section > a[href]", Save::all())?,
            ])
        })
        .expect("macro benchmark child selectors should parse")
        .build();

    let queries = [query];
    let store = parse(&html, &queries).unwrap();
    for article in store.get("main > article.post").unwrap() {
        black_box(article.get(&store, "h1").unwrap().count());
        black_box(article.get(&store, "> section > a[href]").unwrap().count());
    }
}

#[library_benchmark]
#[bench::macro_first(setup_html())]
fn bench_macro_first(html: String) {
    let query = query! {
        first("main > article.post", Save::none()) => {
            first("h1", Save::only_text_content()),
            first("> section > a[href]", Save::all()),
        }
    };

    let queries = [query];
    let store = parse(&html, &queries).unwrap();
    for article in store.get("main > article.post").unwrap() {
        black_box(article.get(&store, "h1").unwrap().count());
        black_box(article.get(&store, "> section > a[href]").unwrap().count());
    }
}

library_benchmark_group!(
    name = comparison_group;
    benchmarks = bench_runtime_all, bench_macro_all, bench_runtime_first, bench_macro_first
);

main!(library_benchmark_groups = comparison_group);
