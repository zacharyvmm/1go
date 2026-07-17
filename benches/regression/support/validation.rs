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

/// Validate `Save::none()` semantics: every element has correct href, no inner
/// HTML, and no text content. All results are exhaustively enumerated — a
/// regression that drops saved data for middle elements will fail.
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

    for (index, element) in elements.iter().enumerate() {
        assert!(
            element.inner_html.is_none(),
            "save_none: inner_html should be None for {selector:?}[{index}]"
        );
        assert!(
            element.text_content(store).is_none(),
            "save_none: text_content should be None for {selector:?}[{index}]"
        );
        let expected_href = expected_href(index);
        assert_eq!(
            element.attribute(store, "href"),
            Some(expected_href.as_str()),
            "save_none: expected href={expected_href:?} for {selector:?}[{index}]"
        );
    }
}


/// Validate `Save::only_inner_html()` semantics: every element has exact inner
/// HTML, correct href, and no text content. All results are exhaustively
/// enumerated — a regression that drops inner HTML for middle elements will fail.
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

    for (index, element) in elements.iter().enumerate() {
        let expected_inner = expected_link_inner_html(index);
        assert_eq!(
            element.inner_html,
            Some(expected_inner.as_str()),
            "save_inner_html: wrong inner HTML for {selector:?}[{index}]"
        );
        assert!(
            element.text_content(store).is_none(),
            "save_inner_html: text_content should be None for {selector:?}[{index}]"
        );
        let expected_href = expected_href(index);
        assert_eq!(
            element.attribute(store, "href"),
            Some(expected_href.as_str()),
            "save_inner_html: expected href={expected_href:?} for {selector:?}[{index}]"
        );
    }
}


/// Validate `Save::only_text_content()` semantics: every element has exact text
/// content, correct href, and no inner HTML. All results are exhaustively
/// enumerated — a regression that drops text for middle elements will fail.
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

    for (index, element) in elements.iter().enumerate() {
        assert!(
            element.inner_html.is_none(),
            "save_text: inner_html should be None for {selector:?}[{index}]"
        );
        let expected_text = expected_link_text(index);
        assert_eq!(
            element.text_content(store),
            Some(expected_text.as_str()),
            "save_text: wrong text content for {selector:?}[{index}]"
        );
        let expected_href = expected_href(index);
        assert_eq!(
            element.attribute(store, "href"),
            Some(expected_href.as_str()),
            "save_text: expected href={expected_href:?} for {selector:?}[{index}]"
        );
    }
}

/// Validate `Save::all()` semantics: every element has exact inner HTML, exact
/// text content, and correct href. All results are exhaustively enumerated — a
/// regression that drops saved data for middle elements will fail.
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

    for (index, element) in elements.iter().enumerate() {
        let expected_inner = expected_link_inner_html(index);
        let expected_text = expected_link_text(index);
        assert_eq!(
            element.inner_html,
            Some(expected_inner.as_str()),
            "save_all: wrong inner HTML for {selector:?}[{index}]"
        );
        assert_eq!(
            element.text_content(store),
            Some(expected_text.as_str()),
            "save_all: wrong text content for {selector:?}[{index}]"
        );
        let expected_href = expected_href(index);
        assert_eq!(
            element.attribute(store, "href"),
            Some(expected_href.as_str()),
            "save_all: expected href={expected_href:?} for {selector:?}[{index}]"
        );
    }
}

// ── Nested product catalog validation ──────────────────────────────────────

/// Validate nested_all product results: parent count, parent saved data, every
/// child present, child saved data, and child class tokens. All products are
/// exhaustively validated — a regression that drops parent inner HTML or child
/// saved fields for middle products will fail.
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
        // ── Parent validation ──────────────────────────────────────────
        assert_eq!(
            product.class,
            Some("product"),
            "product[{index}]: expected class 'product'"
        );

        let expected_inner = super::fixtures::expected_product_inner_html(index);
        assert_eq!(
            product.inner_html,
            Some(expected_inner.as_str()),
            "product[{index}]: wrong parent inner HTML"
        );

        let expected_text = super::fixtures::expected_product_text(index);
        assert_eq!(
            product.text_content(store),
            Some(expected_text.as_str()),
            "product[{index}]: wrong parent text content"
        );

        // ── Child validation ───────────────────────────────────────────
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
            titles.len(), 1,
            "product[{index}]: expected 1 title, got {}",
            titles.len()
        );
        assert_eq!(
            ratings.len(), 1,
            "product[{index}]: expected 1 rating, got {}",
            ratings.len()
        );
        assert_eq!(
            descriptions.len(), 1,
            "product[{index}]: expected 1 description, got {}",
            descriptions.len()
        );

        let product_number = index + 1;

        // Title: text + inner HTML (should match since no nested markup) + no
        // unexpected class.
        let expected_title = super::fixtures::expected_product_title(product_number);
        assert_eq!(
            titles[0].text_content(store),
            Some(expected_title.as_str()),
            "product[{index}]: wrong title text"
        );
        assert_eq!(
            titles[0].inner_html,
            Some(expected_title.as_str()),
            "product[{index}]: wrong title inner HTML"
        );
        assert!(
            titles[0].class.is_none(),
            "product[{index}]: title should have no class"
        );

        // Rating: text + inner HTML + class.
        let expected_rating = super::fixtures::expected_product_rating(index);
        assert_eq!(
            ratings[0].text_content(store),
            Some(expected_rating.as_str()),
            "product[{index}]: wrong rating text"
        );
        assert_eq!(
            ratings[0].inner_html,
            Some(expected_rating.as_str()),
            "product[{index}]: wrong rating inner HTML"
        );
        assert_eq!(
            ratings[0].class,
            Some("rating"),
            "product[{index}]: expected rating class 'rating'"
        );

        // Description: text + inner HTML + class.
        let expected_desc = super::fixtures::expected_product_description(product_number);
        assert_eq!(
            descriptions[0].text_content(store),
            Some(expected_desc.as_str()),
            "product[{index}]: wrong description text"
        );
        assert_eq!(
            descriptions[0].inner_html,
            Some(expected_desc.as_str()),
            "product[{index}]: wrong description inner HTML"
        );
        assert_eq!(
            descriptions[0].class,
            Some("description"),
            "product[{index}]: expected description class 'description'"
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

/// Validate multi-query results: each selector's exact count, returned element
/// identity via data-index, class tokens, inner HTML, and text content. All
/// results are exhaustively validated — a regression that drops saved fields
/// or returns elements under the wrong selector will fail.
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

            // Validate data-index attribute for identity.
            let data_index: usize = element
                .attribute(store, "data-index")
                .expect("element should have data-index attribute")
                .parse()
                .expect("data-index should be a valid usize");

            // The element's class must be consistent with its data-index.
            let data_index_class = format!("class-{}", data_index % query_count);
            assert!(
                classes
                    .split_ascii_whitespace()
                    .any(|class| class == data_index_class),
                "selector {selector}: element data-index={data_index} has classes {classes:?}, \
                 expected class {data_index_class}"
            );
            assert_eq!(
                data_index_class, expected_class,
                "selector {selector}: element data-index={data_index} belongs to class \
                 {data_index_class}, not {expected_class}"
            );

            // Validate exact saved fields.
            let expected_inner = format!("<span>Item {data_index}</span>");
            assert_eq!(
                element.inner_html,
                Some(expected_inner.as_str()),
                "selector {selector}: wrong inner HTML for data-index={data_index}"
            );
            let expected_text = format!("Item {data_index}");
            assert_eq!(
                element.text_content(store),
                Some(expected_text.as_str()),
                "selector {selector}: wrong text content for data-index={data_index}"
            );
        }

        total += elements.len();
    }

    assert_eq!(
        total, element_count,
        "total across all selectors should equal {element_count}, got {total}"
    );
}
