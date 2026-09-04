use super::helpers::{elements, inner_htmls, parse_all, texts};
#[test]
fn leaf_element_finalizes_at_eof() {
    let html = "<section><a href='x'>Link";
    let store = parse_all(html, &["section", "a"]);

    assert_eq!(elements(&store, "section").len(), 1);
    assert_eq!(elements(&store, "a").len(), 1);
    assert_eq!(texts(&store, "a"), vec![Some("Link")]);
    assert_eq!(inner_htmls(&store, "a"), vec![Some("Link")]);
    assert_eq!(texts(&store, "section"), vec![Some("Link")]);
    assert_eq!(
        inner_htmls(&store, "section"),
        vec![Some("<a href='x'>Link")]
    );
}

#[test]
fn multiple_nested_unclosed_elements_finalize_in_order() {
    let html = "<main><article><h1>Title<p>Body";
    let store = parse_all(html, &["main", "article", "h1", "p"]);

    assert_eq!(elements(&store, "main").len(), 1);
    assert_eq!(elements(&store, "article").len(), 1);
    assert_eq!(elements(&store, "h1").len(), 1);
    assert_eq!(elements(&store, "p").len(), 1);
    assert_eq!(texts(&store, "h1"), vec![Some("Title\nBody")]);
    assert_eq!(texts(&store, "p"), vec![Some("Body")]);
    assert_eq!(texts(&store, "article"), vec![Some("Title\nBody")]);
    assert_eq!(texts(&store, "main"), vec![Some("Title\nBody")]);
}

#[test]
fn consecutive_unclosed_p_tags_split_before_eof() {
    let html = "<div><p>One<p>Two";
    let store = parse_all(html, &["p", "div > p"]);

    assert_eq!(elements(&store, "p").len(), 2);
    assert_eq!(texts(&store, "p"), vec![Some("One"), Some("Two")]);
    assert_eq!(inner_htmls(&store, "p"), vec![Some("One"), Some("Two")]);
    assert_eq!(elements(&store, "div > p").len(), 2);
}
