use std::ops::Deref;

#[cfg(debug_assertions)]
use scah::debug;
use scah::{Attribute, Query, QuerySpec, Save, Store, parse, query};
const HTML: &str = r#"
<!DOCTYPE html>
<html>
<head>
    <title>Test Page</title>
    <style>
        .red-background {
            background-color: #ffdddd;
        }
    </style>
</head>
<body>
    <main class="red-background">
        <section id="id">
            <!-- These 3 links will be selected by the selector -->
            <a href="link1">Link 1</a>
            <a href="link2">Link 2</a>
            <a href="link3">Link 3</a>

            <!-- These elements won't be selected -->
            <div>
                <a href="not-selected">Not selected (nested in div)</a>
            </div>
            <span>No link here</span>
        </section>

        <!-- These elements won't be selected -->
        <section>
            <a href="wrong-section">Not selected (wrong section)</a>
        </section>
        <a href="direct-link">Not selected (direct child of main)</a>
    </main>

    <!-- These elements won't be selected -->
    <main>
        <section id="id" class="third-section">
            <a href="wrong-main">Not selected (main has no red-background class)</a>
        </section>
    </main>
</body>
</html>
"#;

#[test]
#[cfg(debug_assertions)]
fn trace_records_open_and_save_events() {
    let html = "<main><section><a href='/x'>x</a></section></main>";
    let query = Query::all("main section a", Save::all()).unwrap().build();
    let queries = [query];

    let store = parse(html, &queries).unwrap();

    assert!(!store.trace.is_empty());
    assert!(
        store
            .trace
            .events()
            .iter()
            .any(|event| { matches!(event, debug::TraceEvent::OpenTag { tag: "main", .. }) })
    );
    assert!(
        store
            .trace
            .events()
            .iter()
            .any(|event| { matches!(event, debug::TraceEvent::ElementSaved { element: "a", .. }) })
    );
}

#[test]
#[cfg(debug_assertions)]
fn trace_records_implied_li_close() {
    let html = "<ul><li>One<li>Two</ul>";
    let query = Query::all("li", Save::all()).unwrap().build();
    let queries = [query];

    let store = parse(html, &queries).unwrap();

    assert!(store.trace.events().iter().any(|event| {
        matches!(
            event,
            debug::TraceEvent::ImpliedClose {
                tag: "li",
                reason: debug::ImpliedCloseReason::OpenTagRule,
                ..
            }
        )
    }));
}

#[test]
#[cfg(debug_assertions)]
fn trace_records_first_query_early_exit() {
    let html = "<div><a>one</a><a>two</a></div>";
    let query = Query::first("a", Save::all()).unwrap().build();
    let queries = [query];

    let store = parse(html, &queries).unwrap();

    assert!(
        store
            .trace
            .events()
            .iter()
            .any(|event| matches!(event, debug::TraceEvent::EarlyExit { .. }))
    );
}

#[test]
#[cfg(debug_assertions)]
fn trace_records_transition_rejections() {
    let html = "<main><span>no</span><a>yes</a></main>";
    let query = Query::all("main > a", Save::all()).unwrap().build();
    let queries = [query];

    let store = parse(html, &queries).unwrap();

    assert!(store.trace.events().iter().any(|event| {
        matches!(
            event,
            debug::TraceEvent::TransitionRejected {
                element: "span",
                reason: debug::TransitionRejectReason::PredicateFailed,
                ..
            }
        )
    }));
}

#[test]
fn test_html_page() {
    let selection_tree = Query::all("main > section#id", Save::all()).unwrap();

    let queries = &[selection_tree.build()];
    let store = parse(HTML, queries).unwrap();
    let list = store.get("main > section#id").unwrap().collect::<Vec<_>>();

    assert_eq!(list.len(), 2);

    let last = list.last().unwrap();

    assert!(last.inner_html.is_some());
    assert_eq!(
        last.inner_html.unwrap().trim(),
        r#"<a href="wrong-main">Not selected (main has no red-background class)</a>"#
    );

    assert!(last.text(&store).is_some());
    assert_eq!(
        last.text(&store).unwrap(),
        r#"Not selected (main has no red-background class)"#
    );

    let first = list.first().unwrap();
    assert_eq!(
        first.inner_html.unwrap().trim(),
        r#"<!-- These 3 links will be selected by the selector -->
            <a href="link1">Link 1</a>
            <a href="link2">Link 2</a>
            <a href="link3">Link 3</a>

            <!-- These elements won't be selected -->
            <div>
                <a href="not-selected">Not selected (nested in div)</a>
            </div>
            <span>No link here</span>"#
    );

    assert_eq!(
        first.text(&store).unwrap(),
        "Link 1 Link 2 Link 3\n Not selected (nested in div)\nNo link here"
    );
}

#[test]
fn test_html_page_all_anchor_tag_selection() {
    let queries = &[Query::all("a", Save::all()).unwrap().build()];
    let store = parse(HTML, queries).unwrap();
    println!("Store: {:#?}", store);

    let list = store.get("a").unwrap().collect::<Vec<_>>();

    assert_eq!(list.len(), 7);
    println!("List: {:#?}", list);
}

#[test]
fn test_html_page_first_anchor_tag_selection() {
    let queries = &[Query::first("a", Save::all()).unwrap().build()];
    let store = parse(HTML, queries).unwrap();
    let mut children = store.get("a").unwrap();

    let a = children.next().unwrap();
    assert_eq!(
        store.attributes.deref().clone(),
        vec![Attribute {
            key: "href",
            value: Some("link1")
        }]
    );
    assert_eq!(a.name, "a");
    assert_eq!(
        a.attributes(&store).unwrap(),
        &[Attribute {
            key: "href",
            value: Some("link1")
        }]
    );
    assert_eq!(a.attribute(&store, "href"), Some("link1"));
    assert_eq!(a.text(&store).unwrap(), "Link 1");
}

#[test]
fn test_html_page_all_anchor_tag_starting_with_link_selection() {
    let queries = &[Query::all("a[href^=link]", Save::all()).unwrap().build()];
    let store = parse(HTML, queries).unwrap();
    let list = store.get("a[href^=link]").unwrap();

    assert_eq!(list.count(), 3);
}

#[test]
fn test_html_page_children_valid_anchor_tags_in_main() {
    let queries = &[Query::all("main > section > a[href]", Save::all())
        .unwrap()
        .build()];

    let store = parse(HTML, queries).unwrap();
    let list = store.get("main > section > a[href]").unwrap();

    assert_eq!(list.count(), 5);
}

#[test]
fn test_html_page_single_main() {
    let queries = &[Query::all("main.red-background > section#id", Save::all())
        .unwrap()
        .build()];
    let store = parse(HTML, queries).unwrap();
    let list = store.get("main.red-background > section#id").unwrap();

    assert_eq!(list.count(), 1);
}

#[test]
fn test_html_multi_selection() {
    let query = Query::all("main > section", Save::all())
        .unwrap()
        .then(|section| {
            Ok([
                // BUG: first selection not working because their is no locking mechanism
                //section.first("> a[href]", Save::all()),
                section.all("> a[href]", Save::all())?,
                section.all("div a", Save::all())?,
                // BUG: If their are 2 identical sub-queries their should be an error.
                //section.all("> a[href]", Save::all()),
            ])
        })
        .unwrap()
        .build();

    let q = &[query];
    let store = parse(HTML, q).unwrap();
    let list = store.get("main > section").unwrap();

    println!("List: {:#?}", list.collect::<Vec<_>>());
}

#[test]
fn test_macro_static_query() {
    let static_query = query! {
        all("main > section", Save::all()) => {
            all("> a[href]", Save::all()),
            first("span", Save::only_text()),
        }
    };
    let runtime_query = Query::all("main > section", Save::all())
        .unwrap()
        .then(|ctx| {
            Ok([
                ctx.all("> a[href]", Save::all())?,
                ctx.first("span", Save::only_text())?,
            ])
        })
        .unwrap()
        .build();

    let static_queries = [static_query];
    let runtime_queries = [runtime_query];
    let static_store = parse(HTML, &static_queries).unwrap();
    let runtime_store = parse(HTML, &runtime_queries).unwrap();
    let count = |store: &scah::Store<'_, '_>, selector| {
        store.get(selector).map(|items| items.count()).unwrap_or(0)
    };

    assert_eq!(
        count(&static_store, "main > section"),
        count(&runtime_store, "main > section")
    );
    assert_eq!(
        count(&static_store, "> a[href]"),
        count(&runtime_store, "> a[href]")
    );
    assert_eq!(count(&static_store, "span"), count(&runtime_store, "span"));
}

#[test]
fn test_macro_query_matches_runtime_query_structure() {
    let static_query = query! {
        all("main > section", Save::all()) => {
            all("> a[href]", Save::all()),
            first("span", Save::only_text()),
        }
    };
    let runtime_query = Query::all("main > section", Save::all())
        .unwrap()
        .then(|ctx| {
            Ok([
                ctx.all("> a[href]", Save::all())?,
                ctx.first("span", Save::only_text())?,
            ])
        })
        .unwrap()
        .build();

    assert_eq!(static_query.states().len(), runtime_query.states().len());
    for (static_state, runtime_state) in static_query.states().iter().zip(runtime_query.states()) {
        assert_eq!(static_state.guard, runtime_state.guard);
        assert_eq!(static_state.predicate.name, runtime_state.predicate.name);
        assert_eq!(static_state.predicate.id, runtime_state.predicate.id);
        assert_eq!(
            static_state.predicate.classes.as_slice(),
            runtime_state.predicate.classes.as_slice()
        );
        assert_eq!(
            static_state.predicate.attributes.as_slice(),
            runtime_state.predicate.attributes.as_slice()
        );
    }

    assert_eq!(static_query.queries(), runtime_query.queries());
    assert_eq!(
        static_query.exit_at_section_end(),
        runtime_query.exit_at_section_end()
    );
}

#[test]
fn test_macro_query_matches_runtime_store_contents() {
    let static_query = query! {
        all("main > section", Save::all()) => {
            all("> a[href]", Save::all()),
            first("span", Save::only_text()),
        }
    };
    let runtime_query = Query::all("main > section", Save::all())
        .unwrap()
        .then(|ctx| {
            Ok([
                ctx.all("> a[href]", Save::all())?,
                ctx.first("span", Save::only_text())?,
            ])
        })
        .unwrap()
        .build();

    let static_queries = [static_query];
    let runtime_queries = [runtime_query];
    let static_store = parse(HTML, &static_queries).unwrap();
    let runtime_store = parse(HTML, &runtime_queries).unwrap();

    type Content = Vec<(String, Option<String>, Option<String>, Option<String>)>;

    fn collect_query_contents<'html, 'query>(
        store: &Store<'html, 'query>,
        selector: &str,
    ) -> Content {
        store
            .get(selector)
            .into_iter()
            .flatten()
            .map(|element| {
                (
                    element.name.to_string(),
                    element.attribute(store, "href").map(str::to_string),
                    element.inner_html.map(str::trim).map(str::to_string),
                    element.text(store).map(str::to_string),
                )
            })
            .collect()
    }

    for selector in ["main > section", "> a[href]", "span"] {
        assert_eq!(
            collect_query_contents(&static_store, selector),
            collect_query_contents(&runtime_store, selector),
            "selector mismatch for {selector}"
        );
    }
}

#[test]
fn escaped_quote_in_attribute_matches() {
    let html = r#"<a title="hello \"world\"">x</a>"#;
    let query = Query::all(r#"a[title="hello \"world\""]"#, Save::all())
        .unwrap()
        .build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    assert_eq!(
        store.get(r#"a[title="hello \"world\""]"#).unwrap().count(),
        1
    );
}

#[test]
fn quoted_attribute_selector_matches_url_with_query_string() {
    let html = r#"<a href="https://example.com/search?q=test">x</a>"#;
    let selector = r#"a[href="https://example.com/search?q=test"]"#;

    let query = Query::all(selector, Save::all()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();

    assert_eq!(store.get(selector).unwrap().count(), 1);
}

#[test]
fn quoted_attribute_selector_matches_value_with_selector_control_chars() {
    let html = r#"<div data-x="a=b*c]d"></div>"#;
    let selector = r#"div[data-x="a=b*c]d"]"#;

    let query = Query::all(selector, Save::all()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();

    assert_eq!(store.get(selector).unwrap().count(), 1);
}

#[test]
fn form_feed_descendant_combinator_matches() {
    let html = "<main><section id='s1'></section></main>";
    let selector = "main\u{000C}section";

    let query = Query::all(selector, Save::all()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();

    assert_eq!(store.get(selector).unwrap().count(), 1);
}
