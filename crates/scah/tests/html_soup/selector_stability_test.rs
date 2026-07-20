use super::helpers::{attr, elements, parse_all, parse_with_saves, texts};
use scah::{Query, Save, parse};

#[test]
fn descendant_and_child_selectors_survive_implied_close() {
    let html = "<section><p><a href='x'>One<div><a href='y'>Two</a></div></section>";
    let store = parse_all(html, &["section a", "section > p > a", "section > div a"]);

    assert_eq!(
        attr(&store, "section a", "href"),
        vec![Some("x"), Some("y")]
    );
    assert_eq!(attr(&store, "section > p > a", "href"), vec![Some("x")]);
    assert_eq!(attr(&store, "section > div a", "href"), vec![Some("y")]);
}

#[test]
fn misnest_recovery_does_not_duplicate_nested_matches() {
    let html = "<div><span><a href='x'>X</div>";
    let store = parse_all(html, &["a", "div a", "span a"]);

    assert_eq!(elements(&store, "a").len(), 1);
    assert_eq!(elements(&store, "div a").len(), 1);
    assert_eq!(elements(&store, "span a").len(), 1);
    assert_eq!(texts(&store, "a"), vec![Some("X")]);
    assert_eq!(texts(&store, "div a"), vec![Some("X")]);
    assert_eq!(texts(&store, "span a"), vec![Some("X")]);
}

#[test]
fn first_selector_returns_first_recovered_match_only() {
    let html = "<ul><li>One<li>Two<li>Three</ul>";
    let queries = [
        Query::first("ul > li", Save::all()).unwrap().build(),
        Query::all("li", Save::all()).unwrap().build(),
    ];
    let store = parse(html, &queries).unwrap();

    assert_eq!(elements(&store, "li").len(), 3);
    assert_eq!(
        texts(&store, "li"),
        vec![Some("One"), Some("Two"), Some("Three")]
    );
    assert_eq!(elements(&store, "ul > li").len(), 1);
    assert_eq!(texts(&store, "ul > li"), vec![Some("One")]);
}

/// A flat descendant selector returns a physical element once even when it has
/// multiple matching ancestors.
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

#[test]
fn child_combinator_nested_returns_each_direct_child() {
    let html = r#"<div><a>1</a><div><a>2</a></div></div>"#;
    let store = parse_all(html, &["div > a"]);
    let anchors = elements(&store, "div > a");
    assert_eq!(anchors.len(), 2);
    let text: Vec<_> = anchors.iter().map(|a| a.text_content(&store)).collect();
    assert!(text.contains(&Some("1")));
    assert!(text.contains(&Some("2")));
}

#[test]
fn flat_descendant_multiple_sections_distinct() {
    let html = r#"<section><div><a>A1</a></div></section><section><div><div><a>A2</a></div></div></section>"#;
    let store = parse_all(html, &["section div a"]);
    let anchors = elements(&store, "section div a");
    assert_eq!(anchors.len(), 2);
    let text: Vec<_> = anchors.iter().map(|a| a.text_content(&store)).collect();
    assert!(text.contains(&Some("A1")));
    assert!(text.contains(&Some("A2")));
}

#[test]
fn then_descendant_dedup_within_parent_scope() {
    let html =
        r#"<section><div><a id="one">1</a></div><div><div><a id="two">2</a></div></div></section>"#;
    let queries = &[Query::first("section", Save::none())
        .unwrap()
        .then(|s| Ok([s.all("div a", Save::only_text_content())?]))
        .unwrap()
        .build()];
    let store = parse(html, queries).unwrap();

    let sections: Vec<_> = store.get("section").unwrap().collect();
    assert_eq!(sections.len(), 1);
    let anchors: Vec<_> = sections[0].get(&store, "div a").unwrap().collect();
    assert_eq!(anchors.len(), 2, "each <a> once even with nested ancestors");
    let text: Vec<_> = anchors.iter().map(|a| a.text_content(&store)).collect();
    assert!(text.contains(&Some("1")));
    assert!(text.contains(&Some("2")));
}

#[test]
fn then_same_element_appears_under_each_parent() {
    let html = r#"<div id="outer"><div id="inner"><a>X</a></div></div>"#;
    let queries = &[Query::all("div", Save::none())
        .unwrap()
        .then(|d| Ok([d.all("a", Save::only_text_content())?]))
        .unwrap()
        .build()];
    let store = parse(html, queries).unwrap();

    let divs: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 2);
    let outer: Vec<_> = divs[0].get(&store, "a").unwrap().collect();
    let inner: Vec<_> = divs[1].get(&store, "a").unwrap().collect();
    assert_eq!(outer.len(), 1, "outer div sees <a> as descendant");
    assert_eq!(inner.len(), 1, "inner div sees <a> as descendant");
    assert_eq!(outer[0].text_content(&store), Some("X"));
    assert_eq!(inner[0].text_content(&store), Some("X"));
}

#[test]
fn then_first_completes_per_parent_scope() {
    let html = r#"<div><a>1</a><a>2</a></div><div><a>3</a><a>4</a></div>"#;
    let queries = &[Query::all("div", Save::none())
        .unwrap()
        .then(|d| Ok([d.first("> a", Save::only_text_content())?]))
        .unwrap()
        .build()];
    let store = parse(html, queries).unwrap();

    let divs: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 2);
    for div in &divs {
        let children: Vec<_> = div.get(&store, "> a").unwrap().collect();
        assert_eq!(children.len(), 1, "each div yields exactly its first <a>");
    }
}

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

#[test]
fn attribute_match_operators_require_equals() {
    assert!(Query::all("div[class~]", Save::none()).is_err());
    assert!(Query::all("div[id^]", Save::none()).is_err());
    assert!(Query::all("a[href$]", Save::none()).is_err());
    assert!(Query::all("a[href*]", Save::none()).is_err());
    assert!(Query::all("div[class|]", Save::none()).is_err());

    assert!(Query::all("div[id]", Save::none()).is_ok());
    assert!(Query::all("div[class]", Save::none()).is_ok());
    assert!(Query::all(r#"a[href^="http"]"#, Save::none()).is_ok());
    assert!(Query::all(r#"div[class~="foo"]"#, Save::none()).is_ok());
}

#[test]
fn cursor_canonicalization_a_main_div_p_single_result() {
    let html = r#"<main><div><div><p>Hello World</p></div></div></main>"#;
    let store = parse_all(html, &["main > div p"]);
    assert_eq!(elements(&store, "main > div p").len(), 1);
}

#[test]
fn cursor_canonicalization_b_sibling_direct_children() {
    let html = r#"<main><div><p>A</p></div><div><p>B</p></div></main>"#;
    let store = parse_all(html, &["main > div p"]);
    assert_eq!(elements(&store, "main > div p").len(), 2);
}

#[test]
fn cursor_canonicalization_c_overlapping_nested_prefixes() {
    let html = r#"<main><div><main><div><p>Hello</p></div></main></div></main>"#;
    let store = parse_all(html, &["main > div p"]);
    assert_eq!(elements(&store, "main > div p").len(), 1);
}

#[test]
fn cursor_canonicalization_d_repeated_child_prefix_overlap() {
    let html = r#"<div><div><div><p>Hello</p></div></div></div>"#;
    let store = parse_all(html, &["div > div p"]);
    assert_eq!(elements(&store, "div > div p").len(), 1);
}

#[test]
fn cursor_canonicalization_e_child_anchors_not_over_pruned() {
    let html = r#"<div><p>Outer</p><div><p>Inner</p></div></div>"#;
    let store = parse_all(html, &["div > p"]);
    assert_eq!(elements(&store, "div > p").len(), 2);
}

#[test]
fn cursor_canonicalization_f_terminal_all_nested_divs() {
    let html = r#"<div><div><div></div></div></div>"#;
    let store = parse_all(html, &["div"]);
    assert_eq!(elements(&store, "div").len(), 3);
}

#[test]
fn cursor_canonicalization_g_then_scopes_distinct_parents() {
    let html = r#"<div><div><div><p>Hello</p></div></div></div>"#;
    let queries = &[Query::all("div", Save::all())
        .unwrap()
        .then(|div| Ok([div.all("p", Save::all())?]))
        .unwrap()
        .build()];
    let store = parse(html, queries).unwrap();

    let divs: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 3);
    let parents_with_p = divs
        .iter()
        .filter(|div| div.get(&store, "p").unwrap().next().is_some())
        .count();
    assert_eq!(
        parents_with_p, 3,
        "Each div scope must keep its own p (not globally deduped)"
    );
}

#[test]
fn cursor_canonicalization_h_first_flat_and_then() {
    let html = r#"<div><p>A</p><p>B</p></div><div><p>C</p><p>D</p></div>"#;
    let flat_first = Query::first("div p", Save::all()).unwrap().build();
    let queries = [flat_first];
    let store = parse(html, &queries).unwrap();
    assert_eq!(elements(&store, "div p").len(), 1);

    let html2 = r#"<div><p>A</p><p>B</p></div>"#;
    let queries2 = &[Query::all("div", Save::none())
        .unwrap()
        .then(|div| Ok([div.first("p", Save::all())?]))
        .unwrap()
        .build()];
    let store2 = parse(html2, queries2).unwrap();
    let divs: Vec<_> = store2.get("div").unwrap().collect();
    assert_eq!(divs[0].get(&store2, "p").unwrap().count(), 1);
}

#[test]
fn cursor_canonicalization_i_implicit_close_and_self_closing() {
    let html = "<ul><li><div><div><p>X</p></div></div><li>Y</ul>";
    let store = parse_all(html, &["div > div p"]);
    assert_eq!(elements(&store, "div > div p").len(), 1);

    let html2 = "<div><div><br /><p>Y</p></div></div>";
    let store2 = parse_all(html2, &["div > div p"]);
    assert_eq!(elements(&store2, "div > div p").len(), 1);
}
