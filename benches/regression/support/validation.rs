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
///
/// When `expected_index` is `Some(index)`, requires exactly one result whose
/// text, inner HTML, href, and class match the fixture at that index.
/// When `None`, requires zero results.
pub fn assert_first_match_result(
    store: &Store<'_, '_>,
    selector: &str,
    expected_index: Option<usize>,
) {
    let elements: Vec<_> = store
        .get(selector)
        .map(|elements| elements.collect())
        .unwrap_or_default();

    match expected_index {
        Some(index) => {
            assert_eq!(
                elements.len(),
                1,
                "first-match selector {selector:?}: expected exactly one result, got {}",
                elements.len(),
            );

            let element = &elements[0];
            let expected_text = format!("Post {index}");
            let expected_href = format!("/post/{index}");

            assert_eq!(
                element.text_content(store),
                Some(expected_text.as_str()),
                "first-match selector {selector:?}: wrong text content",
            );

            assert_eq!(
                element.inner_html,
                Some(expected_text.as_str()),
                "first-match selector {selector:?}: wrong inner HTML",
            );

            assert_eq!(
                element.attribute(store, "href"),
                Some(expected_href.as_str()),
                "first-match selector {selector:?}: wrong href",
            );

            assert_eq!(
                element.class,
                Some("target"),
                "first-match selector {selector:?}: wrong class",
            );
        }

        None => {
            assert!(
                elements.is_empty(),
                "first-match selector {selector:?}: expected no results, got {}",
                elements.len(),
            );
        }
    }
}

// ── Expected-value helpers ──────────────────────────────────────────────────

/// Expected href attribute for a synthetic link at `index`.
fn expected_href(index: usize) -> String {
    format!("/post/{index}")
}

/// Expected inner HTML for a synthetic link at `index`.
///
/// SCaH preserves source-level entity encoding in inner HTML.
fn expected_link_inner_html(index: usize) -> String {
    format!("<b>Post</b> &lt;{index}&gt;")
}

/// Expected text content for a synthetic link at `index`.
///
/// SCaH preserves source-level entity encoding in text content.
fn expected_link_text(index: usize) -> String {
    format!("Post &lt;{index}&gt;")
}

// ── Save-mode field validation ─────────────────────────────────────────────

/// Validate `Save::none()` semantics: elements have counts and attributes but
/// no inner HTML or text content.
pub fn assert_save_none_result(store: &Store<'_, '_>, selector: &str, expected_count: usize) {
    assert!(
        expected_count > 0,
        "save_none: save validation requires non-empty input"
    );

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

    // Validate first representative element
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

    // Validate last representative element
    let last_index = expected_count - 1;
    let last = &elements[last_index];
    let expected_last_href = expected_href(last_index);
    assert!(
        last.inner_html.is_none(),
        "save_none: inner_html should be None for {selector:?}[{last_index}]"
    );
    assert!(
        last.text_content(store).is_none(),
        "save_none: text_content should be None for {selector:?}[{last_index}]"
    );
    assert_eq!(
        last.attribute(store, "href"),
        Some(expected_last_href.as_str()),
        "save_none: expected href={expected_last_href:?} for {selector:?}[{last_index}]"
    );
}

/// Validate `Save::only_inner_html()` semantics: inner HTML present, text absent.
///
/// Asserts exact inner HTML content for the first and last elements, catching
/// truncation, wrong element association, missing entity fragments, and stale indexes.
pub fn assert_save_inner_html_result(store: &Store<'_, '_>, selector: &str, expected_count: usize) {
    assert!(
        expected_count > 0,
        "save_inner_html: save validation requires non-empty input"
    );

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

    // Validate first element
    let first = &elements[0];
    let expected_inner = expected_link_inner_html(0);
    assert_eq!(
        first.inner_html,
        Some(expected_inner.as_str()),
        "save_inner_html: wrong inner HTML for {selector:?}[0]"
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

    // Validate last element
    let last_index = expected_count - 1;
    let last = &elements[last_index];
    let expected_inner = expected_link_inner_html(last_index);
    let expected_last_href = expected_href(last_index);
    assert_eq!(
        last.inner_html,
        Some(expected_inner.as_str()),
        "save_inner_html: wrong inner HTML for {selector:?}[{last_index}]"
    );
    assert!(
        last.text_content(store).is_none(),
        "save_inner_html: text_content should be None for {selector:?}[{last_index}]"
    );
    assert_eq!(
        last.attribute(store, "href"),
        Some(expected_last_href.as_str()),
        "save_inner_html: expected href={expected_last_href:?} for {selector:?}[{last_index}]"
    );
}

/// Validate `Save::only_text_content()` semantics: text present, inner HTML absent.
///
/// Asserts exact text content for the first and last elements. SCaH preserves
/// source-level entity encoding in text content (not decoded).
pub fn assert_save_text_result(store: &Store<'_, '_>, selector: &str, expected_count: usize) {
    assert!(
        expected_count > 0,
        "save_text: save validation requires non-empty input"
    );

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

    // Validate first element
    let first = &elements[0];
    let expected_text = expected_link_text(0);
    assert!(
        first.inner_html.is_none(),
        "save_text: inner_html should be None for {selector:?}[0]"
    );
    assert_eq!(
        first.text_content(store),
        Some(expected_text.as_str()),
        "save_text: wrong text content for {selector:?}[0]"
    );
    assert_eq!(
        first.attribute(store, "href"),
        Some("/post/0"),
        "save_text: expected href=/post/0 for {selector:?}[0]"
    );

    // Validate last element
    let last_index = expected_count - 1;
    let last = &elements[last_index];
    let expected_text = expected_link_text(last_index);
    let expected_last_href = expected_href(last_index);
    assert!(
        last.inner_html.is_none(),
        "save_text: inner_html should be None for {selector:?}[{last_index}]"
    );
    assert_eq!(
        last.text_content(store),
        Some(expected_text.as_str()),
        "save_text: wrong text content for {selector:?}[{last_index}]"
    );
    assert_eq!(
        last.attribute(store, "href"),
        Some(expected_last_href.as_str()),
        "save_text: expected href={expected_last_href:?} for {selector:?}[{last_index}]"
    );
}

/// Validate `Save::all()` semantics: both inner HTML and text content present.
///
/// Asserts exact inner HTML and text content for the first and last elements.
/// A regression that returns empty strings, truncated content, or incorrect
/// entity encoding will fail validation before timing begins.
pub fn assert_save_all_result(store: &Store<'_, '_>, selector: &str, expected_count: usize) {
    assert!(
        expected_count > 0,
        "save_all: save validation requires non-empty input"
    );

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

    // Validate first element
    let first = &elements[0];
    let expected_inner = expected_link_inner_html(0);
    let expected_text = expected_link_text(0);
    assert_eq!(
        first.inner_html,
        Some(expected_inner.as_str()),
        "save_all: wrong inner HTML for {selector:?}[0]"
    );
    assert_eq!(
        first.text_content(store),
        Some(expected_text.as_str()),
        "save_all: wrong text content for {selector:?}[0]"
    );
    assert_eq!(
        first.attribute(store, "href"),
        Some("/post/0"),
        "save_all: expected href=/post/0 for {selector:?}[0]"
    );

    // Validate last element
    let last_index = expected_count - 1;
    let last = &elements[last_index];
    let expected_inner = expected_link_inner_html(last_index);
    let expected_text = expected_link_text(last_index);
    let expected_last_href = expected_href(last_index);
    assert_eq!(
        last.inner_html,
        Some(expected_inner.as_str()),
        "save_all: wrong inner HTML for {selector:?}[{last_index}]"
    );
    assert_eq!(
        last.text_content(store),
        Some(expected_text.as_str()),
        "save_all: wrong text content for {selector:?}[{last_index}]"
    );
    assert_eq!(
        last.attribute(store, "href"),
        Some(expected_last_href.as_str()),
        "save_all: expected href={expected_last_href:?} for {selector:?}[{last_index}]"
    );
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
