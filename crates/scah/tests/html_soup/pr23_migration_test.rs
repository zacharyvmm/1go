//! Regression tests migrated from the closed PR #23 (`global-maxima`).
//!
//! These encode the correctness behaviours described in `PR23_HANDOFF.md`:
//!   * Priority 1 — selector correctness (descendant dedup, `.then()` scoping,
//!     `first()` completion, `[id]`/`[class]` routing, attribute operators
//!     requiring `=`).
//!   * Priority 2 — parser edge cases (form-feed whitespace, comments
//!     containing `>`, empty content without panics, raw-text elements).
//!   * Priority 3 — void and trailing-solidus handling.

use super::helpers::{attr, elements, parse_all, parse_with_saves, texts};
use scah::{Query, Save, parse};

// ===========================================================================
// Priority 1: selector correctness
// ===========================================================================

/// A flat descendant selector returns a physical element once even when it has
/// multiple matching ancestors (`querySelectorAll('div a')` returns 1).
#[test]
fn flat_descendant_single_element_multiple_ancestors() {
    let html = r#"<div><div><a>X</a></div></div>"#;
    let store = parse_all(html, &["div a"]);
    let anchors = elements(&store, "div a");
    assert_eq!(
        anchors.len(),
        1,
        "single <a> with multiple <div> ancestors must appear once"
    );
    assert_eq!(anchors[0].text_content(&store), Some("X"));
}

#[test]
fn flat_descendant_deep_nesting_single_leaf() {
    let html = r#"<div><div><div><span>deep</span></div></div></div>"#;
    let store = parse_all(html, &["div span"]);
    let spans = elements(&store, "div span");
    assert_eq!(spans.len(), 1, "deeply nested single span must appear once");
    assert_eq!(spans[0].text_content(&store), Some("deep"));
}

/// The child combinator is unaffected: each `<a>` is a direct child of *some*
/// `<div>`, so both are returned.
#[test]
fn child_combinator_nested_returns_each_direct_child() {
    let html = r#"<div><a>1</a><div><a>2</a></div></div>"#;
    let store = parse_all(html, &["div > a"]);
    let anchors = elements(&store, "div > a");
    assert_eq!(anchors.len(), 2);
    let t: Vec<_> = anchors.iter().map(|a| a.text_content(&store)).collect();
    assert!(t.contains(&Some("1")));
    assert!(t.contains(&Some("2")));
}

#[test]
fn flat_descendant_multiple_sections_distinct() {
    let html = r#"<section><div><a>A1</a></div></section><section><div><div><a>A2</a></div></div></section>"#;
    let store = parse_all(html, &["section div a"]);
    let anchors = elements(&store, "section div a");
    assert_eq!(anchors.len(), 2);
    let t: Vec<_> = anchors.iter().map(|a| a.text_content(&store)).collect();
    assert!(t.contains(&Some("A1")));
    assert!(t.contains(&Some("A2")));
}

/// A nested `.then()` descendant query dedups the physical child within each
/// parent scope: each `<a>` appears once under the selected `<section>`.
#[test]
fn then_descendant_dedup_within_parent_scope() {
    let html =
        r#"<section><div><a id="one">1</a></div><div><div><a id="two">2</a></div></div></section>"#;
    let queries = &[Query::first("section", Save::none())
        .unwrap()
        .then(|s| Ok([s.all("div a", Save::only_text_content())?]))
        .unwrap()
        .build()];
    let store = parse(html, queries);

    let sections: Vec<_> = store.get("section").unwrap().collect();
    assert_eq!(sections.len(), 1);
    let anchors: Vec<_> = sections[0].get(&store, "div a").unwrap().collect();
    assert_eq!(anchors.len(), 2, "each <a> once even with nested ancestors");
    let t: Vec<_> = anchors.iter().map(|a| a.text_content(&store)).collect();
    assert!(t.contains(&Some("1")));
    assert!(t.contains(&Some("2")));
}

/// The same physical child may appear once under each *distinct* selected
/// parent in a `.then()` query.
#[test]
fn then_same_element_appears_under_each_parent() {
    let html = r#"<div id="outer"><div id="inner"><a>X</a></div></div>"#;
    let queries = &[Query::all("div", Save::none())
        .unwrap()
        .then(|d| Ok([d.all("a", Save::only_text_content())?]))
        .unwrap()
        .build()];
    let store = parse(html, queries);

    let divs: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 2);
    let outer: Vec<_> = divs[0].get(&store, "a").unwrap().collect();
    let inner: Vec<_> = divs[1].get(&store, "a").unwrap().collect();
    assert_eq!(outer.len(), 1, "outer div sees <a> as descendant");
    assert_eq!(inner.len(), 1, "inner div sees <a> as descendant");
    assert_eq!(outer[0].text_content(&store), Some("X"));
    assert_eq!(inner[0].text_content(&store), Some("X"));
}

/// `first()` returns the first match per parent scope and then completes.
#[test]
fn then_first_completes_per_parent_scope() {
    let html = r#"<div><a>1</a><a>2</a></div><div><a>3</a><a>4</a></div>"#;
    let queries = &[Query::all("div", Save::none())
        .unwrap()
        .then(|d| Ok([d.first("> a", Save::only_text_content())?]))
        .unwrap()
        .build()];
    let store = parse(html, queries);

    let divs: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 2);
    for div in &divs {
        let children: Vec<_> = div.get(&store, "> a").unwrap().collect();
        assert_eq!(children.len(), 1, "each div yields exactly its first <a>");
    }
}

/// `[id]` / `[class]` presence selectors match the dedicated fields.
#[test]
fn id_presence_attribute_selector() {
    let html = r#"<div id="a">A</div><div>B</div>"#;
    let store = parse_with_saves(html, &[("div[id]", Save::only_text_content())]);
    let divs = elements(&store, "div[id]");
    assert_eq!(divs.len(), 1);
    assert_eq!(divs[0].text_content(&store), Some("A"));
}

#[test]
fn class_presence_attribute_selector() {
    let html = r#"<div class="x">A</div><div>B</div>"#;
    let store = parse_with_saves(html, &[("div[class]", Save::only_text_content())]);
    let divs = elements(&store, "div[class]");
    assert_eq!(divs.len(), 1);
    assert_eq!(divs[0].text_content(&store), Some("A"));
}

#[test]
fn id_exact_attribute_selector() {
    let html = r#"<div id="x">A</div><div id="y">B</div>"#;
    let store = parse_with_saves(html, &[(r#"div[id="x"]"#, Save::only_text_content())]);
    let divs = elements(&store, r#"div[id="x"]"#);
    assert_eq!(divs.len(), 1);
    assert_eq!(divs[0].text_content(&store), Some("A"));
}

#[test]
fn class_tilde_attribute_selector() {
    let html = r#"<div class="foo bar">A</div><div class="baz">B</div>"#;
    let store = parse_with_saves(html, &[(r#"div[class~="foo"]"#, Save::only_text_content())]);
    let divs = elements(&store, r#"div[class~="foo"]"#);
    assert_eq!(divs.len(), 1);
    assert_eq!(divs[0].text_content(&store), Some("A"));
}

/// Attribute names are case-insensitive in HTML, including routed id/class.
#[test]
fn attribute_name_case_insensitive_routing() {
    let html = r#"<div id="a" data-x="v">A</div>"#;
    let store = parse_with_saves(
        html,
        &[
            ("div[ID]", Save::only_text_content()),
            ("div[DATA-X]", Save::only_text_content()),
        ],
    );
    assert_eq!(elements(&store, "div[ID]").len(), 1);
    assert_eq!(elements(&store, "div[DATA-X]").len(), 1);
}

/// Attribute match operators require `=`; the bare operator is a parse error.
#[test]
fn attribute_match_operators_require_equals() {
    assert!(Query::all("div[class~]", Save::none()).is_err());
    assert!(Query::all("div[id^]", Save::none()).is_err());
    assert!(Query::all("a[href$]", Save::none()).is_err());
    assert!(Query::all("a[href*]", Save::none()).is_err());
    assert!(Query::all("div[class|]", Save::none()).is_err());

    // Presence (no operator) and exact operators remain valid.
    assert!(Query::all("div[id]", Save::none()).is_ok());
    assert!(Query::all("div[class]", Save::none()).is_ok());
    assert!(Query::all(r#"a[href^="http"]"#, Save::none()).is_ok());
    assert!(Query::all(r#"div[class~="foo"]"#, Save::none()).is_ok());
}

// ===========================================================================
// Priority 2: parser edge-case correctness
// ===========================================================================

#[test]
fn empty_elements_do_not_panic() {
    let html = "<div></div><p>   </p><div><!-- comment --></div><div><span></span></div>";
    let queries = &[Query::all("div", Save::only_text_content())
        .unwrap()
        .build()];
    let store = parse(html, queries);
    let divs: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 3);
    for div in &divs {
        assert_eq!(div.text_content(&store), None);
    }
}

#[test]
fn comment_with_gt_does_not_leak_elements() {
    let html = r#"<!-- a > <a href="fake">not-real</a> --><a href="real">real</a>"#;
    let store = parse_all(html, &["a"]);
    let links = elements(&store, "a");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("real"));
}

#[test]
fn abrupt_comment_close_does_not_swallow_document() {
    let html = r#"<!--><div id="real">content</div>"#;
    let store = parse_all(html, &["div#real"]);
    assert_eq!(
        elements(&store, "div#real").len(),
        1,
        "abruptly-closed comment must not swallow the div"
    );
}

#[test]
fn comment_open_only_at_eof_does_not_panic() {
    let html = "<!--";
    let queries = &[Query::all("div", Save::none()).unwrap().build()];
    let store = parse(html, queries);
    assert!(store.get("div").is_none());
}

#[test]
fn form_feed_is_treated_as_whitespace() {
    let html = "<div\x0Cclass=\"real\">text</div>";
    let store = parse_with_saves(html, &[("div.real", Save::only_text_content())]);
    assert_eq!(elements(&store, "div.real").len(), 1);
}

#[test]
fn tab_and_newline_whitespace_in_tags() {
    let html = "<a\n  href=\"x\"\n  class=\"link\">text</a>";
    let store = parse_all(html, &["a.link"]);
    let links = elements(&store, "a.link");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("x"));
}

#[test]
fn double_equals_in_unquoted_attrs_does_not_panic() {
    let html = r#"<div key=a=b>text</div>"#;
    let store = parse_all(html, &["div"]);
    assert_eq!(elements(&store, "div").len(), 1);
}

// ---- raw-text / RCDATA elements: script, style, textarea, title ----

#[test]
fn raw_text_style_is_not_parsed_as_markup() {
    let html = r#"<style>.x::before { content: "<a>"; }</style><a href="real">real</a>"#;
    let store = parse_all(html, &["a"]);
    let links = elements(&store, "a");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("real"));
}

#[test]
fn raw_text_textarea_is_not_parsed_as_markup() {
    let html = r#"<textarea><a>not an element</a></textarea><a>real</a>"#;
    let store = parse_all(html, &["a"]);
    let links = elements(&store, "a");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].text_content(&store), Some("real"));
}

#[test]
fn raw_text_title_is_not_parsed_as_markup() {
    let html = r#"<title><a>not an element</a></title><a>real</a>"#;
    let store = parse_all(html, &["a"]);
    let links = elements(&store, "a");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].text_content(&store), Some("real"));
}

/// Raw-text matching must be ASCII-case-insensitive, including `<SCRIPT>`.
/// (The #23 helper only checked lowercase and regressed uppercase.)
#[test]
fn raw_text_uppercase_script_is_case_insensitive() {
    let html = r#"<SCRIPT>const x = "<a href='bad'>fake</a>";</SCRIPT><a href="real">real</a>"#;
    let store = parse_all(html, &["a"]);
    let links = elements(&store, "a");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("real"));
}

#[test]
fn raw_text_mixed_case_style_is_case_insensitive() {
    let html = r#"<StYlE>.x { content: "<a>"; }</StYlE><a href="real">real</a>"#;
    let store = parse_all(html, &["a"]);
    let links = elements(&store, "a");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("real"));
}

/// A raw-text end tag may carry HTML whitespace before `>` (`</style >`).
/// An exact `</style>` literal check would fail to match and swallow the
/// remainder of the document; the parser must exit raw-text mode and still
/// discover the following `<a>`. (Blocker from the PR #23 review addendum.)
#[test]
fn raw_text_close_tag_allows_whitespace_before_gt() {
    let html = r#"<style>x</style ><a href="ok">ok</a>"#;
    let store = parse_all(html, &["a"]);
    let links = elements(&store, "a");
    assert_eq!(
        links.len(),
        1,
        "`</style >` must terminate raw text so the trailing <a> is found"
    );
    assert_eq!(links[0].attribute(&store, "href"), Some("ok"));
}

/// Raw-text end-tag whitespace tolerance is ASCII-case-insensitive and accepts
/// any HTML whitespace (here an uppercase tag with a newline and spaces).
#[test]
fn raw_text_close_tag_whitespace_is_case_insensitive() {
    let html = "<SCRIPT>var a = \"<a>\";</SCRIPT\n  ><a href=\"ok\">ok</a>";
    let store = parse_all(html, &["a"]);
    let links = elements(&store, "a");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("ok"));
}

/// A near-miss end tag (`</styles>`) must NOT terminate raw text: the name has
/// to be followed by an appropriate terminator, not more name characters.
#[test]
fn raw_text_near_miss_close_tag_does_not_terminate() {
    let html = r#"<style>a { }</styles><a href="fake">x</a></style><a href="real">ok</a>"#;
    let store = parse_all(html, &["a"]);
    let links = elements(&store, "a");
    assert_eq!(
        links.len(),
        1,
        "`</styles>` is not a valid <style> end tag and must stay raw text"
    );
    assert_eq!(links[0].attribute(&store, "href"), Some("real"));
}

// ===========================================================================
// Priority 3: void and trailing-solidus handling
// ===========================================================================

#[test]
fn void_elements_self_close_with_or_without_solidus() {
    let html = r#"<hr/><hr /><input disabled/><input disabled /><div />after</div>"#;
    let store = parse_all(html, &["hr", "input"]);
    assert_eq!(elements(&store, "hr").len(), 2);
    assert_eq!(elements(&store, "input").len(), 2);
}

#[test]
fn trailing_solidus_is_not_exposed_as_attribute() {
    let store = parse_all("<hr />", &["hr"]);
    let hrs = elements(&store, "hr");
    assert_eq!(hrs.len(), 1);
    assert!(
        hrs[0].attribute(&store, "/").is_none(),
        "trailing solidus must not become an attribute"
    );
    assert!(hrs[0].attributes(&store).is_none() || hrs[0].attributes(&store).unwrap().is_empty());
}

#[test]
fn input_with_attribute_and_trailing_solidus_has_clean_attrs() {
    let store = parse_all("<input disabled />", &["input"]);
    let inputs = elements(&store, "input");
    assert_eq!(inputs.len(), 1);
    assert!(inputs[0].attribute(&store, "/").is_none());
    let attrs = inputs[0].attributes(&store).unwrap_or_default();
    let keys: Vec<_> = attrs.iter().map(|a| a.key).collect();
    assert_eq!(keys, vec!["disabled"]);
}

/// A non-void element with a trailing `/` is NOT self-closing in HTML mode:
/// `<div />after</div>` must save `after` as its content, in both debug and
/// release parsing paths.
#[test]
fn non_void_trailing_solidus_is_not_self_closing() {
    let html = "<div />after</div>";
    let queries = &[Query::all("div", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    let divs: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 1);
    assert_eq!(
        divs[0].text_content(&store),
        Some("after"),
        "non-void <div /> must capture trailing content as text"
    );
    assert_eq!(
        divs[0].inner_html.map(str::trim),
        Some("after"),
        "non-void <div /> must capture trailing content as inner_html"
    );
    // The trailing solidus must not leak into attributes.
    assert!(divs[0].attribute(&store, "/").is_none());
}

/// A `/` inside an *unquoted* attribute value is a literal character, never a
/// self-closing marker. `<div data=/foo/>` must keep `data="/foo/"` (both
/// slashes) and, being non-void, still capture the trailing content.
/// (Blocker from the PR #23 review addendum: the value's final `/` was being
/// stripped because it sat immediately before `>`.)
#[test]
fn unquoted_value_trailing_solidus_is_preserved() {
    let html = "<div data=/foo/>after</div>";
    let queries = &[Query::all("div", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    let divs: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 1);
    assert_eq!(
        divs[0].attribute(&store, "data"),
        Some("/foo/"),
        "a solidus inside an unquoted value must be preserved verbatim"
    );
    assert_eq!(
        divs[0].text_content(&store),
        Some("after"),
        "non-void <div> keeps capturing content after an unquoted /value/"
    );
    // The trailing solidus must not leak in as a bare attribute.
    assert!(divs[0].attribute(&store, "/").is_none());
}

/// A leading/embedded `/` in an unquoted value survives too (real URLs).
#[test]
fn unquoted_url_value_with_slashes_is_preserved() {
    let html = r#"<a href=/a/b/c>link</a>"#;
    let store = parse_all(html, &["a"]);
    let links = elements(&store, "a");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("/a/b/c"));
}

#[test]
fn void_elements_do_not_capture_trailing_text_variants() {
    let html = "<div>Line1<br/>Line2<img src='x'/>Line3</div>";
    let store = parse_all(html, &["br", "img", "div"]);
    assert_eq!(elements(&store, "br").len(), 1);
    assert_eq!(elements(&store, "img").len(), 1);
    assert_eq!(texts(&store, "br"), vec![None]);
    assert_eq!(texts(&store, "img"), vec![None]);
    assert_eq!(texts(&store, "div"), vec![Some("Line1 Line2 Line3")]);
    // `src` must be a clean attribute, not merged with a solidus.
    assert_eq!(attr(&store, "img", "src"), vec![Some("x")]);
}
