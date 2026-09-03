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
fn structural_selectors_use_streaming_child_and_type_ordinals() {
    let html =
        "<ul><li>a</li><li class='hit'>b</li><li>c</li><li class='hit'>d</li></ul><p>x</p><p>y</p>";
    let queries = [
        Query::all("li:nth-child(even)", Save::all())
            .unwrap()
            .build(),
        Query::all("li:nth-of-type(2)", Save::all())
            .unwrap()
            .build(),
        Query::all("li:nth-child(-n+3)", Save::all())
            .unwrap()
            .build(),
        Query::all("li:nth-child(2 of .hit)", Save::all())
            .unwrap()
            .build(),
    ];
    let store = parse(html, &queries).unwrap();
    assert_eq!(store.get("li:nth-child(even)").unwrap().count(), 2);
    assert_eq!(store.get("li:nth-of-type(2)").unwrap().count(), 1);
    assert_eq!(store.get("li:nth-child(-n+3)").unwrap().count(), 3);
    assert_eq!(store.get("li:nth-child(2 of .hit)").unwrap().count(), 1);
}

#[test]
fn filtered_structural_selector_has_macro_parity() {
    let html = "<ul><li class='hit'>a</li><li>b</li><li class='hit'>c</li></ul>";
    let runtime = Query::all("li:nth-child(2 of .hit)", Save::all())
        .unwrap()
        .build();
    let compiled = query! { all("li:nth-child(2 of .hit)", Save::all()) };
    let runtime_queries = [runtime];
    let compiled_queries = [compiled];
    let runtime_store = parse(html, &runtime_queries).unwrap();
    let compiled_store = parse(html, &compiled_queries).unwrap();
    assert_eq!(
        runtime_store
            .get("li:nth-child(2 of .hit)")
            .unwrap()
            .count(),
        compiled_store
            .get("li:nth-child(2 of .hit)")
            .unwrap()
            .count()
    );
}

#[test]
fn filtered_ordinals_support_multiple_filters_and_attribute_filters() {
    let html = "<ul><li class='a'>1</li><li data-card='yes'>2</li><li class='a'>3</li><li data-card='yes'>4</li></ul>";
    let queries = [
        Query::all("li:nth-child(2 of .a)", Save::all())
            .unwrap()
            .build(),
        Query::all("li:nth-child(2 of [data-card])", Save::all())
            .unwrap()
            .build(),
    ];
    let store = parse(html, &queries).unwrap();
    assert_eq!(store.get("li:nth-child(2 of .a)").unwrap().count(), 1);
    assert_eq!(
        store.get("li:nth-child(2 of [data-card])").unwrap().count(),
        1
    );
}

#[test]
fn root_selector_matches_only_the_first_document_element() {
    let html = "<!-- comment --><main>one</main><aside>two</aside>";
    let query = Query::all(":root", Save::all()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    assert_eq!(store.get(":root").unwrap().count(), 1);
}

#[test]
fn future_dependent_structural_pseudos_are_rejected() {
    assert!(Query::all("li:nth-last-child(2)", Save::none()).is_err());
    assert!(Query::all("div:has(a)", Save::none()).is_err());
}

#[test]
fn scope_selector_anchors_nested_child_queries() {
    let query = Query::all("section", Save::none())
        .unwrap()
        .then(|_| Ok([Query::all(":scope > a", Save::all()).unwrap()]))
        .unwrap()
        .build();
    let queries = [query];
    let store = parse(
        "<section><a>one</a><div><a>two</a></div></section>",
        &queries,
    )
    .unwrap();
    let section = store.get("section").unwrap().next().unwrap();
    assert_eq!(section.get(&store, ":scope > a").unwrap().count(), 1);
}

#[test]
fn scope_anchor_is_normalized_once_across_builder_forms() {
    let selector = ":scope > :scope > a";
    let chained = Query::all("main", Save::all())
        .unwrap()
        .all(selector, Save::all())
        .unwrap()
        .build();
    let factory = Query::all("main", Save::all())
        .unwrap()
        .then(|context| Ok([context.all(selector, Save::all())?]))
        .unwrap()
        .build();

    for query in [chained, factory] {
        let queries = [query];
        let store = parse("<main><a>unexpected</a></main>", &queries).unwrap();
        let main = store.get("main").unwrap().next().unwrap();
        assert_eq!(main.get(&store, selector).into_iter().flatten().count(), 0);
    }

    let static_query = query! {
        all("main", Save::all()) => {
            all(":scope > :scope > a", Save::all())
        }
    };
    let store = parse(
        "<main><a>unexpected</a></main>",
        std::slice::from_ref(&static_query),
    )
    .unwrap();
    let main = store.get("main").unwrap().next().unwrap();
    assert_eq!(main.get(&store, selector).into_iter().flatten().count(), 0);
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

    assert!(last.text_content(&store).is_some());
    assert_eq!(
        last.text_content(&store).unwrap(),
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
        first.text_content(&store).unwrap(),
        r#"Link 1 Link 2 Link 3 Not selected (nested in div) No link here"#
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
    assert_eq!(a.text_content(&store).unwrap(), "Link 1");
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
            first("span", Save::only_text_content()),
        }
    };
    let runtime_query = Query::all("main > section", Save::all())
        .unwrap()
        .then(|ctx| {
            Ok([
                ctx.all("> a[href]", Save::all())?,
                ctx.first("span", Save::only_text_content())?,
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
fn test_macro_static_query_with_nested_attributes() {
    let not_query = query! {
        all("div:not([hidden])", Save::none())
    };
    let alternatives_query = query! {
        all("div:is([data-x=a], [data-x=b])", Save::none())
    };

    assert_eq!(not_query.states().len(), 1);
    assert_eq!(alternatives_query.states().len(), 1);
    assert_eq!(
        alternatives_query.states()[0].metadata().attribute_names(),
        &["data-x"]
    );
}

#[test]
fn test_top_level_scope_anchors_to_document_root() {
    let html = r#"<main><a id="child"></a></main><a id="top"></a>"#;
    let query = Query::all(":scope > a", Save::none()).unwrap().build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    let ids = store
        .get(":scope > a")
        .unwrap()
        .filter_map(|element| element.id)
        .collect::<Vec<_>>();

    assert_eq!(ids, ["child"]);
}

#[test]
fn test_macro_query_matches_runtime_query_structure() {
    let static_query = query! {
        all("main > section", Save::all()) => {
            all("> a[href]", Save::all()),
            first("span", Save::only_text_content()),
        }
    };
    let runtime_query = Query::all("main > section", Save::all())
        .unwrap()
        .then(|ctx| {
            Ok([
                ctx.all("> a[href]", Save::all())?,
                ctx.first("span", Save::only_text_content())?,
            ])
        })
        .unwrap()
        .build();

    assert_eq!(static_query.states().len(), runtime_query.states().len());
    for (static_state, runtime_state) in static_query.states().iter().zip(runtime_query.states()) {
        assert_eq!(static_state.guard, runtime_state.guard);
        assert_eq!(
            static_state.predicate().name,
            runtime_state.predicate().name
        );
        assert_eq!(static_state.predicate().id, runtime_state.predicate().id);
        assert_eq!(
            static_state.predicate().classes.as_slice(),
            runtime_state.predicate().classes.as_slice()
        );
        assert_eq!(
            static_state.predicate().attributes.as_slice(),
            runtime_state.predicate().attributes.as_slice()
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
            first("span", Save::only_text_content()),
        }
    };
    let runtime_query = Query::all("main > section", Save::all())
        .unwrap()
        .then(|ctx| {
            Ok([
                ctx.all("> a[href]", Save::all())?,
                ctx.first("span", Save::only_text_content())?,
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
                    element.text_content(store).map(str::to_string),
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
fn replacing_transition_predicate_refreshes_parser_preflight() {
    let mut query = Query::all("a", Save::name_only()).unwrap().build();
    let mut predicate = query.states[0].predicate().clone();
    predicate.name = Some("div");
    query.states[0].set_predicate(predicate);

    let queries = [query];
    let store = parse("<a></a><div></div>", &queries).unwrap();
    let mut matches = store.get("a").unwrap();

    assert_eq!(matches.next().unwrap().name, "div");
    assert!(matches.next().is_none());
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
fn universal_child_selector_keeps_its_left_hand_transition() {
    let selector = "* > a";
    let query = Query::all(selector, Save::all()).unwrap().build();
    let queries = [query];
    let store = parse("<section><div><a></a></div></section>", &queries).unwrap();

    assert_eq!(store.get(selector).unwrap().count(), 1);
}

#[test]
fn filtered_ordinals_exclude_elements_outside_the_filter() {
    let html = concat!(
        "<ul>",
        "<li>miss one</li><li class='hit'>hit one</li>",
        "<li>miss two</li><li class='hit'>hit two</li>",
        "</ul>"
    );
    let selectors = ["li:nth-child(n of .hit)", "li:nth-child(even of .hit)"];
    let queries: Vec<_> = selectors
        .iter()
        .map(|selector| Query::all(selector, Save::all()).unwrap().build())
        .collect();
    let store = parse(html, &queries).unwrap();

    assert_eq!(store.get(selectors[0]).unwrap().count(), 2);
    assert_eq!(store.get(selectors[1]).unwrap().count(), 1);
}

#[test]
fn filtered_ordinals_support_more_than_eight_distinct_filters() {
    let selectors: Vec<_> = (0..9)
        .map(|index| format!("li:nth-child(1 of .f{index})"))
        .collect();
    let queries: Vec<_> = selectors
        .iter()
        .map(|selector| Query::all(selector, Save::all()).unwrap().build())
        .collect();
    let html = (0..9)
        .map(|index| format!("<li class='f{index}'></li>"))
        .collect::<String>();
    let store = parse(&html, &queries).unwrap();

    for selector in &selectors {
        assert_eq!(store.get(selector).unwrap().count(), 1, "{selector}");
    }
}

#[test]
fn filtered_ordinals_support_more_than_eight_overlapping_filters() {
    let selectors: Vec<_> = (0..9)
        .map(|index| format!("li:nth-child(1 of .f{index})"))
        .collect();
    let queries: Vec<_> = selectors
        .iter()
        .map(|selector| Query::all(selector, Save::all()).unwrap().build())
        .collect();
    let classes = (0..9)
        .map(|index| format!("f{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    let html = format!("<ul><li class='{classes}'></li></ul>");
    let store = parse(&html, &queries).unwrap();

    for selector in &selectors {
        assert_eq!(store.get(selector).unwrap().count(), 1, "{selector}");
    }
}

#[test]
fn filtered_ordinal_uses_one_selector_list() {
    let selector = "li:nth-child(2 of .a, [data-card])";
    let query = Query::all(selector, Save::all()).unwrap().build();
    let queries = [query];
    let store = parse(
        "<ul><li class='a'></li><li></li><li data-card></li></ul>",
        &queries,
    )
    .unwrap();

    assert_eq!(store.get(selector).unwrap().count(), 1);
}

#[test]
fn filtered_ordinals_count_matching_siblings_of_other_types() {
    let selector = "li:nth-child(2 of .hit)";
    let query = Query::all(selector, Save::all()).unwrap().build();
    let queries = [query];
    let store = parse(
        "<section><div class='hit'></div><li class='hit'></li></section>",
        &queries,
    )
    .unwrap();

    assert_eq!(store.get(selector).unwrap().count(), 1);
}

#[test]
fn overlapping_parent_alternatives_reuse_the_saved_output_scope() {
    let query = Query::all("div, .hit", Save::all())
        .unwrap()
        .then(|parent| Ok([parent.all("span", Save::all())?]))
        .unwrap()
        .build();
    let queries = [query];
    let store = parse("<div class='hit'><span></span></div>", &queries).unwrap();
    let parent = store.get("div, .hit").unwrap().next().unwrap();

    assert_eq!(parent.get(&store, "span").unwrap().count(), 1);
    assert_eq!(store.get("span").map_or(0, Iterator::count), 0);
}

#[test]
fn uppercase_attribute_flags_have_runtime_and_macro_parity() {
    let runtime_insensitive = Query::all(r#"[data-x="FOO" I]"#, Save::all())
        .unwrap()
        .build();
    let runtime_sensitive = Query::all(r#"[data-x="FOO" S]"#, Save::all())
        .unwrap()
        .build();
    let compiled_insensitive = query! { all(r#"[data-x="FOO" I]"#, Save::all()) };
    let compiled_sensitive = query! { all(r#"[data-x="FOO" S]"#, Save::all()) };
    let runtime_queries = [runtime_insensitive, runtime_sensitive];
    let compiled_queries = [compiled_insensitive, compiled_sensitive];
    let runtime_store = parse("<div data-x='foo'></div>", &runtime_queries).unwrap();
    let compiled_store = parse("<div data-x='foo'></div>", &compiled_queries).unwrap();

    for store in [&runtime_store, &compiled_store] {
        assert_eq!(store.get(r#"[data-x="FOO" I]"#).unwrap().count(), 1);
        assert_eq!(
            store.get(r#"[data-x="FOO" S]"#).map_or(0, Iterator::count),
            0
        );
    }
}

#[test]
fn unquoted_i_and_s_values_have_runtime_and_macro_parity() {
    let html = "<div data-i='i' data-s='s' data-mi='I' data-ms='s'></div>";
    let runtime_queries = [
        Query::all("[data-i=i]", Save::all()).unwrap().build(),
        Query::all("[data-s=s]", Save::all()).unwrap().build(),
        Query::all("[data-mi=i i]", Save::all()).unwrap().build(),
        Query::all("[data-ms=s s]", Save::all()).unwrap().build(),
    ];
    let compiled_queries = [
        query! { all("[data-i=i]", Save::all()) },
        query! { all("[data-s=s]", Save::all()) },
        query! { all("[data-mi=i i]", Save::all()) },
        query! { all("[data-ms=s s]", Save::all()) },
    ];
    let runtime_store = parse(html, &runtime_queries).unwrap();
    let compiled_store = parse(html, &compiled_queries).unwrap();

    for selector in ["[data-i=i]", "[data-s=s]", "[data-mi=i i]", "[data-ms=s s]"] {
        assert_eq!(
            runtime_store.get(selector).unwrap().count(),
            1,
            "{selector}"
        );
        assert_eq!(
            compiled_store.get(selector).unwrap().count(),
            1,
            "{selector}"
        );
    }
}

#[test]
fn mixed_case_pseudo_names_have_runtime_and_macro_parity() {
    let html =
        "<main><div class='card'></div><div class='ad'></div><ul><li></li><li></li></ul></main>";
    let runtime_queries = [
        Query::all("li:FIRST-CHILD", Save::all()).unwrap().build(),
        Query::all("li:NTH-CHILD(2)", Save::all()).unwrap().build(),
        Query::all("div:NOT(.ad)", Save::all()).unwrap().build(),
        Query::all(":ROOT", Save::all()).unwrap().build(),
    ];
    let compiled_queries = [
        query! { all("li:FIRST-CHILD", Save::all()) },
        query! { all("li:NTH-CHILD(2)", Save::all()) },
        query! { all("div:NOT(.ad)", Save::all()) },
        query! { all(":ROOT", Save::all()) },
    ];
    let runtime_store = parse(html, &runtime_queries).unwrap();
    let compiled_store = parse(html, &compiled_queries).unwrap();

    for selector in ["li:FIRST-CHILD", "li:NTH-CHILD(2)", "div:NOT(.ad)", ":ROOT"] {
        assert_eq!(
            runtime_store.get(selector).unwrap().count(),
            compiled_store.get(selector).unwrap().count(),
            "{selector}"
        );
        assert_eq!(
            runtime_store.get(selector).unwrap().count(),
            1,
            "{selector}"
        );
    }
}

#[test]
fn an_plus_b_case_and_whitespace_have_runtime_and_macro_parity() {
    let html = "<ul><li></li><li></li><li></li><li></li></ul>";
    let runtime_queries = [
        Query::all("li:nth-child(ODD)", Save::all())
            .unwrap()
            .build(),
        Query::all("li:nth-child(2N + 1)", Save::all())
            .unwrap()
            .build(),
    ];
    let compiled_queries = [
        query! { all("li:nth-child(ODD)", Save::all()) },
        query! { all("li:nth-child(2N + 1)", Save::all()) },
    ];
    let runtime_store = parse(html, &runtime_queries).unwrap();
    let compiled_store = parse(html, &compiled_queries).unwrap();

    for selector in ["li:nth-child(ODD)", "li:nth-child(2N + 1)"] {
        assert_eq!(
            runtime_store.get(selector).unwrap().count(),
            2,
            "{selector}"
        );
        assert_eq!(
            compiled_store.get(selector).unwrap().count(),
            2,
            "{selector}"
        );
    }

    for selector in [
        "li:nth-child(3 n)",
        "li:nth-child(+ 2n)",
        "li:nth-child(+ 2)",
    ] {
        assert!(Query::all(selector, Save::none()).is_err(), "{selector}");
    }
}

#[test]
fn unsupported_structural_compositions_fail_at_query_build_time() {
    for selector in [
        "li:not(:first-child)",
        "li:nth-child(2 of :first-child)",
        ":scope.foo > a",
    ] {
        assert!(Query::all(selector, Save::none()).is_err(), "{selector}");
    }

    let selector = "li:is(:first-child)";
    let queries = [Query::all(selector, Save::all()).unwrap().build()];
    let store = parse("<ul><li></li></ul>", &queries).unwrap();
    assert_eq!(
        store.get(selector).map_or(0, |elements| elements.count()),
        0
    );
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

#[test]
fn universal_and_attribute_case_flags_match_without_lowercasing() {
    let html =
        r#"<main><div data-kind="FooBar" class="card"></div><div data-kind="other"></div></main>"#;
    let selectors = [
        "*",
        "*.card",
        r#"[data-kind="FOO" i]"#,
        r#"[data-kind^="FOO" i]"#,
        r#"[data-kind="FOO" s]"#,
    ];
    let queries: Vec<_> = selectors
        .iter()
        .map(|selector| Query::all(selector, Save::all()).unwrap().build())
        .collect();
    let store = parse(html, &queries).unwrap();

    assert_eq!(store.get("*").unwrap().count(), 3);
    assert_eq!(store.get("*.card").unwrap().count(), 1);
    assert_eq!(
        store
            .get(r#"[data-kind="FOO" i]"#)
            .map_or(0, |items| items.count()),
        0
    );
    assert_eq!(store.get(r#"[data-kind^="FOO" i]"#).unwrap().count(), 1);
    assert_eq!(
        store
            .get(r#"[data-kind="FOO" s]"#)
            .map_or(0, |items| items.count()),
        0
    );
}

#[test]
fn attribute_case_flags_have_macro_parity() {
    let query = query! { all(r#"[data-kind="FOO" i]"#, Save::all()) };
    let queries = [query];
    let store = parse(r#"<div data-kind="foo"></div>"#, &queries).unwrap();
    assert_eq!(store.get(r#"[data-kind="FOO" i]"#).unwrap().count(), 1);
}

#[test]
fn local_logical_pseudos_match_runtime_and_nested_lists() {
    let html = r#"<main><div class="ok"></div><div class="ad"></div><span hidden></span></main>"#;
    let selectors = [
        "div:not(.ad)",
        "div:is(.ok, .missing)",
        "div:where(.missing, .ad)",
        "div:not(:is(.ad, [hidden]))",
        "div:not([hidden])",
    ];
    let queries: Vec<_> = selectors
        .iter()
        .map(|selector| Query::all(selector, Save::all()).unwrap().build())
        .collect();
    let store = parse(html, &queries).unwrap();

    assert_eq!(store.get(selectors[0]).unwrap().count(), 1);
    assert_eq!(store.get(selectors[1]).unwrap().count(), 1);
    assert_eq!(store.get(selectors[2]).unwrap().count(), 1);
    assert_eq!(store.get(selectors[3]).unwrap().count(), 1);
    assert_eq!(store.get(selectors[4]).unwrap().count(), 2);
}

#[test]
fn local_logical_pseudos_have_macro_parity() {
    let query = query! { all("div:not(.ad)", Save::all()) };
    let queries = [query];
    let store = parse(r#"<div class="ok"></div><div class="ad"></div>"#, &queries).unwrap();
    assert_eq!(store.get("div:not(.ad)").unwrap().count(), 1);
}

#[test]
fn selector_lists_match_in_document_order_and_deduplicate() {
    let html =
        r#"<main><h2>two</h2><h1>one</h1><div class="hit"></div><h1 class="hit">last</h1></main>"#;
    let selector = "h1, h2";
    let query = Query::all(selector, Save::only_text_content())
        .unwrap()
        .build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    let names: Vec<_> = store
        .get(selector)
        .unwrap()
        .map(|element| element.name)
        .collect();
    assert_eq!(names, vec!["h2", "h1", "h1"]);

    let complex = "main > h1, main > h2";
    let query = Query::first(complex, Save::only_text_content())
        .unwrap()
        .build();
    let queries = [query];
    let store = parse(html, &queries).unwrap();
    assert_eq!(store.get(complex).unwrap().next().unwrap().name, "h2");

    let overlap = "div, .hit";
    let query = Query::all(overlap, Save::none()).unwrap().build();
    let queries = [query];
    let store = parse(r#"<div class="hit"></div><p class="hit"></p>"#, &queries).unwrap();
    assert_eq!(store.get(overlap).unwrap().count(), 2);
}

#[test]
fn selector_list_has_macro_parity() {
    let query = query! { all("h1, h2", Save::only_text_content()) };
    let queries = [query];
    let store = parse("<h2></h2><h1></h1>", &queries).unwrap();
    assert_eq!(store.get("h1, h2").unwrap().count(), 2);
}

#[test]
fn child_selector_lists_share_one_output_parent() {
    let query = Query::all("main", Save::all())
        .unwrap()
        .then(|main| Ok([main.all("h1, h2", Save::all())?]))
        .unwrap()
        .build();
    let queries = [query];
    let store = parse("<main><h2></h2><h1></h1></main>", &queries).unwrap();
    let main = store.get("main").unwrap().next().unwrap();
    assert_eq!(main.get(&store, "h1, h2").unwrap().count(), 2);
}
