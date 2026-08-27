use super::helpers::{elements, parse_all};
use scah::{Query, Save, parse};

fn ids<'a>(store: &'a scah::Store<'a, 'a>, selector: &str) -> Vec<Option<&'a str>> {
    elements(store, selector)
        .into_iter()
        .map(|element| element.id)
        .collect()
}

#[test]
fn adjacent_sibling_basic_match() {
    let store = parse_all(
        r#"
        <main>
          <h1></h1>
          <p id="hit"></p>
        </main>
        "#,
        &["h1 + p"],
    );
    assert_eq!(ids(&store, "h1 + p"), [Some("hit")]);
}

#[test]
fn adjacent_sibling_text_does_not_consume() {
    let store = parse_all(
        r#"
        <main>
          <h1></h1>
          text
          <p id="hit"></p>
        </main>
        "#,
        &["h1 + p"],
    );
    assert_eq!(ids(&store, "h1 + p"), [Some("hit")]);
}

#[test]
fn adjacent_sibling_intervening_element_consumes() {
    let store = parse_all(
        r#"
        <main>
          <h1></h1>
          <div></div>
          <p id="miss"></p>
        </main>
        "#,
        &["h1 + p"],
    );
    assert!(elements(&store, "h1 + p").is_empty());
}

#[test]
fn adjacent_sibling_nested_descendant_is_not_sibling() {
    let store = parse_all(
        r#"
        <main>
          <h1>
            <p id="nested-miss"></p>
          </h1>
          <p id="hit"></p>
        </main>
        "#,
        &["h1 + p"],
    );
    assert_eq!(ids(&store, "h1 + p"), [Some("hit")]);
}

#[test]
fn adjacent_sibling_parent_boundary() {
    let store = parse_all(
        r#"
        <main>
          <h1></h1>
        </main>
        <p id="outside-miss"></p>
        "#,
        &["h1 + p"],
    );
    assert!(elements(&store, "h1 + p").is_empty());
}

#[test]
fn adjacent_sibling_chained_right_hand_selector() {
    let store = parse_all(
        r#"
        <main>
          <h1></h1>
          <p><span id="hit"></span></p>
        </main>
        "#,
        &["h1 + p > span"],
    );
    assert_eq!(ids(&store, "h1 + p > span"), [Some("hit")]);
}

#[test]
fn adjacent_sibling_repeated_preconditions_match_once() {
    let store = parse_all(
        r#"
        <main>
          <div></div>
          <div></div>
          <p id="hit"></p>
        </main>
        "#,
        &["div + p"],
    );
    assert_eq!(ids(&store, "div + p"), [Some("hit")]);
}

#[test]
fn adjacent_sibling_void_source_element() {
    let store = parse_all(
        r#"
        <main>
          <br>
          <p id="hit"></p>
        </main>
        "#,
        &["br + p"],
    );
    assert_eq!(ids(&store, "br + p"), [Some("hit")]);
}

#[test]
fn adjacent_sibling_implied_close() {
    let store = parse_all(
        r#"
        <ul>
          <li id="first">one
          <li id="second">two
        </ul>
        "#,
        &["li + li"],
    );
    assert_eq!(ids(&store, "li + li"), [Some("second")]);
}

#[test]
fn subsequent_sibling_all_later_matches() {
    let store = parse_all(
        r#"
        <main>
          <h1></h1>
          <div></div>
          <p id="a"></p>
          <section></section>
          <p id="b"></p>
        </main>
        "#,
        &["h1 ~ p"],
    );
    assert_eq!(ids(&store, "h1 ~ p"), [Some("a"), Some("b")]);
}

#[test]
fn subsequent_sibling_nested_and_outside_do_not_match() {
    let store = parse_all(
        r#"
        <main>
          <div></div>
          <section>
            <p id="nested-miss"></p>
          </section>
          <p id="hit"></p>
        </main>
        <p id="outside-miss"></p>
        "#,
        &["main > div ~ p"],
    );
    assert_eq!(ids(&store, "main > div ~ p"), [Some("hit")]);
}

#[test]
fn subsequent_sibling_chained_selector() {
    let store = parse_all(
        r#"
        <main>
          <div></div>
          <p><span id="a"></span></p>
          <section><span id="nested-miss"></span></section>
          <p><span id="b"></span></p>
        </main>
        "#,
        &["main > div ~ p > span"],
    );
    assert_eq!(ids(&store, "main > div ~ p > span"), [Some("a"), Some("b")]);
}

#[test]
fn subsequent_sibling_multiple_preconditions_do_not_duplicate() {
    let store = parse_all(
        r#"
        <main>
          <div></div>
          <div></div>
          <p><span id="once"></span></p>
        </main>
        "#,
        &["main > div ~ p > span"],
    );
    assert_eq!(ids(&store, "main > div ~ p > span"), [Some("once")]);
}

#[test]
fn subsequent_sibling_void_source_element() {
    let store = parse_all(
        r#"
        <main>
          <br>
          <div></div>
          <p id="hit"></p>
        </main>
        "#,
        &["br ~ p"],
    );
    assert_eq!(ids(&store, "br ~ p"), [Some("hit")]);
}

#[test]
fn subsequent_sibling_first_vs_all_selection() {
    let html = r#"
        <main>
          <h1></h1>
          <p id="a"></p>
          <p id="b"></p>
          <p id="c"></p>
        </main>
    "#;

    let first = [Query::first("h1 ~ p", Save::none()).unwrap().build()];
    let first_store = parse(html, &first).expect("parse succeeds");
    assert_eq!(ids(&first_store, "h1 ~ p"), [Some("a")]);

    let all = [Query::all("h1 ~ p", Save::none()).unwrap().build()];
    let all_store = parse(html, &all).expect("parse succeeds");
    assert_eq!(ids(&all_store, "h1 ~ p"), [Some("a"), Some("b"), Some("c")]);
}

#[test]
fn subsequent_sibling_structured_then_scopes_independently() {
    let html = r#"
        <article id="a">
          <div></div>
          <p id="a1"></p>
          <p id="a2"></p>
        </article>
        <article id="b">
          <div></div>
          <p id="b1"></p>
          <p id="b2"></p>
        </article>
    "#;

    let query = Query::all("article", Save::all())
        .unwrap()
        .then(|article| Ok([article.all("div ~ p", Save::all())?]))
        .unwrap()
        .build();
    let queries = [query];
    let store = parse(html, &queries).expect("parse succeeds");

    let articles: Vec<_> = store.get("article").unwrap().collect();
    assert_eq!(articles.len(), 2);

    let article_a = articles.iter().find(|a| a.id == Some("a")).unwrap();
    let article_b = articles.iter().find(|a| a.id == Some("b")).unwrap();

    let a_ps: Vec<_> = article_a.get(&store, "div ~ p").unwrap().collect();
    assert_eq!(a_ps.len(), 2);
    assert_eq!(a_ps[0].id, Some("a1"));
    assert_eq!(a_ps[1].id, Some("a2"));

    let b_ps: Vec<_> = article_b.get(&store, "div ~ p").unwrap().collect();
    assert_eq!(b_ps.len(), 2);
    assert_eq!(b_ps[0].id, Some("b1"));
    assert_eq!(b_ps[1].id, Some("b2"));
}

#[test]
fn sibling_callback_discarded_when_parent_closes_in_same_batch() {
    // Closing </main> pops section, then div, then main. The section's sibling
    // callback must not activate into later content outside main.
    let store = parse_all(
        r#"
        <main>
          <div>
            <section>
        </main>
        <p id="outside-miss"></p>
        "#,
        &["section + p", "div ~ p"],
    );
    assert!(elements(&store, "section + p").is_empty());
    assert!(elements(&store, "div ~ p").is_empty());
}

#[test]
fn sibling_callback_not_activated_at_eof() {
    let store = parse_all(
        r#"
        <main>
          <h1></h1>
        "#,
        &["h1 + p", "h1 ~ p"],
    );
    assert!(elements(&store, "h1 + p").is_empty());
    assert!(elements(&store, "h1 ~ p").is_empty());
}
