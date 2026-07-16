use scah::Store;

/// Validate that a store has exactly `expected_count` matches for `selector`.
pub fn assert_match_count(store: &Store<'_, '_>, selector: &str, expected_count: usize) {
    let count = store.get(selector).map(|el| el.count()).unwrap_or(0);
    assert_eq!(
        count, expected_count,
        "selector {selector:?}: expected {expected_count} matches, got {count}"
    );
}

/// Validate that at least one match for `selector` has an `href` attribute.
pub fn assert_has_href_attribute(store: &Store<'_, '_>, selector: &str) {
    let mut elements = store.get(selector).expect("selector should have matches");
    let has_href = elements.any(|el| el.attribute(store, "href").is_some());
    assert!(
        has_href,
        "selector {selector:?}: expected at least one element with href attribute"
    );
}

/// Validate that the first-match benchmark produced the expected result.
pub fn assert_first_match_result(
    store: &Store<'_, '_>,
    selector: &str,
    expects_match: bool,
    expected_position_text: Option<&str>,
) {
    let elements = store.get(selector);
    if expects_match {
        let mut elements = elements.expect("expected matches for first-match selector");
        if let Some(first) = elements.next() {
            if let Some(expected_text) = expected_position_text {
                let text = first.text_content(store).unwrap_or("");
                assert!(
                    text.contains(expected_text),
                    "expected first match text to contain {expected_text:?}, got {text:?}"
                );
            }
        } else {
            panic!("expected at least one match for first-match selector");
        }
    } else {
        match elements {
            None => {} // no matches, expected
            Some(els) => assert_eq!(els.count(), 0, "expected no matches for no_match scenario"),
        }
    }
}
