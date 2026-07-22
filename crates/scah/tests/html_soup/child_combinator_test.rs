use super::helpers::{elements, parse_all};
use scah::{Query, Save, parse};

fn ids<'a>(store: &'a scah::Store<'a, 'a>, selector: &str) -> Vec<Option<&'a str>> {
    elements(store, selector)
        .into_iter()
        .map(|element| element.id)
        .collect()
}

#[test]
fn child_selector_rejects_deep_descendants() {
    let store = parse_all(
        r#"
        <main>
          <article>
            <section>
              <p id="deep"></p>
            </section>
          </article>
        </main>
        "#,
        &["main > article > p"],
    );
    assert!(elements(&store, "main > article > p").is_empty());
}

#[test]
fn child_selector_matches_only_direct_child() {
    let store = parse_all(
        r#"
        <main>
          <article>
            <p id="direct"></p>
            <section>
              <p id="deep"></p>
            </section>
          </article>
        </main>
        "#,
        &["main > article > p"],
    );
    assert_eq!(ids(&store, "main > article > p"), [Some("direct")]);
}

#[test]
fn child_selector_repeated_direct_siblings_remain_active() {
    let store = parse_all(
        r#"
        <main>
          <p id="one"></p>
          <p id="two"></p>
          <section>
            <p id="nested"></p>
          </section>
          <p id="three"></p>
        </main>
        "#,
        &["main > p"],
    );
    assert_eq!(
        ids(&store, "main > p"),
        [Some("one"), Some("two"), Some("three")]
    );
}

#[test]
fn child_section_created_through_then_matches_only_direct_anchors() {
    let html = r#"
        <main id="m1">
          <a id="direct-1"></a>
          <section>
            <a id="nested-1"></a>
          </section>
          <a id="direct-2"></a>
        </main>
        <main id="m2">
          <a id="direct-3"></a>
          <div>
            <a id="nested-2"></a>
          </div>
        </main>
    "#;

    let query = Query::all("main", Save::all())
        .unwrap()
        .then(|main| Ok([main.all("> a", Save::all())?]))
        .unwrap()
        .build();
    let queries = [query];
    let store = parse(html, &queries).expect("parse succeeds");

    let mains: Vec<_> = store.get("main").unwrap().collect();
    assert_eq!(mains.len(), 2);

    let m1 = mains.iter().find(|m| m.id == Some("m1")).unwrap();
    let m2 = mains.iter().find(|m| m.id == Some("m2")).unwrap();

    let m1_anchors: Vec<_> = m1.get(&store, "> a").unwrap().collect();
    assert_eq!(
        m1_anchors.iter().map(|a| a.id).collect::<Vec<_>>(),
        [Some("direct-1"), Some("direct-2")]
    );

    let m2_anchors: Vec<_> = m2.get(&store, "> a").unwrap().collect();
    assert_eq!(
        m2_anchors.iter().map(|a| a.id).collect::<Vec<_>>(),
        [Some("direct-3")]
    );
}

#[test]
fn sibling_to_child_chaining_matches_only_direct_span() {
    let html = r#"
        <main>
          <div></div>
          <p>
            <span id="direct"></span>
            <em>
              <span id="deep"></span>
            </em>
          </p>
        </main>
    "#;

    let adjacent = parse_all(html, &["div + p > span"]);
    assert_eq!(ids(&adjacent, "div + p > span"), [Some("direct")]);

    let subsequent = parse_all(html, &["div ~ p > span"]);
    assert_eq!(ids(&subsequent, "div ~ p > span"), [Some("direct")]);
}

#[test]
fn child_to_sibling_chaining_stays_at_parent_scope() {
    let store = parse_all(
        r#"
        <main>
          <div></div>
          <p id="adjacent"></p>
          <section>
            <div></div>
            <p id="nested"></p>
          </section>
        </main>
        "#,
        &["main > div + p"],
    );
    assert_eq!(ids(&store, "main > div + p"), [Some("adjacent")]);
}

#[test]
fn void_child_elements_match_only_direct() {
    let store = parse_all(
        r#"
        <main>
          <img id="direct">
          <section>
            <img id="nested">
          </section>
        </main>
        "#,
        &["main > img"],
    );
    assert_eq!(ids(&store, "main > img"), [Some("direct")]);
}

#[test]
fn void_child_participates_in_sibling_chaining() {
    let store = parse_all(
        r#"
        <main>
          <br>
          <p id="hit"></p>
          <section>
            <br>
            <p id="nested-miss"></p>
          </section>
        </main>
        "#,
        &["main > br + p"],
    );
    assert_eq!(ids(&store, "main > br + p"), [Some("hit")]);
}

#[test]
fn first_child_selection_retires_after_direct_winner() {
    let html = r#"
        <main>
          <section>
            <p id="nested"></p>
          </section>
          <p id="first-direct"></p>
          <p id="second-direct"></p>
        </main>
    "#;

    let query = [Query::first("main > p", Save::none()).unwrap().build()];
    let store = parse(html, &query).expect("parse succeeds");
    assert_eq!(ids(&store, "main > p"), [Some("first-direct")]);
}

#[test]
fn mixed_child_and_descendant_sections_do_not_leak_depth_rules() {
    let html = r#"
        <main>
          <p id="direct"></p>
          <section>
            <p id="nested"></p>
          </section>
        </main>
    "#;

    let query = Query::all("main", Save::all())
        .unwrap()
        .then(|main| {
            Ok([
                main.all("> p", Save::all())?,
                main.all("section p", Save::all())?,
            ])
        })
        .unwrap()
        .build();
    let queries = [query];
    let store = parse(html, &queries).expect("parse succeeds");

    let mains: Vec<_> = store.get("main").unwrap().collect();
    assert_eq!(mains.len(), 1);
    let main = &mains[0];

    let direct: Vec<_> = main.get(&store, "> p").unwrap().collect();
    assert_eq!(
        direct.iter().map(|p| p.id).collect::<Vec<_>>(),
        [Some("direct")]
    );

    let nested: Vec<_> = main.get(&store, "section p").unwrap().collect();
    assert_eq!(
        nested.iter().map(|p| p.id).collect::<Vec<_>>(),
        [Some("nested")]
    );
}
