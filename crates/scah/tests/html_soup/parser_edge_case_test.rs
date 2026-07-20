use super::helpers::{elements, parse_all, parse_with_saves};
use scah::{Query, Save, parse};

#[test]
fn empty_elements_do_not_panic() {
    let html = "<div></div><p>   </p><div><!-- comment --></div><div><span></span></div>";
    let queries = &[Query::all("div", Save::only_text()).unwrap().build()];
    let store = parse(html, queries).unwrap();
    let divs: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 3);
    for div in &divs {
        assert_eq!(div.text(&store), Some(""));
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
    let store = parse(html, queries).unwrap();
    assert!(store.get("div").is_none());
}

#[test]
fn form_feed_is_treated_as_whitespace() {
    let html = "<div\x0Cclass=\"real\">text</div>";
    let store = parse_with_saves(html, &[("div.real", Save::only_text())]);
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

#[test]
fn comment_with_multibyte_char_before_gt_does_not_leak_elements() {
    let html = r#"<!--€><a href="fake">bad</a>--><a href="real">ok</a>"#;
    let store = parse_all(html, &["a"]);
    let links = elements(&store, "a");

    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("real"));
    assert_eq!(links[0].text(&store), Some("ok"));
}
