use super::helpers::{attr, elements, inner_htmls, parse_all, parse_with_saves, texts};
use scah::Save;

#[test]
fn void_elements_do_not_capture_following_text() {
    let html = "<div>Line1<br>Line2<img src='x'>Line3</div>";
    let store = parse_all(html, &["br", "img", "div", "div > br", "div > img"]);

    assert_eq!(elements(&store, "br").len(), 1);
    assert_eq!(elements(&store, "img").len(), 1);
    assert_eq!(texts(&store, "br"), vec![None]);
    assert_eq!(texts(&store, "img"), vec![None]);
    assert_eq!(inner_htmls(&store, "img"), vec![None]);
    assert_eq!(texts(&store, "div"), vec![Some("Line1 Line2 Line3")]);
    assert_eq!(elements(&store, "div > br").len(), 1);
    assert_eq!(elements(&store, "div > img").len(), 1);
}

#[test]
fn void_syntax_and_plain_void_behavior_match() {
    let html = "<form><input id='a'/><input id='b'></form>";
    let store = parse_all(html, &["input", "form > input"]);

    assert_eq!(elements(&store, "input").len(), 2);
    assert_eq!(elements(&store, "form > input").len(), 2);
    assert_eq!(attr(&store, "input", "id"), vec![None, None]);
    assert_eq!(texts(&store, "input"), vec![None, None]);
    assert_eq!(inner_htmls(&store, "input"), vec![None, None]);
}

#[test]
fn script_contents_do_not_emit_false_selector_matches() {
    let html = "<div></div><script>const x = \"<div><a href='x'>bad</a></div>\";</script><a href='ok'>good</a>";
    let store = parse_with_saves(
        html,
        &[
            ("div", Save::none()),
            ("a", Save::all()),
            ("script", Save::none()),
        ],
    );

    assert_eq!(elements(&store, "div").len(), 1);
    assert_eq!(elements(&store, "a").len(), 1);
    assert_eq!(elements(&store, "script").len(), 1);
    assert_eq!(attr(&store, "a", "href"), vec![Some("ok")]);
    assert_eq!(texts(&store, "a"), vec![Some("good")]);
}

#[test]
fn quoted_attributes_long_text_and_script_content_parse_correctly() {
    let long_text = "x".repeat(16 * 1024);
    let html = format!(
        "<main><a href='https://example.com?q=<tag>&v=\"quoted\"'>{long_text}</a>\
         <script>const fake = \"<a href='bad'>bad</a>\";</script>\
         <a href='tail'>tail</a></main>"
    );
    let store = parse_with_saves(
        &html,
        &[("main > a", Save::all()), ("script", Save::none())],
    );

    assert_eq!(
        attr(&store, "main > a", "href"),
        vec![
            Some("https://example.com?q=<tag>&v=\"quoted\""),
            Some("tail")
        ]
    );
    assert_eq!(
        texts(&store, "main > a"),
        vec![Some(long_text.as_str()), Some("tail")]
    );
    assert_eq!(elements(&store, "script").len(), 1);
}
