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
        assert!(!section.has_raw_text(&store));
        assert!(!section.has_text(&store));
        assert_eq!(section.raw_text(&store), None);
        assert_eq!(section.text(&store), None);
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
    assert!(!hits[0].has_raw_text(&store));
    assert!(!hits[0].has_text(&store));
    assert_eq!(hits[0].raw_text(&store), None);
    assert_eq!(hits[0].text(&store), None);
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
    assert!(!products[0].has_raw_text(&store));
    assert!(!products[0].has_text(&store));
    assert_eq!(products[0].raw_text(&store), None);
    assert_eq!(products[0].text(&store), None);

    let title = products[0].get(&store, "> h1").unwrap().next().unwrap();
    assert_eq!(title.name, "h1");
    assert!(title.inner_html.is_none());
    assert!(!title.has_raw_text(&store));
    assert!(!title.has_text(&store));
    assert_eq!(title.raw_text(&store), None);
    assert_eq!(title.text(&store), None);
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

// --- Generated block newlines at cell boundaries must become tabs ---

#[test]
fn block_at_end_of_cell_keeps_column_boundary() {
    let html = "<table><tr><td><div>A</div></td><td>B</td></tr></table>";

    assert_eq!(text_of(html, "table"), "A\tB");
}

#[test]
fn mixed_block_content_keeps_terminal_cell_boundary() {
    let html = "<table><tr><td>A<div>B</div></td><td>C</td></tr></table>";

    assert_eq!(text_of(html, "table"), "A\nB\tC");
}

#[test]
fn nested_blocks_in_cell_keep_terminal_boundary() {
    let html = "<table><tr><td><div>A</div><div>B</div></td><td>C</td></tr></table>";

    assert_eq!(text_of(html, "table"), "A\nB\tC");
}

#[test]
fn block_cell_boundary_does_not_leak_into_selected_cells() {
    let html = "<table><tr><td><div>A</div></td><td>B</td></tr></table>";

    let queries = &[
        Query::all("table", Save::only_text()).unwrap().build(),
        Query::all("td", Save::only_text()).unwrap().build(),
    ];

    let store = parse(html, queries).unwrap();

    let table = store.get("table").unwrap().next().unwrap();
    assert_eq!(table.text(&store), Some("A\tB"));

    let cells: Vec<_> = store.get("td").unwrap().collect();
    assert_eq!(cells.len(), 2);
    assert_eq!(cells[0].text(&store), Some("A"));
    assert_eq!(cells[1].text(&store), Some("B"));
}

#[test]
fn multi_row_table_still_uses_row_newlines() {
    let html = "<table><tr><td>A</td><td>B</td></tr><tr><td>C</td><td>D</td></tr></table>";
    assert_eq!(text_of(html, "table"), "A\tB\nC\tD");
}

// --- Void Save::none() must not require content finalization ---

#[test]
fn void_save_none_still_stores_match() {
    let html = "<div><br><br></div>";
    let query = Query::all("br", Save::none()).unwrap().build();
    let queries = [query];

    let store = parse(html, &queries).unwrap();
    let breaks: Vec<_> = store.get("br").unwrap().collect();

    assert_eq!(breaks.len(), 2);
    assert_eq!(breaks[0].text(&store), None);
    assert_eq!(breaks[0].raw_text(&store), None);
}

#[test]
fn void_text_capture_still_finalizes_empty_range() {
    let html = "<br>";
    let query = Query::all("br", Save::only_text()).unwrap().build();
    let queries = [query];

    let store = parse(html, &queries).unwrap();
    let br = store.get("br").unwrap().next().unwrap();

    assert_eq!(br.text(&store), Some(""));
    assert_eq!(br.raw_text(&store), None);
}

#[test]
fn void_raw_capture_still_finalizes_empty_range() {
    let html = "<br>";
    let query = Query::all("br", Save::only_raw_text()).unwrap().build();
    let queries = [query];

    let store = parse(html, &queries).unwrap();
    let br = store.get("br").unwrap().next().unwrap();

    assert_eq!(br.raw_text(&store), Some(""));
    assert_eq!(br.text(&store), None);
}

#[test]
fn first_void_save_none_completes_at_synthetic_close() {
    let html = "<br><br><br>";
    let query = Query::first("br", Save::none()).unwrap().build();
    let queries = [query];

    let store = parse(html, &queries).unwrap();
    assert_eq!(store.get("br").unwrap().count(), 1);
}

#[test]
fn save_combinations_populate_only_requested_fields() {
    let html = "<div><p>Hello <b>world</b></p><p></p></div>";
    let cases: &[(&str, Save, bool, bool, bool)] = &[
        ("inner", Save::only_inner_html(), true, false, false),
        ("raw", Save::only_raw_text(), false, true, false),
        ("text", Save::only_text(), false, false, true),
        (
            "inner+raw",
            Save {
                inner_html: true,
                raw_text: true,
                text: false,
            },
            true,
            true,
            false,
        ),
        (
            "inner+text",
            Save {
                inner_html: true,
                raw_text: false,
                text: true,
            },
            true,
            false,
            true,
        ),
        (
            "raw+text",
            Save {
                inner_html: false,
                raw_text: true,
                text: true,
            },
            false,
            true,
            true,
        ),
        ("all", Save::all(), true, true, true),
        ("none", Save::none(), false, false, false),
    ];

    for (label, save, expect_inner, expect_raw, expect_text) in cases {
        let query = Query::all("p", *save).unwrap().build();
        let queries = [query];
        let store = parse(html, &queries).unwrap();
        let paragraphs: Vec<_> = store.get("p").unwrap().collect();
        assert_eq!(paragraphs.len(), 2, "{label}");

        let filled = paragraphs[0];
        let empty = paragraphs[1];

        assert_eq!(
            filled.inner_html.is_some(),
            *expect_inner,
            "{label} filled inner"
        );
        assert_eq!(
            filled.has_raw_text(&store),
            *expect_raw,
            "{label} filled raw"
        );
        assert_eq!(filled.has_text(&store), *expect_text, "{label} filled text");

        if *expect_inner {
            assert_eq!(filled.inner_html, Some("Hello <b>world</b>"));
        }
        if *expect_raw {
            assert_eq!(filled.raw_text(&store), Some("Hello world"));
            // Captured empty content remains distinguishable from uncaptured.
            assert_eq!(empty.raw_text(&store), Some(""));
            assert!(empty.has_raw_text(&store));
        } else {
            assert_eq!(filled.raw_text(&store), None);
            assert_eq!(empty.raw_text(&store), None);
        }
        if *expect_text {
            assert_eq!(filled.text(&store), Some("Hello world"));
            assert_eq!(empty.text(&store), Some(""));
            assert!(empty.has_text(&store));
        } else {
            assert_eq!(filled.text(&store), None);
            assert_eq!(empty.text(&store), None);
        }
    }
}

#[test]
fn nested_elements_keep_independent_text_ranges() {
    let html = "<section>before <strong>inside</strong> after</section>";
    let queries = [
        Query::all("section", Save::all()).unwrap().build(),
        Query::all("strong", Save::all()).unwrap().build(),
    ];
    let store = parse(html, &queries).unwrap();
    let section = store.get("section").unwrap().next().unwrap();
    let strong = store.get("strong").unwrap().next().unwrap();

    assert_eq!(section.raw_text(&store), Some("before inside after"));
    assert_eq!(strong.raw_text(&store), Some("inside"));
    assert_eq!(section.text(&store), Some("before inside after"));
    assert_eq!(strong.text(&store), Some("inside"));
}

#[test]
fn text_sidecar_unallocated_for_inner_html_only() {
    let html = "<p>one</p><p>two</p>";
    let query = Query::all("p", Save::only_inner_html()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    let paragraphs: Vec<_> = store.get("p").unwrap().collect();
    assert_eq!(paragraphs.len(), 2);
    for p in paragraphs {
        assert!(p.inner_html.is_some());
        assert!(!p.has_raw_text(&store));
        assert!(!p.has_text(&store));
        assert_eq!(p.raw_text(&store), None);
        assert_eq!(p.text(&store), None);
    }
}

#[test]
fn text_sidecar_aligned_for_multiple_matching_queries() {
    let html = "<p class=\"x\">A</p>";
    let queries = [
        Query::all("p", Save::only_text()).unwrap().build(),
        Query::all("p.x", Save::only_raw_text()).unwrap().build(),
    ];
    let store = parse(html, &queries).unwrap();
    assert_eq!(store.elements.len(), 2);

    let by_tag = store.get("p").unwrap().next().unwrap();
    let by_class = store.get("p.x").unwrap().next().unwrap();
    assert_eq!(by_tag.text(&store), Some("A"));
    assert_eq!(by_tag.raw_text(&store), None);
    assert_eq!(by_class.raw_text(&store), Some("A"));
    assert_eq!(by_class.text(&store), None);
}

use scah::{ParseError, QueryMultiplexer, Reader, Store, XHtmlParser, parse_without_text_capture};

/// Directly constructs `XHtmlParser` with capacity preallocation and parses HTML.
fn parse_with_general_parser<'html>(
    html: &'html str,
    queries: &'html [Query],
) -> Result<Store<'html, 'html>, ParseError> {
    let selectors = QueryMultiplexer::new(queries);
    let mut parser = XHtmlParser::with_capacity(selectors, html.len());
    let mut reader = Reader::new(html);

    while parser.next(&mut reader) {}

    if let Some(error) = parser.take_parse_error() {
        return Err(error);
    }

    Ok(parser.finish())
}

/// Directly constructs `XHtmlParser` without preallocation (`new`) and parses HTML.
fn parse_with_general_parser_new<'html>(
    html: &'html str,
    queries: &'html [Query],
) -> Result<Store<'html, 'html>, ParseError> {
    let selectors = QueryMultiplexer::new(queries);
    let mut parser = XHtmlParser::new(selectors);
    let mut reader = Reader::new(html);

    while parser.next(&mut reader) {}

    if let Some(error) = parser.take_parse_error() {
        return Err(error);
    }

    Ok(parser.finish())
}

/// Assert complete Store equivalence between general XHtmlParser and specialized NoTextParser.
fn assert_store_equivalent(general: &Store, specialized: &Store, selectors_to_check: &[&str]) {
    assert_eq!(
        general.elements.len(),
        specialized.elements.len(),
        "element count mismatch"
    );

    // Flat arena equality
    for (g, s) in general.elements.iter().zip(specialized.elements.iter()) {
        assert_eq!(g.name, s.name, "element name mismatch");
        assert_eq!(g.id, s.id, "element id mismatch");
        assert_eq!(g.inner_html, s.inner_html, "inner_html mismatch");
        assert_eq!(
            g.attributes(general),
            s.attributes(specialized),
            "attributes mismatch"
        );
        assert_eq!(
            g.has_raw_text(general),
            s.has_raw_text(specialized),
            "has_raw_text mismatch"
        );
        assert_eq!(
            g.has_text(general),
            s.has_text(specialized),
            "has_text mismatch"
        );
        assert_eq!(
            g.raw_text(general),
            None,
            "general raw_text must be None for no-text queries"
        );
        assert_eq!(
            s.raw_text(specialized),
            None,
            "specialized raw_text must be None for no-text queries"
        );
        assert_eq!(
            g.text(general),
            None,
            "general text must be None for no-text queries"
        );
        assert_eq!(
            s.text(specialized),
            None,
            "specialized text must be None for no-text queries"
        );
    }

    // Check Store::get() results for every selector
    for &selector in selectors_to_check {
        let general_matches: Vec<_> = general
            .get(selector)
            .map(|it| it.collect())
            .unwrap_or_default();
        let spec_matches: Vec<_> = specialized
            .get(selector)
            .map(|it| it.collect())
            .unwrap_or_default();
        assert_eq!(
            general_matches.len(),
            spec_matches.len(),
            "Store::get('{selector}') count mismatch"
        );
        for (g_el, s_el) in general_matches.iter().zip(spec_matches.iter()) {
            assert_eq!(
                g_el.name, s_el.name,
                "Store::get('{selector}') element name mismatch"
            );
            assert_eq!(
                g_el.id, s_el.id,
                "Store::get('{selector}') element id mismatch"
            );
            assert_eq!(
                g_el.inner_html, s_el.inner_html,
                "Store::get('{selector}') inner_html mismatch"
            );
            assert_eq!(
                g_el.attributes(general),
                s_el.attributes(specialized),
                "Store::get('{selector}') attributes mismatch"
            );
        }
    }
}

#[test]
fn parity_ordinary_nested_html() {
    let html = "<div><section><p class=\"intro\">Hello <span>world</span></p></section></div>";
    let selectors = [
        "div",
        "section",
        "p",
        "p.intro",
        "span",
        "div p",
        "section > p",
    ];

    for save in [Save::none(), Save::only_inner_html()] {
        let queries: Vec<_> = selectors
            .iter()
            .map(|s| Query::all(s, save).unwrap().build())
            .collect();

        let general = parse_with_general_parser(html, &queries).unwrap();
        let general_new = parse_with_general_parser_new(html, &queries).unwrap();
        let spec = parse_without_text_capture(html, &queries).unwrap();

        assert_store_equivalent(&general, &spec, &selectors);
        assert_store_equivalent(&general_new, &spec, &selectors);
    }
}

#[test]
fn parity_void_elements() {
    let html = "<div><input type=\"text\" value=\"val\"><img src=\"x\"><br><hr></div>";
    let selectors = ["input", "img", "br", "hr", "div > input"];

    for save in [Save::none(), Save::only_inner_html()] {
        let queries: Vec<_> = selectors
            .iter()
            .map(|s| Query::all(s, save).unwrap().build())
            .collect();

        let general = parse_with_general_parser(html, &queries).unwrap();
        let spec = parse_without_text_capture(html, &queries).unwrap();

        assert_store_equivalent(&general, &spec, &selectors);
        assert_eq!(general.get("input").unwrap().count(), 1);
        assert_eq!(general.get("img").unwrap().count(), 1);
        assert_eq!(general.get("br").unwrap().count(), 1);
        assert_eq!(general.get("hr").unwrap().count(), 1);
    }

    // First early exit with void elements
    let first_q = [Query::first("input", Save::none()).unwrap().build()];
    let gen_first = parse_with_general_parser_new(html, &first_q).unwrap();
    let spec_first = parse_without_text_capture(html, &first_q).unwrap();
    assert_store_equivalent(&gen_first, &spec_first, &["input"]);
    assert_eq!(gen_first.get("input").unwrap().count(), 1);
}

#[test]
fn parity_raw_text_elements() {
    let html = "<main><script>if (a < b) { x = \"</style>\"; }</script><style>p { color: red; }</style></main>";
    let selectors = ["main", "script", "style", "main > script", "main > style"];

    for save in [Save::none(), Save::only_inner_html()] {
        let queries: Vec<_> = selectors
            .iter()
            .map(|s| Query::all(s, save).unwrap().build())
            .collect();

        let general = parse_with_general_parser(html, &queries).unwrap();
        let spec = parse_without_text_capture(html, &queries).unwrap();

        assert_store_equivalent(&general, &spec, &selectors);
    }
}

#[test]
fn parity_implied_paragraph_closes() {
    let html = "<div><p>one<p>two<p>three</div>";
    let selectors = ["p", "div > p"];

    for save in [Save::none(), Save::only_inner_html()] {
        let queries: Vec<_> = selectors
            .iter()
            .map(|s| Query::all(s, save).unwrap().build())
            .collect();

        let general = parse_with_general_parser(html, &queries).unwrap();
        let spec = parse_without_text_capture(html, &queries).unwrap();

        assert_store_equivalent(&general, &spec, &selectors);
        assert_eq!(general.get("p").unwrap().count(), 3);
        assert_eq!(spec.get("p").unwrap().count(), 3);

        if save.inner_html {
            let gen_inners: Vec<_> = general.get("p").unwrap().map(|p| p.inner_html).collect();
            let spec_inners: Vec<_> = spec.get("p").unwrap().map(|p| p.inner_html).collect();
            assert_eq!(gen_inners, vec![Some("one"), Some("two"), Some("three")]);
            assert_eq!(spec_inners, gen_inners);
        }
    }
}

#[test]
fn parity_mismatched_closes() {
    let html = "<div><p>mismatched <span>content</div></span></p>";
    let selectors = ["div", "p", "span", "div p"];

    for save in [Save::none(), Save::only_inner_html()] {
        let queries: Vec<_> = selectors
            .iter()
            .map(|s| Query::all(s, save).unwrap().build())
            .collect();

        let general = parse_with_general_parser(html, &queries).unwrap();
        let spec = parse_without_text_capture(html, &queries).unwrap();

        assert_store_equivalent(&general, &spec, &selectors);
    }
}

#[test]
fn parity_table_structures() {
    let html = "<table><thead><tr><th>Header</th></tr></thead><tbody><tr><td>A</td><td>B</td></tr></tbody></table>";
    let selectors = ["table", "thead", "tbody", "tr", "th", "td", "table td"];

    for save in [Save::none(), Save::only_inner_html()] {
        let queries: Vec<_> = selectors
            .iter()
            .map(|s| Query::all(s, save).unwrap().build())
            .collect();

        let general = parse_with_general_parser(html, &queries).unwrap();
        let spec = parse_without_text_capture(html, &queries).unwrap();

        assert_store_equivalent(&general, &spec, &selectors);
    }
}

#[test]
fn parity_eof_open_elements() {
    let html = "<div><section><p>unclosed";
    let selectors = ["div", "section", "p", "div p"];

    for save in [Save::none(), Save::only_inner_html()] {
        let queries: Vec<_> = selectors
            .iter()
            .map(|s| Query::all(s, save).unwrap().build())
            .collect();

        let general = parse_with_general_parser(html, &queries).unwrap();
        let spec = parse_without_text_capture(html, &queries).unwrap();

        assert_store_equivalent(&general, &spec, &selectors);
    }
}

#[test]
fn parity_mixed_case_tags() {
    let html = "<DIV><P Class=\"Test\">Upper</P><BR></DIV>";
    let selectors = ["div", "p", "p.Test", "br"];

    for save in [Save::none(), Save::only_inner_html()] {
        let queries: Vec<_> = selectors
            .iter()
            .map(|s| Query::all(s, save).unwrap().build())
            .collect();

        let general = parse_with_general_parser(html, &queries).unwrap();
        let spec = parse_without_text_capture(html, &queries).unwrap();

        assert_store_equivalent(&general, &spec, &selectors);
    }
}

#[test]
fn parity_nested_query_relative_lookup() {
    let html = "<div><section><p>child 1</p><p>child 2</p></section></div>";

    let q1 = [Query::all("div", Save::only_inner_html())
        .unwrap()
        .then(|div| Ok([div.all("p", Save::only_inner_html())?]))
        .unwrap()
        .build()];
    let q2 = [q1[0].clone()];

    let general = parse_with_general_parser(html, &q1).unwrap();
    let spec = parse_without_text_capture(html, &q2).unwrap();

    let gen_parent = general.get("div").unwrap().next().unwrap();
    let spec_parent = spec.get("div").unwrap().next().unwrap();

    let gen_children: Vec<_> = gen_parent.get(&general, "p").unwrap().collect();
    let spec_children: Vec<_> = spec_parent.get(&spec, "p").unwrap().collect();

    assert_eq!(gen_children.len(), spec_children.len());
    assert_eq!(gen_children.len(), 2);
    for (g_c, s_c) in gen_children.iter().zip(spec_children.iter()) {
        assert_eq!(g_c.name, s_c.name);
        assert_eq!(g_c.inner_html, s_c.inner_html);
    }
}

#[test]
fn parity_early_exit_reader_position() {
    let html = format!(
        "<div id=\"hit\">x</div>{}",
        "<span>filler</span>".repeat(5_000)
    );
    let q1 = [Query::first("#hit", Save::none()).unwrap().build()];
    let q2 = [q1[0].clone()];

    let (gen_store, gen_pos) =
        scah::bench_internals::parse_general_with_position(&html, &q1).unwrap();
    let (spec_store, spec_pos) =
        scah::bench_internals::parse_no_text_with_position(&html, &q2).unwrap();

    assert!(gen_pos < html.len(), "general parser must stop before EOF");
    assert!(
        spec_pos < html.len(),
        "specialized parser must stop before EOF"
    );
    assert_eq!(
        gen_pos, spec_pos,
        "both parsers must stop at the exact same byte position"
    );
    assert_eq!(gen_store.get("#hit").unwrap().count(), 1);
    assert_eq!(spec_store.get("#hit").unwrap().count(), 1);
}

#[test]
fn real_maximum_depth_boundary_succeeds() {
    // 65,533 open <div> tags + 1 <p> tag = 65,534 total open elements (exact MAX_ELEMENT_DEPTH limit)
    let opens = "<div>".repeat(65_533);
    let html = format!("{opens}<p>leaf</p>");
    let q1 = [Query::all("p", Save::none()).unwrap().build()];
    let q2 = [q1[0].clone()];

    let general_store = parse_with_general_parser(&html, &q1).unwrap();
    let spec_store = parse_without_text_capture(&html, &q2).unwrap();

    assert_eq!(general_store.get("p").unwrap().count(), 1);
    assert_eq!(spec_store.get("p").unwrap().count(), 1);
}

#[test]
fn real_maximum_depth_boundary_plus_one_fails() {
    // 65,534 open <div> tags + 1 <p> tag = 65,535 total open elements (exceeds MAX_ELEMENT_DEPTH limit)
    let opens = "<div>".repeat(65_534);
    let html = format!("{opens}<p>leaf</p>");
    let q1 = [Query::all("p", Save::none()).unwrap().build()];
    let q2 = [q1[0].clone()];

    let gen_err = parse_with_general_parser(&html, &q1).unwrap_err();
    let spec_err = parse_without_text_capture(&html, &q2).unwrap_err();

    assert_eq!(gen_err, ParseError::MaximumDepthExceeded);
    assert_eq!(spec_err, ParseError::MaximumDepthExceeded);
}

#[test]
fn parse_without_text_capture_rejects_raw_text() {
    let queries = &[Query::all("p", Save::only_raw_text()).unwrap().build()];

    let err = parse_without_text_capture("<p>x</p>", queries).unwrap_err();

    assert_eq!(err, ParseError::TextCaptureRequired);
}

#[test]
fn parse_without_text_capture_rejects_normalized_text() {
    let queries = &[Query::all("p", Save::only_text()).unwrap().build()];

    let err = parse_without_text_capture("<p>x</p>", queries).unwrap_err();

    assert_eq!(err, ParseError::TextCaptureRequired);
}

#[test]
fn parse_without_text_capture_rejects_all() {
    let queries = &[Query::all("p", Save::all()).unwrap().build()];

    let err = parse_without_text_capture("<p>x</p>", queries).unwrap_err();

    assert_eq!(err, ParseError::TextCaptureRequired);
}

#[test]
fn parse_without_text_capture_accepts_inner_html() {
    let queries = &[Query::all("p", Save::only_inner_html()).unwrap().build()];

    let store = parse_without_text_capture("<p>x</p>", queries).unwrap();
    let element = store.get("p").unwrap().next().unwrap();

    assert_eq!(element.inner_html, Some("x"));
}

#[test]
fn parse_without_text_capture_accepts_none() {
    let queries = &[Query::all("p", Save::none()).unwrap().build()];

    let store = parse_without_text_capture("<p>x</p>", queries).unwrap();
    let element = store.get("p").unwrap().next().unwrap();

    assert_eq!(element.name, "p");
    assert!(element.inner_html.is_none());
}

#[test]
fn parse_dispatches_correctly_for_all_save_modes() {
    let raw_q = &[Query::all("p", Save::only_raw_text()).unwrap().build()];
    let store_raw = parse("<p>Hello</p>", raw_q).unwrap();
    assert_eq!(
        store_raw
            .get("p")
            .unwrap()
            .next()
            .unwrap()
            .raw_text(&store_raw),
        Some("Hello")
    );

    let text_q = &[Query::all("p", Save::only_text()).unwrap().build()];
    let store_text = parse("<p>Hello</p>", text_q).unwrap();
    assert_eq!(
        store_text
            .get("p")
            .unwrap()
            .next()
            .unwrap()
            .text(&store_text),
        Some("Hello")
    );

    let both_q = &[Query::all("p", Save::all()).unwrap().build()];
    let store_both = parse("<p>Hello</p>", both_q).unwrap();
    let p = store_both.get("p").unwrap().next().unwrap();
    assert_eq!(p.raw_text(&store_both), Some("Hello"));
    assert_eq!(p.text(&store_both), Some("Hello"));
}
