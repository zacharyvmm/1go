use super::helpers::{attr, elements, inner_htmls, parse_all, parse_with_saves, texts};
use scah::{Query, Save, parse};

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
fn html_tag_and_id_class_attribute_names_are_ascii_case_insensitive() {
    let html = "<DIV ID='hero' CLASS='card featured'>Hello</DIV>";
    let store = parse_with_saves(
        html,
        &[
            ("div#hero.card", Save::only_text_content()),
            ("DIV#hero.featured", Save::only_text_content()),
        ],
    );

    assert_eq!(texts(&store, "div#hero.card"), vec![Some("Hello")]);
    assert_eq!(texts(&store, "DIV#hero.featured"), vec![Some("Hello")]);
}

#[test]
fn mixed_case_void_elements_do_not_capture_following_text() {
    let html = "<DIV>before<Br>middle<IMg src='x'>after</DIV>";
    let store = parse_all(html, &["div", "br", "img"]);

    assert_eq!(texts(&store, "br"), vec![None]);
    assert_eq!(texts(&store, "img"), vec![None]);
    assert_eq!(texts(&store, "div"), vec![Some("before middle after")]);
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

#[test]
fn raw_text_close_tag_whitespace_is_case_insensitive() {
    let html = "<SCRIPT>var a = \"<a>\";</SCRIPT\n  ><a href=\"ok\">ok</a>";
    let store = parse_all(html, &["a"]);
    let links = elements(&store, "a");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("ok"));
}

#[test]
fn raw_text_close_tag_tolerates_trailing_garbage_after_whitespace() {
    let html = r#"<div><style>body { color: red; }</style ignored><a href="tail">tail</a></div>"#;
    let store = parse_all(html, &["style", "div > a", "style > a"]);

    assert_eq!(texts(&store, "style"), vec![Some("body { color: red; }")]);
    assert_eq!(elements(&store, "div > a").len(), 1);
    assert_eq!(attr(&store, "div > a", "href"), vec![Some("tail")]);
    assert!(
        elements(&store, "style > a").is_empty(),
        "the trailing <a> must not remain beneath the closed <style>"
    );
}

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

#[test]
fn raw_text_near_miss_with_trailing_garbage_does_not_terminate() {
    let html = r#"<div><style>x</styles ignored><a href="fake">fake</a></style ignored><a href="real">real</a></div>"#;
    let store = parse_all(html, &["div > a", "style > a"]);

    assert_eq!(attr(&store, "div > a", "href"), vec![Some("real")]);
    assert!(
        elements(&store, "style > a").is_empty(),
        "the near-miss </styles ignored> must stay literal raw text"
    );
}

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

#[test]
fn non_void_trailing_solidus_is_not_self_closing() {
    let html = "<div />after</div>";
    let queries = &[Query::all("div", Save::all()).unwrap().build()];
    let store = parse(html, queries).unwrap();
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
    assert!(divs[0].attribute(&store, "/").is_none());
}

#[test]
fn unquoted_value_trailing_solidus_is_preserved() {
    let html = "<div data=/foo/>after</div>";
    let queries = &[Query::all("div", Save::all()).unwrap().build()];
    let store = parse(html, queries).unwrap();
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
    assert!(divs[0].attribute(&store, "/").is_none());
}

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
    assert_eq!(attr(&store, "img", "src"), vec![Some("x")]);
}
