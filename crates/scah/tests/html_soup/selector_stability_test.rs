use super::helpers::{attr, elements, parse_all, texts};
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
