use super::helpers::{elements, inner_htmls, parse_all, texts};

#[test]
fn implied_p_close_splits_content_correctly() {
    let html = "<div><p>Hello<div>World</div></div>";
    let store = parse_all(html, &["p", "div", "div > p", "div > div"]);

    assert_eq!(elements(&store, "p").len(), 1);
    assert_eq!(texts(&store, "p"), vec![Some("Hello")]);
    assert_eq!(inner_htmls(&store, "p"), vec![Some("Hello")]);
    // With the end-guard fix, end=true cursors no longer match, eliminating
    // a spurious duplicate div match from the root cursor at inner <div>.
    assert_eq!(
        texts(&store, "div"),
        vec![Some("Hello World"), Some("World")]
    );
    assert_eq!(elements(&store, "div > p").len(), 1);
    assert_eq!(elements(&store, "div > div").len(), 1);
}

#[test]
fn implied_li_close_creates_distinct_siblings() {
    let html = "<ul><li>One<li>Two<li>Three</ul>";
    let store = parse_all(html, &["li", "ul > li"]);

    assert_eq!(elements(&store, "li").len(), 3);
    assert_eq!(
        texts(&store, "li"),
        vec![Some("One"), Some("Two"), Some("Three")]
    );
    assert_eq!(
        inner_htmls(&store, "li"),
        vec![Some("One"), Some("Two"), Some("Three")]
    );
    assert_eq!(elements(&store, "ul > li").len(), 3);
}

#[test]
fn implied_dt_dd_close_creates_expected_pairs() {
    let html = "<dl><dt>A<dd>B<dt>C<dd>D</dl>";
    let store = parse_all(html, &["dt", "dd", "dl > dt", "dl > dd"]);

    assert_eq!(texts(&store, "dt"), vec![Some("A"), Some("C")]);
    assert_eq!(texts(&store, "dd"), vec![Some("B"), Some("D")]);
    assert_eq!(elements(&store, "dl > dt").len(), 2);
    assert_eq!(elements(&store, "dl > dd").len(), 2);
}

#[test]
fn implied_option_close_splits_options() {
    let html = "<select><option>One<option>Two<option>Three</select>";
    let store = parse_all(html, &["option", "select > option"]);

    assert_eq!(elements(&store, "option").len(), 3);
    assert_eq!(
        texts(&store, "option"),
        vec![Some("One"), Some("Two"), Some("Three")]
    );
    assert_eq!(
        inner_htmls(&store, "option"),
        vec![Some("One"), Some("Two"), Some("Three")]
    );
    assert_eq!(elements(&store, "select > option").len(), 3);
}
