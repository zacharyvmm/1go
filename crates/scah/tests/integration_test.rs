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

    let store = parse(html, &queries);

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

    let store = parse(html, &queries);

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

    let store = parse(html, &queries);

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

    let store = parse(html, &queries);

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
    let store = parse(HTML, queries);
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
    let store = parse(HTML, queries);
    println!("Store: {:#?}", store);

    let list = store.get("a").unwrap().collect::<Vec<_>>();

    assert_eq!(list.len(), 7);
    println!("List: {:#?}", list);
}

#[test]
fn test_html_page_first_anchor_tag_selection() {
    let queries = &[Query::first("a", Save::all()).unwrap().build()];
    let store = parse(HTML, queries);
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
    let store = parse(HTML, queries);
    let list = store.get("a[href^=link]").unwrap();

    assert_eq!(list.count(), 3);
}

#[test]
fn test_html_page_children_valid_anchor_tags_in_main() {
    let queries = &[Query::all("main > section > a[href]", Save::all())
        .unwrap()
        .build()];

    let store = parse(HTML, queries);
    let list = store.get("main > section > a[href]").unwrap();

    assert_eq!(list.count(), 5);
}

#[test]
fn test_html_page_single_main() {
    let queries = &[Query::all("main.red-background > section#id", Save::all())
        .unwrap()
        .build()];
    let store = parse(HTML, queries);
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
    let store = parse(HTML, q);
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
    let static_store = parse(HTML, &static_queries);
    let runtime_store = parse(HTML, &runtime_queries);
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
    let static_store = parse(HTML, &static_queries);
    let runtime_store = parse(HTML, &runtime_queries);

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

// ============================================================================
// Edge Case Regression Tests
// ============================================================================

/// Edge Case #1: Empty elements must not panic
#[test]
fn test_empty_elements_no_panic() {
    let html = "<div></div><p>   </p><div><!-- comment --></div><div><span></span></div>";
    let queries = &[Query::all("div", Save::only_text_content())
        .unwrap()
        .build()];
    let store = parse(html, queries);
    let divs: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 3);
    for div in &divs {
        // None of these should have text content, but they must not panic
        assert_eq!(div.text_content(&store), None);
    }
}

/// Edge Case #2: Slash / self-closing behavior
#[test]
fn test_slash_self_closing() {
    let html = r#"<hr/><hr /><input disabled/><input disabled /><div />after</div>"#;
    let queries = &[Query::all("hr", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    // Both <hr> variants should be matched
    assert_eq!(store.get("hr").unwrap().count(), 2);

    // Non-void elements with / should NOT be self-closing
    let div_queries = &[Query::all("div", Save::all()).unwrap().build()];
    let div_store = parse(html, div_queries);
    let divs: Vec<_> = div_store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 1);
}

/// Edge Case #2 (continued): / should not leak into attributes
#[test]
fn test_slash_not_in_attributes() {
    let queries = &[Query::all("hr", Save::all()).unwrap().build()];
    let store = parse("<hr />", queries);
    let hrs: Vec<_> = store.get("hr").unwrap().collect();
    assert_eq!(hrs.len(), 1);
    // / should NOT appear as an attribute
    assert!(hrs[0].attribute(&store, "/").is_none());
}

/// Edge Case #3: Whitespace in tags (tabs, newlines)
#[test]
fn test_whitespace_in_tags() {
    let html = "<a\n  href=\"x\"\n  class=\"link\">text</a>";
    let queries = &[Query::all("a.link", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    let links: Vec<_> = store.get("a.link").unwrap().collect();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("x"));
}

/// Edge Case #7: Comments containing >
#[test]
fn test_comment_with_gt() {
    // The > inside the comment should not leak fake elements
    let html = r#"<!-- a > <a href="fake">not-real</a> --><a href="real">real</a>"#;
    let queries = &[Query::all("a", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    let links: Vec<_> = store.get("a").unwrap().collect();
    // Only the real link should be found
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("real"));
}

/// Edge Case #8: Raw-text / RCDATA elements (style, script, textarea, title)
#[test]
fn test_raw_text_elements() {
    let html = r#"<style>.x::before { content: "<a>"; }</style><a>real</a>"#;
    let queries = &[Query::all("a", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    let links: Vec<_> = store.get("a").unwrap().collect();
    // The fake <a> inside <style> should not be matched
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].text_content(&store), Some("real"));
}

#[test]
fn test_script_raw_text() {
    let html = r#"<script>const x = "<a>fake</a>";</script><a>real</a>"#;
    let queries = &[Query::all("a", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    let links: Vec<_> = store.get("a").unwrap().collect();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].text_content(&store), Some("real"));
}

#[test]
fn test_textarea_raw_text() {
    let html = r#"<textarea><a>not an element</a></textarea><a>real</a>"#;
    let queries = &[Query::all("a", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    let links: Vec<_> = store.get("a").unwrap().collect();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].text_content(&store), Some("real"));
}

#[test]
fn test_title_raw_text() {
    let html = r#"<title><a>not an element</a></title><a>real</a>"#;
    let queries = &[Query::all("a", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    let links: Vec<_> = store.get("a").unwrap().collect();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].text_content(&store), Some("real"));
}

/// Edge Case #4: [id] and [class] attribute selectors
#[test]
fn test_id_attribute_selector() {
    let html = r#"<div id="a">A</div><div>B</div>"#;
    let queries = &[Query::all("div[id]", Save::only_text_content())
        .unwrap()
        .build()];
    let store = parse(html, queries);
    let divs: Vec<_> = store.get("div[id]").unwrap().collect();
    assert_eq!(divs.len(), 1);
    assert_eq!(divs[0].text_content(&store), Some("A"));
}

#[test]
fn test_class_attribute_selector() {
    let html = r#"<div class="x">A</div><div>B</div>"#;
    let queries = &[Query::all("div[class]", Save::only_text_content())
        .unwrap()
        .build()];
    let store = parse(html, queries);
    let divs: Vec<_> = store.get("div[class]").unwrap().collect();
    assert_eq!(divs.len(), 1);
    assert_eq!(divs[0].text_content(&store), Some("A"));
}

/// Edge Case #5: Nested descendant deduplication
#[test]
fn test_nested_descendant_dedup() {
    let html = r#"<section><div><a id="one">1</a></div><div><div><a id="two">2</a></div></div></section>"#;
    let queries = &[Query::first("section", Save::none())
        .unwrap()
        .then(|s| Ok([s.all("div a", Save::only_text_content())?]))
        .unwrap()
        .build()];
    let store = parse(html, queries);
    let root_sections: Vec<_> = store.get("section").unwrap().collect();
    assert_eq!(root_sections.len(), 1);
    // Each <a> should appear exactly once, even though "two"
    // is a descendant of two nested divs
    let anchors: Vec<_> = root_sections[0]
        .get(&store, "div a")
        .unwrap()
        .collect();
    assert_eq!(anchors.len(), 2);
    let texts: Vec<_> = anchors.iter().map(|a| a.text_content(&store)).collect();
    assert!(texts.contains(&Some("1")));
    assert!(texts.contains(&Some("2")));
}

/// Edge Case #6: Nested first() completion
#[test]
fn test_nested_first_completion() {
    // Each <div> should only yield its FIRST <a> child
    let html = r#"<div><a>1</a><a>2</a></div><div><a>3</a><a>4</a></div>"#;
    let queries = &[Query::all("div", Save::none())
        .unwrap()
        .then(|d| Ok([d.first("> a", Save::only_text_content())?]))
        .unwrap()
        .build()];
    let store = parse(html, queries);
    let divs: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 2);

    for div in &divs {
        let children: Vec<_> = div.get(&store, "> a").unwrap().collect();
        // Each div should have exactly ONE <a> child (the first one)
        assert_eq!(children.len(), 1, "Each div should have exactly 1 first child <a>");
    }
}

/// Single element with multiple ancestor paths — browser deduplicates
#[test]
fn test_single_element_multiple_ancestors_count() {
    // <div><div><a>X</a></div></div> — the <a> has two <div> ancestors
    // but is physically one DOM node. querySelectorAll('div a') returns 1.
    let html = r#"<div><div><a>X</a></div></div>"#;
    let queries = &[Query::all("div a", Save::only_text_content())
        .unwrap()
        .build()];
    let store = parse(html, queries);
    let anchors: Vec<_> = store.get("div a").unwrap().collect();
    assert_eq!(anchors.len(), 1, "single <a> with multiple <div> ancestors must appear once");
    assert_eq!(anchors[0].text_content(&store), Some("X"));
}

/// Deep nesting should not create phantom duplicates
#[test]
fn test_deep_nesting_single_leaf() {
    // Triple-nested divs with one span at the bottom
    let html = r#"<div><div><div><span>deep</span></div></div></div>"#;
    let queries = &[Query::all("div span", Save::only_text_content())
        .unwrap()
        .build()];
    let store = parse(html, queries);
    let spans: Vec<_> = store.get("div span").unwrap().collect();
    assert_eq!(spans.len(), 1, "deeply nested single span must appear once");
    assert_eq!(spans[0].text_content(&store), Some("deep"));
}

/// Child combinator — each <a> is a direct child of some <div>
#[test]
fn test_child_combinator_nested() {
    // <div><a>1</a><div><a>2</a></div></div>
    // Both <a> elements are direct children of a <div>:
    //   <a>1</a> is direct child of outer <div>
    //   <a>2</a> is direct child of inner <div>
    let html = r#"<div><a>1</a><div><a>2</a></div></div>"#;
    let queries = &[Query::all("div > a", Save::only_text_content())
        .unwrap()
        .build()];
    let store = parse(html, queries);
    let anchors: Vec<_> = store.get("div > a").unwrap().collect();
    assert_eq!(anchors.len(), 2, "both <a> elements are direct children of some <div>");
    let texts: Vec<_> = anchors.iter().map(|a| a.text_content(&store).unwrap()).collect();
    assert!(texts.contains(&"1"));
    assert!(texts.contains(&"2"));
}

/// Multiple sections with nested divs — each <a> is distinct
#[test]
fn test_multiple_sections_nested() {
    let html = r#"<section><div><a>A1</a></div></section><section><div><div><a>A2</a></div></div></section>"#;
    let queries = &[Query::all("section div a", Save::only_text_content())
        .unwrap()
        .build()];
    let store = parse(html, queries);
    let anchors: Vec<_> = store.get("section div a").unwrap().collect();
    assert_eq!(anchors.len(), 2, "two distinct sections, two distinct <a> elements");
    let texts: Vec<_> = anchors.iter().map(|a| a.text_content(&store).unwrap()).collect();
    assert!(texts.contains(&"A1"));
    assert!(texts.contains(&"A2"));
}

/// .then() — same element appears under both parents (correct for per-parent model)
#[test]
fn test_then_same_element_two_parents() {
    // <div id="outer"><div id="inner"><a>X</a></div></div>
    // Both divs should see the <a> as their descendant
    let html = r#"<div id="outer"><div id="inner"><a>X</a></div></div>"#;
    let queries = &[Query::all("div", Save::none())
        .unwrap()
        .then(|d| Ok([d.all("a", Save::only_text_content())?]))
        .unwrap()
        .build()];
    let store = parse(html, queries);
    let divs: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 2);

    let outer_children: Vec<_> = divs[0].get(&store, "a").unwrap().collect();
    let inner_children: Vec<_> = divs[1].get(&store, "a").unwrap().collect();
    // Both divs should find the <a> (it IS a descendant of both)
    assert_eq!(outer_children.len(), 1, "outer div should see <a> as descendant");
    assert_eq!(inner_children.len(), 1, "inner div should see <a> as descendant");
    assert_eq!(outer_children[0].text_content(&store), Some("X"));
    assert_eq!(inner_children[0].text_content(&store), Some("X"));
}

// ============================================================================
// Fix Regression Tests (edge-case-discovery-and-remediation)
// ============================================================================

/// Fix 1: Double `=` in unquoted attrs must not panic
#[test]
fn test_double_equals_in_unquoted_attrs() {
    let html = r#"<div key=a=b>text</div>"#;
    let queries = &[Query::all("div", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    let divs: Vec<_> = store.get("div").unwrap().collect();
    assert_eq!(divs.len(), 1);
    // The key should be parsed (key="a" and then "b" becomes a separate key)
    // or the value could be consumed as-is. We mainly care it doesn't panic.
}

/// Fix 2: `<!-->` must not swallow rest of document
#[test]
fn test_abrupt_comment_close() {
    let html = r#"<!--><div id="real">content</div>"#;
    let queries = &[Query::all("div#real", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    let divs: Vec<_> = store.get("div#real").unwrap().collect();
    assert_eq!(divs.len(), 1, "abruptly-closed comment should not swallow the div");
}

/// Fix 2 also: `<!--` alone at EOF should not panic
#[test]
fn test_comment_open_only() {
    let html = "<!--";
    let queries = &[Query::all("div", Save::none()).unwrap().build()];
    let store = parse(html, queries);
    // Just verify no panic
    assert!(store.get("div").is_none());
}

/// Fix 3: Form feed (U+000C) must be treated as whitespace
#[test]
fn test_form_feed_as_whitespace() {
    let html = "<div\x0Cclass=\"real\">text</div>";
    let queries = &[Query::all("div.real", Save::only_text_content())
        .unwrap()
        .build()];
    let store = parse(html, queries);
    let divs: Vec<_> = store.get("div.real").unwrap().collect();
    assert_eq!(divs.len(), 1);
}

/// Fix 5: Attribute match operators without `=` must be rejected
#[test]
fn test_attr_selector_missing_equals() {
    // [attr~] without `=` should error
    assert!(Query::all("div[class~]", Save::none()).is_err());
    // [attr^] without `=` should error
    assert!(Query::all("div[id^]", Save::none()).is_err());
    // [attr$] without `=` should error
    assert!(Query::all("a[href$]", Save::none()).is_err());
    // [attr*] without `=` should error
    assert!(Query::all("a[href*]", Save::none()).is_err());
    // [attr] (presence) without `=` should still work
    assert!(Query::all("div[id]", Save::none()).is_ok());
    assert!(Query::all("div[class]", Save::none()).is_ok());
}

/// Fix 6: Simple tag parser must handle all raw text elements, not just script
#[test]
fn test_simple_tag_parser_style_raw_text() {
    let html = r#"<style>div { content: "<a>"; }</style><a href="real">link</a>"#;
    let queries = &[Query::all("a", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    let links: Vec<_> = store.get("a").unwrap().collect();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("real"));
}

/// Fix 6 continued: textarea / title in simple tag parser
#[test]
fn test_simple_tag_parser_textarea_raw_text() {
    let html = r#"<textarea><a>not an element</a></textarea><a href="real">link</a>"#;
    let queries = &[Query::all("a", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    let links: Vec<_> = store.get("a").unwrap().collect();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("real"));
}

/// Fix 6 continued: title in simple tag parser
#[test]
fn test_simple_tag_parser_title_raw_text() {
    let html = r#"<title><a>not an element</a></title><a href="real">link</a>"#;
    let queries = &[Query::all("a", Save::all()).unwrap().build()];
    let store = parse(html, queries);
    let links: Vec<_> = store.get("a").unwrap().collect();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].attribute(&store, "href"), Some("real"));
}
