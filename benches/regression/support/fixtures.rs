/// Generate HTML with a list of link elements.
///
/// Produces count `<a>` tags inside `<div class="article">` wrappers.
/// Each link has an href and inner text.
pub fn generate_link_list_html(count: usize) -> String {
    // ~100 bytes per element
    let mut html = String::with_capacity(count * 100 + 200);
    html.push_str("<html><body><div id='content'>");
    for i in 0..count {
        html.push_str(&format!(
            r#"<div class="article"><a href="/post/{i}"><b>Post</b> &lt;{i}&gt;</a></div>"#
        ));
    }
    html.push_str("</div></body></html>");
    html
}

/// Generate HTML for a product catalog with nested structure.
///
/// Each product has a title (`h1`), rating (`span.rating`), and description (`p.description`).
pub fn generate_product_catalog_html(count: usize) -> String {
    let mut html = String::with_capacity(count * 180 + 200);
    html.push_str(r#"<html><body><section id="products">"#);

    for i in 1..=count {
        let rating = ((i - 1) % 5) + 1;
        html.push_str(&format!(
            r#"<div class="product"><h1>Product #{i}</h1><span class="rating">{rating}/5</span><p class="description">Description #{i}</p></div>"#
        ));
    }

    html.push_str("</section></body></html>");
    html
}

/// Generate HTML for first-match placement benchmarks.
///
/// Creates `count` repeated `<a>` elements. If `match_position` is `Some(pos)`,
/// the element at 0-indexed position `pos` gets `class="target"` instead of
/// the default class. If `None`, no element gets the target class (no-match scenario).
///
/// # Panics
///
/// Panics if `match_position` is `Some(pos)` and `pos >= count`.
pub fn generate_first_match_html(count: usize, match_position: Option<usize>) -> String {
    if let Some(pos) = match_position {
        assert!(
            pos < count,
            "match_position {pos} out of range for count {count}"
        );
    }

    let mut html = String::with_capacity(count * 100 + 200);
    html.push_str("<html><body><div id='content'>");
    for i in 0..count {
        if Some(i) == match_position {
            html.push_str(&format!(
                r#"<div class="article"><a class="target" href="/post/{i}">Post {i}</a></div>"#
            ));
        } else {
            html.push_str(&format!(
                r#"<div class="article"><a href="/post/{i}">Post {i}</a></div>"#
            ));
        }
    }
    html.push_str("</div></body></html>");
    html
}

/// Generate HTML for multi-query pressure benchmarks.
///
/// Creates `element_count` repeated `<div>` elements, each with a unique class
/// from `class-0` through `class-{query_count-1}` cycling. Each element also
/// carries a `data-index` attribute matching its 0-based position, enabling
/// validation to verify result identity without relying on nested text.
pub fn generate_multi_query_html(element_count: usize, query_count: usize) -> String {
    let mut html = String::with_capacity(element_count * 120 + 200);
    html.push_str("<html><body><div id='content'>");
    for i in 0..element_count {
        let class_idx = i % query_count;
        html.push_str(&format!(
            r#"<div class="item class-{class_idx}" data-index="{i}"><span>Item {i}</span></div>"#
        ));
    }
    html.push_str("</div></body></html>");
    html
}

// ── Expected-value helpers for product catalog ──────────────────────────────

/// Expected title text for product number `n` (1-based).
pub fn expected_product_title(n: usize) -> String {
    format!("Product #{n}")
}

/// Expected rating text for product at 0-based `index`.
pub fn expected_product_rating(index: usize) -> String {
    let rating = (index % 5) + 1;
    format!("{rating}/5")
}

/// Expected description text for product number `n` (1-based).
pub fn expected_product_description(n: usize) -> String {
    format!("Description #{n}")
}

/// Expected inner HTML for a product `<div>` at 0-based `index`.
pub fn expected_product_inner_html(index: usize) -> String {
    let n = index + 1;
    let rating = (index % 5) + 1;
    format!(
        "<h1>Product #{n}</h1><span class=\"rating\">{rating}/5</span><p class=\"description\">Description #{n}</p>"
    )
}

/// Expected text content for a product `<div>` at 0-based `index`.
///
/// SCaH concatenates child text nodes with single spaces.
pub fn expected_product_text(index: usize) -> String {
    let n = index + 1;
    let rating = (index % 5) + 1;
    format!("Product #{n} {rating}/5 Description #{n}")
}
