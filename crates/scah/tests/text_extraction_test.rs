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

/// Assert generated/collapsed separator canonicalization.
///
/// Do not apply this to selected `pre` / `textarea` output: literal
/// preformatted whitespace may legally produce `" \n"` when a structural
/// newline follows source-literal trailing spaces.
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
fn save_none_matches_remain_in_store_without_content() {
    let html = r#"
        <section id="s1"><p class="item">one</p></section>
        <section id="s2"><p class="item">two</p></section>
    "#;
    let queries = &[Query::all("section", Save::none()).unwrap().build()];
    let store = parse(html, queries).unwrap();

    let sections: Vec<_> = store.get("section").unwrap().collect();
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].id, Some("s1"));
    assert_eq!(sections[1].id, Some("s2"));
    for section in &sections {
        assert!(section.inner_html.is_none());
        assert!(section.raw_text.is_none());
        assert!(section.text.is_none());
    }
}

#[test]
fn save_none_first_completes_after_element_lifecycle() {
    let html = r#"
        <div id="hit"><span>inside</span></div>
        <div id="later">should not be required for First</div>
    "#;
    let queries = &[Query::first("div", Save::none()).unwrap().build()];
    let store = parse(html, queries).unwrap();

    let hits: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, Some("hit"));
    assert!(hits[0].inner_html.is_none());
    assert!(hits[0].raw_text.is_none());
    assert!(hits[0].text.is_none());
}

#[test]
fn save_none_nested_first_completes_with_parent_child() {
    let html = r#"
        <div class="product"><h1>Product 0</h1><p>desc</p></div>
        <div class="product"><h1>Product 1</h1></div>
    "#;
    let queries = &[query! {
        first("div.product", Save::none()) => {
            first("> h1", Save::none()),
        }
    }];
    let store = parse(html, queries).unwrap();

    let products: Vec<_> = store.get("div.product").unwrap().collect();
    assert_eq!(products.len(), 1);
    assert!(products[0].inner_html.is_none());
    assert!(products[0].raw_text.is_none());
    assert!(products[0].text.is_none());

    let title = products[0].get(&store, "> h1").unwrap().next().unwrap();
    assert_eq!(title.name, "h1");
    assert!(title.inner_html.is_none());
    assert!(title.raw_text.is_none());
    assert!(title.text.is_none());
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

#[test]
fn finalized_pre_range_survives_following_block() {
    let html = "<pre>A   </pre><div></div>";
    let queries = &[
        Query::all("pre", Save::only_text()).unwrap().build(),
        Query::all("div", Save::only_text()).unwrap().build(),
    ];
    let store = parse(html, queries).unwrap();
    let pre = store.get("pre").unwrap().next().unwrap();
    let div = store.get("div").unwrap().next().unwrap();
    assert_eq!(pre.text(&store), Some("A   "));
    assert_eq!(div.text(&store), Some(""));
}

#[test]
fn finalized_pre_range_survives_following_section() {
    let html = "<pre>A   </pre><section>B</section>";
    let queries = &[
        Query::all("pre", Save::only_text()).unwrap().build(),
        Query::all("section", Save::only_text()).unwrap().build(),
    ];
    let store = parse(html, queries).unwrap();
    assert_eq!(
        store.get("pre").unwrap().next().unwrap().text(&store),
        Some("A   ")
    );
    assert_eq!(
        store.get("section").unwrap().next().unwrap().text(&store),
        Some("B")
    );
}

#[test]
fn finalized_pre_range_survives_following_hidden_block() {
    let html = "<pre>A   </pre><div hidden><div>X</div></div>";
    let queries = &[
        Query::all("pre", Save::only_text()).unwrap().build(),
        Query::all("div", Save::only_text()).unwrap().build(),
    ];
    let store = parse(html, queries).unwrap();
    let mut divs = store.get("div").unwrap();
    let outer = divs.next().unwrap();
    assert_eq!(
        store.get("pre").unwrap().next().unwrap().text(&store),
        Some("A   ")
    );
    assert_eq!(outer.text(&store), Some(""));
}

#[test]
fn multiple_finalized_pre_ranges_survive_following_div() {
    let html = "<pre>A   </pre><pre>B  </pre><div>C</div>";
    let queries = &[
        Query::all("pre", Save::only_text()).unwrap().build(),
        Query::all("div", Save::only_text()).unwrap().build(),
    ];
    let store = parse(html, queries).unwrap();

    let mut pres = store.get("pre").unwrap();
    let first_pre = pres.next().unwrap();
    let second_pre = pres.next().unwrap();
    let div = store.get("div").unwrap().next().unwrap();

    // Access-order independence: read div first, then reverse pre order.
    assert_eq!(div.text(&store), Some("C"));
    assert_eq!(second_pre.text(&store), Some("B  "));
    assert_eq!(first_pre.text(&store), Some("A   "));

    // And again in original order.
    assert_eq!(first_pre.text(&store), Some("A   "));
    assert_eq!(second_pre.text(&store), Some("B  "));
    assert_eq!(div.text(&store), Some("C"));
}

#[test]
fn selected_span_inside_pre_preserves_whitespace() {
    let html = "<pre><span>  A  </span></pre>";
    assert_eq!(text_of(html, "span"), "  A  ");
}

#[test]
fn selected_nested_strong_inside_pre_preserves_whitespace() {
    let html = "<pre><span><strong>  A  </strong></span></pre>";
    assert_eq!(text_of(html, "strong"), "  A  ");
}

#[test]
fn selected_strong_inside_pre_preserves_intervening_leading_newline() {
    let html = "<pre><strong>\n  A\n</strong></pre>";
    assert_eq!(text_of(html, "strong"), "\n  A\n");
}

#[test]
fn ordinary_span_outside_pre_trims_collapsed_edges() {
    let html = "<div><span>  A  </span></div>";
    assert_eq!(text_of(html, "span"), "A");
}

#[test]
fn suppressed_descendant_inside_pre_stays_empty() {
    let html = "<pre>A<span hidden>  X  </span>B</pre>";
    let queries = &[
        Query::all("pre", Save::only_text()).unwrap().build(),
        Query::all("span", Save::only_text()).unwrap().build(),
    ];
    let store = parse(html, queries).unwrap();
    assert_eq!(
        store.get("span").unwrap().next().unwrap().text(&store),
        Some("")
    );
    assert_eq!(
        store.get("pre").unwrap().next().unwrap().text(&store),
        Some("AB")
    );
}

#[test]
fn inline_descendant_inside_hidden_pre_subtree_does_not_flush_pending_separator() {
    let baseline = "<pre>A<br><span hidden></span></pre>";
    let nested = "<pre>A<br><span hidden><i></i></span></pre>";

    assert_eq!(text_of(baseline, "pre"), "A");
    assert_eq!(text_of(nested, "pre"), "A");
}

#[test]
fn block_descendant_inside_hidden_pre_subtree_does_not_flush_pending_separator() {
    let baseline = "<pre>A<br><span hidden></span></pre>";
    let nested = "<pre>A<br><span hidden><div></div></span></pre>";

    assert_eq!(text_of(baseline, "pre"), "A");
    assert_eq!(text_of(nested, "pre"), "A");
}

#[test]
fn table_cell_inside_hidden_subtree_does_not_flush_pending_separator() {
    let html = concat!(
        "<pre>A<br>",
        "<span hidden><table><tr><td>X</td></tr></table></span>",
        "</pre>",
    );

    assert_eq!(text_of(html, "pre"), "A");
}

#[test]
fn break_inside_nested_hidden_descendant_does_not_emit_separator() {
    let html = "<pre>A<span hidden><i><br></i></span>B</pre>";
    assert_eq!(text_of(html, "pre"), "AB");
}

#[test]
fn hidden_subtree_structure_does_not_change_visible_ancestor_text() {
    let variants = [
        "<pre>A<br><span hidden></span></pre>",
        "<pre>A<br><span hidden><i></i></span></pre>",
        "<pre>A<br><span hidden><div></div></span></pre>",
        "<pre>A<br><span hidden><div><i><br></i></div></span></pre>",
    ];

    for html in variants {
        assert_eq!(text_of(html, "pre"), "A", "failed for {html}");
    }
}

#[test]
fn parent_and_child_selected_inside_pre_preserve_whitespace() {
    let html = "<pre><span>  A  </span></pre>";
    let queries = &[
        Query::all("pre", Save::only_text()).unwrap().build(),
        Query::all("span", Save::only_text()).unwrap().build(),
    ];
    let store = parse(html, queries).unwrap();
    assert_eq!(
        store.get("pre").unwrap().next().unwrap().text(&store),
        Some("  A  ")
    );
    assert_eq!(
        store.get("span").unwrap().next().unwrap().text(&store),
        Some("  A  ")
    );
}

#[test]
fn pre_followed_by_block_keeps_literal_spaces_before_newline() {
    // Literal preformatted trailing spaces must remain; a following structural
    // newline may follow those spaces. The global "no space before newline"
    // rule applies only to generated/collapsed whitespace.
    let html = "<pre>A   </pre><div>B</div>";
    let queries = &[
        Query::all("pre", Save::only_text()).unwrap().build(),
        Query::all("div", Save::only_text()).unwrap().build(),
    ];
    let store = parse(html, queries).unwrap();
    assert_eq!(
        store.get("pre").unwrap().next().unwrap().text(&store),
        Some("A   ")
    );
    assert_eq!(
        store.get("div").unwrap().next().unwrap().text(&store),
        Some("B")
    );
}

// --- Range ownership: parent separators must not leak into Preserve ranges ---

#[test]
fn textarea_range_excludes_parent_collapsed_space() {
    let html = "<div>A <textarea>B</textarea></div>";
    assert_eq!(text_of(html, "textarea"), "B");
}

#[test]
fn textarea_range_excludes_parent_break() {
    let html = "<div>A<br><textarea>B</textarea></div>";
    assert_eq!(text_of(html, "textarea"), "B");
}

#[test]
fn selected_inline_child_inside_pre_excludes_previous_break() {
    let html = "<pre>A<br><span>B</span></pre>";
    assert_eq!(text_of(html, "span"), "B");
}

#[test]
fn parent_still_contains_separator_excluded_from_child() {
    let html = "<pre>A<br><span>B</span></pre>";
    let queries = &[
        Query::all("pre", Save::only_text()).unwrap().build(),
        Query::all("span", Save::only_text()).unwrap().build(),
    ];
    let store = parse(html, queries).unwrap();

    let pre = store.get("pre").unwrap().next().unwrap();
    let span = store.get("span").unwrap().next().unwrap();

    assert_eq!(pre.text(&store), Some("A\nB"));
    assert_eq!(span.text(&store), Some("B"));
}

#[test]
fn selected_preformatted_descendant_excludes_previous_block_separator() {
    let html = "<pre><div>A</div><span>B</span></pre>";
    assert_eq!(text_of(html, "span"), "B");
}

// --- Nested pre/textarea each own the initial-newline rule ---

#[test]
fn textarea_inside_pre_strips_its_own_initial_newline() {
    assert_eq!(
        text_of("<pre><textarea>\nB</textarea></pre>", "textarea"),
        "B"
    );
}

#[test]
fn nested_pre_strips_its_own_initial_newline() {
    let html = "<pre>A<pre>\nB</pre>C</pre>";
    let query = Query::all("pre", Save::only_text()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();

    let mut pres = store.get("pre").unwrap();
    let outer = pres.next().unwrap();
    let inner = pres.next().unwrap();

    assert_eq!(inner.text(&store), Some("B"));
    // Inner `pre` is a block, so its opening boundary contributes a newline
    // to the outer preformatted parent; closing contributes another before C.
    assert_eq!(outer.text(&store), Some("A\nB\nC"));
}

#[test]
fn child_inside_nested_pre_cancels_initial_newline() {
    // textarea is a rawtext element, so markup inside it is literal text and
    // cannot cancel eligibility. Nested pre does parse children/comments.
    let html = "<pre><pre><span></span>\nB</pre></pre>";
    let query = Query::all("pre", Save::only_text()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    let mut pres = store.get("pre").unwrap();
    let _outer = pres.next().unwrap();
    let inner = pres.next().unwrap();
    assert_eq!(inner.text(&store), Some("\nB"));
}

#[test]
fn comment_inside_nested_pre_cancels_initial_newline() {
    let html = "<pre><pre><!--x-->\nB</pre></pre>";
    let query = Query::all("pre", Save::only_text()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    let mut pres = store.get("pre").unwrap();
    let _outer = pres.next().unwrap();
    let inner = pres.next().unwrap();
    assert_eq!(inner.text(&store), Some("\nB"));
}

#[test]
fn nested_textarea_strips_initial_crlf() {
    assert_eq!(
        text_of("<pre><textarea>\r\nB</textarea></pre>", "textarea"),
        "B"
    );
}

#[test]
fn nested_textarea_strips_initial_cr() {
    assert_eq!(
        text_of("<pre><textarea>\rB</textarea></pre>", "textarea"),
        "B"
    );
}

// --- Empty table cells must preserve column boundaries ---

#[test]
fn empty_table_cell_preserves_column_boundary() {
    let html = "<table><tr><td>A</td><td></td><td>B</td></tr></table>";
    assert_eq!(text_of(html, "table"), "A\t\tB");
}

#[test]
fn consecutive_empty_table_cells_preserve_all_boundaries() {
    let html = "<table><tr><td>A</td><td></td><td></td><td>B</td></tr></table>";
    assert_eq!(text_of(html, "table"), "A\t\t\tB");
}

#[test]
fn leading_empty_table_cell_preserves_boundary() {
    // Leading synthetic tabs are intentionally suppressed (no leading separator
    // on an empty tape). A leading empty cell therefore contributes no visible
    // tab; only internal empty cells create extra column boundaries.
    let html = "<table><tr><td></td><td>B</td></tr></table>";
    assert_eq!(text_of(html, "table"), "B");
}

#[test]
fn selected_table_cells_exclude_neighbor_separators() {
    let html = "<table><tr><td>A</td><td></td><td>B</td></tr></table>";
    let queries = &[
        Query::all("table", Save::only_text()).unwrap().build(),
        Query::all("td", Save::only_text()).unwrap().build(),
    ];
    let store = parse(html, queries).unwrap();

    let table = store.get("table").unwrap().next().unwrap();
    assert_eq!(table.text(&store), Some("A\t\tB"));

    let cells: Vec<_> = store.get("td").unwrap().collect();
    assert_eq!(cells.len(), 3);
    assert_eq!(cells[0].text(&store), Some("A"));
    assert_eq!(cells[1].text(&store), Some(""));
    assert_eq!(cells[2].text(&store), Some("B"));
}

#[test]
fn indented_empty_table_cells_preserve_boundaries() {
    let html = r#"
        <table>
          <tr>
            <td>A</td>
            <td></td>
            <td>B</td>
          </tr>
        </table>
    "#;

    assert_eq!(text_of(html, "table"), "A\t\tB");
}

#[test]
fn hidden_table_cell_does_not_create_column_separator() {
    let html = "<table><tr><td>A</td><td hidden>X</td><td>B</td></tr></table>";
    assert_eq!(text_of(html, "table"), "A\tB");
}

#[test]
fn table_cell_after_preserved_space_keeps_tab_boundary() {
    let html = "<table><tr><td><textarea>A </textarea></td><td>B</td></tr></table>";
    assert_eq!(text_of(html, "table"), "A \tB");
}

#[test]
fn table_cell_after_preserved_newline_keeps_tab_boundary() {
    // A cell that literally ends with a newline still emits the structural
    // tab boundary, so the next cell remains distinguishable: "A\n\tB".
    let html = "<table><tr><td><textarea>A\n</textarea></td><td>B</td></tr></table>";
    assert_eq!(text_of(html, "table"), "A\n\tB");
}

#[test]
fn selected_cells_after_preserved_whitespace_exclude_neighbor_tabs() {
    let html = "<table><tr><td><textarea>A </textarea></td><td>B</td></tr></table>";
    let queries = &[
        Query::all("table", Save::only_text()).unwrap().build(),
        Query::all("td", Save::only_text()).unwrap().build(),
    ];
    let store = parse(html, queries).unwrap();

    let table = store.get("table").unwrap().next().unwrap();
    assert_eq!(table.text(&store), Some("A \tB"));

    let cells: Vec<_> = store.get("td").unwrap().collect();
    assert_eq!(cells.len(), 2);
    assert_eq!(cells[0].text(&store), Some("A "));
    assert_eq!(cells[1].text(&store), Some("B"));
}

#[test]
fn selected_cells_after_preserved_newline_exclude_neighbor_tabs() {
    let html = "<table><tr><td><textarea>A\n</textarea></td><td>B</td></tr></table>";
    let queries = &[
        Query::all("table", Save::only_text()).unwrap().build(),
        Query::all("td", Save::only_text()).unwrap().build(),
    ];
    let store = parse(html, queries).unwrap();

    let table = store.get("table").unwrap().next().unwrap();
    assert_eq!(table.text(&store), Some("A\n\tB"));

    let cells: Vec<_> = store.get("td").unwrap().collect();
    assert_eq!(cells.len(), 2);
    assert_eq!(cells[0].text(&store), Some("A\n"));
    assert_eq!(cells[1].text(&store), Some("B"));
}
