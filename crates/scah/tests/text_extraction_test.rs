use scah::{Query, Save, parse, query};

#[test]
fn inline_boundaries_preserve_source_spacing() {
    let with_spaces = "<p>Hello <strong>world</strong>!</p>";
    let without_spaces = "<p>Hello<strong>world</strong>!</p>";

    for (html, expected) in [
        (with_spaces, "Hello world!"),
        (without_spaces, "Helloworld!"),
    ] {
        let query = Query::all("p", Save::all()).unwrap().build();
        let queries = [query];
        let store = parse(html, &queries).unwrap();
        let p = store.get("p").unwrap().next().unwrap();
        assert_eq!(p.raw_text(&store), Some(expected));
        assert_eq!(p.text(&store), Some(expected));
    }
}

#[test]
fn br_inserts_normalized_line_break() {
    let html = "<p>Hello<br>world</p>";
    let query = Query::all("p", Save::all()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    let p = store.get("p").unwrap().next().unwrap();

    assert_eq!(p.raw_text(&store), Some("Helloworld"));
    assert_eq!(p.text(&store), Some("Hello\nworld"));
}

#[test]
fn pre_strips_initial_newline_in_normalized_text() {
    let html = "<pre>\n  alpha\n    beta\n</pre>";
    let query = Query::all("pre", Save::all()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    let pre = store.get("pre").unwrap().next().unwrap();

    assert_eq!(pre.raw_text(&store), Some("\n  alpha\n    beta\n"));
    assert_eq!(pre.text(&store), Some("alpha\n    beta"));
}

#[test]
fn overlapping_parent_and_child_ranges_share_tape() {
    let html = "<section>before <strong>inside</strong> after</section>";
    let queries = &[
        Query::all("section", Save::all()).unwrap().build(),
        Query::all("strong", Save::all()).unwrap().build(),
    ];
    let store = parse(html, queries).unwrap();

    let section = store.get("section").unwrap().next().unwrap();
    let strong = store.get("strong").unwrap().next().unwrap();

    assert_eq!(section.raw_text(&store), Some("before inside after"));
    assert_eq!(strong.raw_text(&store), Some("inside"));
    assert_eq!(section.text(&store), Some("before inside after"));
    assert_eq!(strong.text(&store), Some("inside"));
}

#[test]
fn first_with_normalized_text_captures_before_early_exit() {
    let html =
        "<div id=\"hit\">important text</div>".to_string() + &"<span>filler</span>".repeat(1_000);
    let query = Query::first("#hit", Save::only_text()).unwrap().build();
    let queries = [query];
    let store = parse(&html, &queries).unwrap();

    let hit = store.get("#hit").unwrap().next().unwrap();
    assert_eq!(hit.text(&store), Some("important text"));
    assert_eq!(hit.raw_text(&store), None);
}

#[test]
fn first_with_all_text_fields_finalizes_block_newlines() {
    let html = r#"
        <div class="product"><h1>Product 0</h1><span class="rating">3/5</span><p class="description">Description</p></div>
        <div class="product"><h1>Product 1</h1></div>
    "#;
    let queries = &[query! {
        first("div.product", Save::all()) => {
            first("> h1", Save::all()),
        }
    }];
    let store = parse(html, queries).unwrap();

    let product = store.get("div.product").unwrap().next().unwrap();
    assert_eq!(product.text(&store), Some("Product 0\n3/5\nDescription"));

    let title = product.get(&store, "> h1").unwrap().next().unwrap();
    assert_eq!(title.text(&store), Some("Product 0"));
}

#[test]
fn first_with_raw_text_only_skips_normalized_tape() {
    let html = "<div class=\"product\"><h1>Product 0</h1><p>tail</p></div><div class=\"product\"><h1>Product 1</h1></div>";
    let queries = &[query! {
        first("div.product", Save::only_raw_text()) => {
            first("> h1", Save::only_raw_text()),
        }
    }];
    let store = parse(html, queries).unwrap();

    let product = store.get("div.product").unwrap().next().unwrap();
    assert_eq!(product.raw_text(&store), Some("Product 0tail"));
    assert_eq!(product.text(&store), None);

    let title = product.get(&store, "> h1").unwrap().next().unwrap();
    assert_eq!(title.raw_text(&store), Some("Product 0"));
    assert_eq!(title.text(&store), None);
}
