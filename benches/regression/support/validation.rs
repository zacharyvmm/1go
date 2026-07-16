use scah::Store;

// ── Basic validation ───────────────────────────────────────────────────────

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

// ── Save-mode field validation ─────────────────────────────────────────────

/// Validate `Save::none()` semantics: elements have counts and attributes but
/// no inner HTML or text content.
pub fn assert_save_none_result(store: &Store<'_, '_>, selector: &str, expected_count: usize) {
    let elements: Vec<_> = store
        .get(selector)
        .unwrap_or_else(|| panic!("save_none: no results for {selector:?}"))
        .collect();
    assert_eq!(
        elements.len(),
        expected_count,
        "save_none: expected {expected_count} matches for {selector:?}, got {}",
        elements.len()
    );

    // Validate first and last representative elements
    let first = &elements[0];
    assert!(
        first.inner_html.is_none(),
        "save_none: inner_html should be None for {selector:?}[0]"
    );
    assert!(
        first.text_content(store).is_none(),
        "save_none: text_content should be None for {selector:?}[0]"
    );
    assert_eq!(
        first.attribute(store, "href"),
        Some("/post/0"),
        "save_none: expected href=/post/0 for {selector:?}[0]"
    );

    if elements.len() > 1 {
        let last = &elements[elements.len() - 1];
        assert!(
            last.inner_html.is_none(),
            "save_none: inner_html should be None for {selector:?}[last]"
        );
        assert!(
            last.text_content(store).is_none(),
            "save_none: text_content should be None for {selector:?}[last]"
        );
    }
}

/// Validate `Save::only_inner_html()` semantics: inner HTML present, text absent.
pub fn assert_save_inner_html_result(store: &Store<'_, '_>, selector: &str, expected_count: usize) {
    let elements: Vec<_> = store
        .get(selector)
        .unwrap_or_else(|| panic!("save_inner_html: no results for {selector:?}"))
        .collect();
    assert_eq!(
        elements.len(),
        expected_count,
        "save_inner_html: expected {expected_count} matches for {selector:?}, got {}",
        elements.len()
    );

    let first = &elements[0];
    assert!(
        first.inner_html.is_some(),
        "save_inner_html: inner_html should be Some for {selector:?}[0]"
    );
    assert!(
        first.text_content(store).is_none(),
        "save_inner_html: text_content should be None for {selector:?}[0]"
    );
    assert_eq!(
        first.attribute(store, "href"),
        Some("/post/0"),
        "save_inner_html: expected href=/post/0 for {selector:?}[0]"
    );

    // Verify representative inner HTML content
    let inner = first.inner_html.unwrap();
    assert!(
        inner.contains("<b>Post</b>"),
        "save_inner_html: expected <b>Post</b> in inner_html, got {inner:?}"
    );
    assert!(
        inner.contains("&lt;0&gt;"),
        "save_inner_html: expected &lt;0&gt; in inner_html, got {inner:?}"
    );

    if elements.len() > 1 {
        let last = &elements[elements.len() - 1];
        assert!(
            last.inner_html.is_some(),
            "save_inner_html: inner_html should be Some for {selector:?}[last]"
        );
        assert!(
            last.text_content(store).is_none(),
            "save_inner_html: text_content should be None for {selector:?}[last]"
        );
    }
}

/// Validate `Save::only_text_content()` semantics: text present, inner HTML absent.
pub fn assert_save_text_result(store: &Store<'_, '_>, selector: &str, expected_count: usize) {
    let elements: Vec<_> = store
        .get(selector)
        .unwrap_or_else(|| panic!("save_text: no results for {selector:?}"))
        .collect();
    assert_eq!(
        elements.len(),
        expected_count,
        "save_text: expected {expected_count} matches for {selector:?}, got {}",
        elements.len()
    );

    let first = &elements[0];
    assert!(
        first.inner_html.is_none(),
        "save_text: inner_html should be None for {selector:?}[0]"
    );
    assert!(
        first.text_content(store).is_some(),
        "save_text: text_content should be Some for {selector:?}[0]"
    );

    // Verify representative text content (SCaH preserves source entities)
    let text = first.text_content(store).unwrap();
    assert!(
        text.contains("Post"),
        "save_text: expected 'Post' in text_content, got {text:?}"
    );
    assert!(
        text.contains("&lt;0&gt;") || text.contains("<0>"),
        "save_text: expected entity representation in text_content, got {text:?}"
    );

    if elements.len() > 1 {
        let last = &elements[elements.len() - 1];
        assert!(
            last.inner_html.is_none(),
            "save_text: inner_html should be None for {selector:?}[last]"
        );
        assert!(
            last.text_content(store).is_some(),
            "save_text: text_content should be Some for {selector:?}[last]"
        );
    }
}

/// Validate `Save::all()` semantics: both inner HTML and text content present.
pub fn assert_save_all_result(store: &Store<'_, '_>, selector: &str, expected_count: usize) {
    let elements: Vec<_> = store
        .get(selector)
        .unwrap_or_else(|| panic!("save_all: no results for {selector:?}"))
        .collect();
    assert_eq!(
        elements.len(),
        expected_count,
        "save_all: expected {expected_count} matches for {selector:?}, got {}",
        elements.len()
    );

    let first = &elements[0];
    assert!(
        first.inner_html.is_some(),
        "save_all: inner_html should be Some for {selector:?}[0]"
    );
    assert!(
        first.text_content(store).is_some(),
        "save_all: text_content should be Some for {selector:?}[0]"
    );
    assert_eq!(
        first.attribute(store, "href"),
        Some("/post/0"),
        "save_all: expected href=/post/0 for {selector:?}[0]"
    );

    if elements.len() > 1 {
        let last = &elements[elements.len() - 1];
        assert!(
            last.inner_html.is_some(),
            "save_all: inner_html should be Some for {selector:?}[last]"
        );
        assert!(
            last.text_content(store).is_some(),
            "save_all: text_content should be Some for {selector:?}[last]"
        );
    }
}

// ── Nested product catalog validation ──────────────────────────────────────

/// Validate nested_all product results: parent count, every child present, content correct.
pub fn assert_product_catalog_all(
    store: &Store<'_, '_>,
    product_selector: &str,
    title_selector: &str,
    rating_selector: &str,
    description_selector: &str,
    expected_products: usize,
) {
    let products: Vec<_> = store
        .get(product_selector)
        .expect("product selector should exist")
        .collect();

    assert_eq!(
        products.len(),
        expected_products,
        "expected {expected_products} products, got {}",
        products.len()
    );

    for (index, product) in products.iter().enumerate() {
        let titles: Vec<_> = product
            .get(store, title_selector)
            .expect("title child query should exist")
            .collect();

        let ratings: Vec<_> = product
            .get(store, rating_selector)
            .expect("rating child query should exist")
            .collect();

        let descriptions: Vec<_> = product
            .get(store, description_selector)
            .expect("description child query should exist")
            .collect();

        assert_eq!(
            titles.len(),
            1,
            "product[{index}]: expected 1 title, got {}",
            titles.len()
        );
        assert_eq!(
            ratings.len(),
            1,
            "product[{index}]: expected 1 rating, got {}",
            ratings.len()
        );
        assert_eq!(
            descriptions.len(),
            1,
            "product[{index}]: expected 1 description, got {}",
            descriptions.len()
        );

        let product_number = index + 1;
        let expected_title = format!("Product #{product_number}");
        assert_eq!(
            titles[0].text_content(store),
            Some(expected_title.as_str()),
            "product[{index}]: wrong title"
        );

        let expected_description = format!("Description #{product_number}");
        assert_eq!(
            descriptions[0].text_content(store),
            Some(expected_description.as_str()),
            "product[{index}]: wrong description"
        );

        let expected_rating = format!("{}/5", (index % 5) + 1);
        assert_eq!(
            ratings[0].text_content(store),
            Some(expected_rating.as_str()),
            "product[{index}]: wrong rating"
        );
    }
}

/// Validate nested_first product results: exactly one parent with all child selectors.
pub fn assert_product_catalog_first(
    store: &Store<'_, '_>,
    product_selector: &str,
    title_selector: &str,
    rating_selector: &str,
    description_selector: &str,
) {
    let products: Vec<_> = store
        .get(product_selector)
        .expect("product selector should exist")
        .collect();

    assert_eq!(
        products.len(),
        1,
        "expected exactly 1 product, got {}",
        products.len()
    );

    let product = &products[0];

    let titles: Vec<_> = product
        .get(store, title_selector)
        .expect("title child query should exist")
        .collect();
    assert_eq!(titles.len(), 1);

    let ratings: Vec<_> = product
        .get(store, rating_selector)
        .expect("rating child query should exist")
        .collect();
    assert_eq!(ratings.len(), 1);

    let descriptions: Vec<_> = product
        .get(store, description_selector)
        .expect("description child query should exist")
        .collect();
    assert_eq!(descriptions.len(), 1);

    assert_eq!(titles[0].text_content(store), Some("Product #1"));
    assert_eq!(descriptions[0].text_content(store), Some("Description #1"));
    assert_eq!(ratings[0].text_content(store), Some("1/5"));
}

// ── Multi-query validation ─────────────────────────────────────────────────

/// Expected count for a class when cycling `class-0`..`class-N` across elements.
fn expected_class_count(element_count: usize, query_count: usize, class_index: usize) -> usize {
    let base = element_count / query_count;
    let remainder = element_count % query_count;
    base + usize::from(class_index < remainder)
}

/// Validate multi-query results: each selector's exact count and returned element classes.
pub fn assert_multi_query_results(store: &Store<'_, '_>, element_count: usize, query_count: usize) {
    let selectors: Vec<String> = (0..query_count).map(|i| format!(".class-{i}")).collect();

    let mut total = 0usize;

    for (class_index, selector) in selectors.iter().enumerate() {
        let elements: Vec<_> = store
            .get(selector)
            .unwrap_or_else(|| panic!("missing results for {selector}"))
            .collect();

        let expected = expected_class_count(element_count, query_count, class_index);
        assert_eq!(
            elements.len(),
            expected,
            "selector {selector}: expected {expected} elements, got {}",
            elements.len()
        );

        let expected_class = format!("class-{class_index}");

        for element in &elements {
            let classes = element.class.expect("matched div should have a class");
            assert!(
                classes
                    .split_ascii_whitespace()
                    .any(|class| class == expected_class),
                "selector {selector} returned element with classes {classes:?}",
            );
        }

        total += elements.len();
    }

    assert_eq!(
        total, element_count,
        "total across all selectors should equal {element_count}, got {total}"
    );
}
