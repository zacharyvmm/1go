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
    // Initial newline removed; indentation and trailing newline preserved.
    assert_eq!(pre.text(&store), Some("  alpha\n    beta\n"));
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

fn assert_canonical_separators(text: &str) {
    assert!(!text.contains("\n "), "space after newline: {text:?}");
    assert!(!text.contains("\n\t"), "tab after newline: {text:?}");
    assert!(!text.contains(" \n"), "space before newline: {text:?}");
    assert!(!text.contains("\n\n"), "duplicate blank line: {text:?}");
}

fn text_of(html: &str, selector: &str) -> String {
    let query = Query::all(selector, Save::only_text()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    store
        .get(selector)
        .unwrap()
        .next()
        .unwrap()
        .text(&store)
        .unwrap()
        .to_string()
}

fn raw_and_text(html: &str, selector: &str) -> (Option<String>, Option<String>) {
    let query = Query::all(selector, Save::all()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    let el = store.get(selector).unwrap().next().unwrap();
    (
        el.raw_text(&store).map(str::to_string),
        el.text(&store).map(str::to_string),
    )
}

#[test]
fn separator_canonicalization_no_space_after_newline() {
    let html = "<section>\n  <div>A</div>\n  <div>B</div>\n</section>";
    let text = text_of(html, "section");
    assert_eq!(text, "A\nB");
    assert_canonical_separators(&text);
}

#[test]
fn separator_canonicalization_nested_blocks_no_duplicate_newline() {
    let html = "<section>A<div><div>B</div></div>C</section>";
    let text = text_of(html, "section");
    assert_eq!(text, "A\nB\nC");
    assert_canonical_separators(&text);
}

#[test]
fn separator_canonicalization_indented_block_content() {
    let html = "<section>A<div>\n  B\n</div>C</section>";
    let text = text_of(html, "section");
    assert_eq!(text, "A\nB\nC");
    assert_canonical_separators(&text);
}

#[test]
fn table_cells_use_tab_without_newline_space() {
    let html = "<table>\n  <tr>\n    <td>A</td>\n    <td>B</td>\n  </tr>\n</table>";
    let text = text_of(html, "table");
    assert_eq!(text, "A\tB");
    assert_canonical_separators(&text);
}

#[test]
fn hidden_block_does_not_emit_separator() {
    assert_eq!(text_of("<div>A<div hidden>X</div>B</div>", "div"), "AB");
}

#[test]
fn hidden_br_does_not_emit_separator() {
    assert_eq!(text_of("<div>A<br hidden>B</div>", "div"), "AB");
}

#[test]
fn nested_hidden_does_not_emit_separator() {
    assert_eq!(
        text_of("<div>A<div hidden><div>X</div></div>B</div>", "div"),
        "AB"
    );
}

#[test]
fn script_omitted_from_normalized_text() {
    let (raw, text) = raw_and_text("<div>A<script><div>X</div></script>B</div>", "div");
    assert_eq!(text.as_deref(), Some("AB"));
    assert!(raw.as_deref().unwrap().contains("X"));
}

#[test]
fn template_omitted_from_normalized_text() {
    assert_eq!(
        text_of("<div>A<template><p>X</p></template>B</div>", "div"),
        "AB"
    );
}

#[test]
fn selected_hidden_div_is_empty_text_with_raw() {
    let (raw, text) = raw_and_text("<div hidden>A<span>B</span>C</div>", "div");
    assert_eq!(raw.as_deref(), Some("ABC"));
    assert_eq!(text.as_deref(), Some(""));
}

#[test]
fn visible_br_inserts_linebreak() {
    assert_eq!(text_of("<p>A<br>B</p>", "p"), "A\nB");
}

#[test]
fn hidden_br_inside_paragraph() {
    assert_eq!(text_of("<p>A<br hidden>B</p>", "p"), "AB");
}

#[test]
fn visible_hr_inserts_linebreak() {
    assert_eq!(text_of("<div>A<hr>B</div>", "div"), "A\nB");
}

#[test]
fn selected_br_and_hr_are_empty() {
    let br = text_of("<br>", "br");
    let hr = text_of("<hr>", "hr");
    assert_eq!(br, "");
    assert_eq!(hr, "");
}

#[test]
fn pre_preserves_indentation_after_initial_newline() {
    let (raw, text) = raw_and_text("<pre>\n  alpha\n    beta\n</pre>", "pre");
    assert_eq!(raw.as_deref(), Some("\n  alpha\n    beta\n"));
    assert_eq!(text.as_deref(), Some("  alpha\n    beta\n"));
}

#[test]
fn textarea_preserves_preformatted_and_decodes_entities() {
    let text = text_of("<textarea>\n  alpha &amp; beta\n</textarea>", "textarea");
    assert_eq!(text, "  alpha & beta\n");
}

#[test]
fn intervening_tag_cancels_pre_initial_newline() {
    let text = text_of("<pre><span>\nX</span></pre>", "pre");
    assert_eq!(text, "\nX");
}

#[test]
fn intervening_comment_cancels_pre_initial_newline() {
    let text = text_of("<pre><!-- comment -->\nX</pre>", "pre");
    assert_eq!(text, "\nX");
}

#[test]
fn pre_normalizes_crlf_and_cr() {
    let text = text_of("<pre>\r\n  A\rB\nC</pre>", "pre");
    assert_eq!(text, "  A\nB\nC");
}

#[test]
fn parent_of_pre_may_trim_outer_edges() {
    let html = "<section><pre>\n  A\n</pre></section>";
    let queries = &[
        Query::all("section", Save::only_text()).unwrap().build(),
        Query::all("pre", Save::only_text()).unwrap().build(),
    ];
    let store = parse(html, queries).unwrap();
    let section = store.get("section").unwrap().next().unwrap();
    let pre = store.get("pre").unwrap().next().unwrap();
    assert_eq!(pre.text(&store), Some("  A\n"));
    // Ordinary parent applies collapsed-edge trimming.
    assert_eq!(section.text(&store), Some("A"));
}

#[test]
fn entity_matrix_html_data_state() {
    let cases = [
        ("<p>&amp;</p>", "&"),
        ("<p>&amp</p>", "&"),
        ("<p>&copy test</p>", "© test"),
        ("<p>&#65;</p>", "A"),
        ("<p>&#65</p>", "A"),
        ("<p>&#x41;</p>", "A"),
        ("<p>&#x41</p>", "A"),
        ("<p>&#0;</p>", "\u{FFFD}"),
        ("<p>&#xD800;</p>", "\u{FFFD}"),
        ("<p>&#x110000;</p>", "\u{FFFD}"),
        ("<p>&#128;</p>", "€"),
        ("<p>&NotEqualTilde;</p>", "\u{2242}\u{0338}"),
        ("<p>&unknown;</p>", "&unknown;"),
        ("<p>&</p>", "&"),
        ("<p>&#;</p>", "&#;"),
        ("<p>&#x;</p>", "&#x;"),
    ];
    for (html, expected) in cases {
        assert_eq!(text_of(html, "p"), expected, "html={html}");
    }
}

#[test]
fn raw_only_skips_normalized_tape() {
    let html = "<div hidden>A&amp;B</div>";
    let query = Query::all("div", Save::only_raw_text()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    let div = store.get("div").unwrap().next().unwrap();
    assert_eq!(div.raw_text(&store), Some("A&amp;B"));
    assert_eq!(div.text(&store), None);
}

#[test]
fn text_only_skips_raw_tape() {
    let html = "<div>A&amp;B</div>";
    let query = Query::all("div", Save::only_text()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    let div = store.get("div").unwrap().next().unwrap();
    assert_eq!(div.text(&store), Some("A&B"));
    assert_eq!(div.raw_text(&store), None);
}

#[test]
fn no_content_skips_both_tapes() {
    let html = "<div>A&amp;B</div>";
    let query = Query::all("div", Save::none()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    let div = store.get("div").unwrap().next().unwrap();
    assert_eq!(div.text(&store), None);
    assert_eq!(div.raw_text(&store), None);
}

#[test]
fn hidden_attribute_case_insensitive() {
    for html in [
        "<div>A<div HIDDEN>X</div>B</div>",
        "<div>A<div hidden=\"\">X</div>B</div>",
        "<div>A<div hidden=\"hidden\">X</div>B</div>",
    ] {
        assert_eq!(text_of(html, "div"), "AB", "html={html}");
    }
}
