use std::ops::{Deref, Range};

use super::arena::{Arena, Node, id};
use super::{Attribute, Store};

/// Per-element raw/normalized text ranges stored beside the element arena.
///
/// Kept out of [`Element`] so inner-HTML-only and no-content matches do not
/// pay for unused `Option<Range<usize>>` fields on every result record.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub(crate) struct ElementTextRanges {
    pub(crate) raw_text: Option<Range<usize>>,
    pub(crate) text: Option<Range<usize>>,
}

/// A matched HTML element stored in the [`Store`](crate::Store).
///
/// Each `Element` represents one HTML tag that was captured during parsing.
/// It holds zero-copy `&str` references into the original HTML source for
/// its name, class, id, and inner HTML.
///
/// Text ranges live in a store-owned sidecar and are accessed through
/// [`Element::raw_text`] / [`Element::text`] (not as fields on this struct).
///
/// # Accessing Data
///
/// | Data | How to access |
/// |------|---------------|
/// | Tag name | `element.name` |
/// | Class | `element.class` |
/// | ID | `element.id` |
/// | Inner HTML | `element.inner_html` |
/// | Text content | [`element.text(&store)`](Element::text) |
/// | Raw text | [`element.raw_text(&store)`](Element::raw_text) |
/// | All attributes | [`element.attributes(&store)`](Element::attributes) |
/// | Single attribute | [`element.attribute(&store, "href")`](Element::attribute) |
/// | Child query results | [`element.get(&store, "selector")`](Element::get) |
#[derive(Default, Debug, PartialEq)]
pub struct Element<'html> {
    /// The tag name (e.g. `"a"`, `"div"`, `"section"`).
    pub name: &'html str,
    /// The value of the `class` attribute, if present.
    pub class: Option<&'html str>,
    /// The value of the `id` attribute, if present.
    pub id: Option<&'html str>,
    /// The raw HTML between the element's opening and closing tags.
    /// Only populated when [`Save::inner_html`](crate::Save::inner_html) was `true`.
    pub inner_html: Option<&'html str>,
    /// Internal range into the attribute arena.
    /// Use [`Element::attributes`] or [`Element::attribute`] instead.
    pub attributes: Option<Range<u32>>,

    pub first_child_query: Option<id::QueryId>,
    pub next_sibling: Option<id::ElementId>,
}

impl<'html> Node<id::ElementId> for Element<'html> {
    fn next_sibling(&self) -> Option<id::ElementId> {
        self.next_sibling
    }
}

impl<'html> Element<'html> {
    pub fn iter(
        &self,
        arena: &'html Arena<Element<'html>, id::ElementId>,
    ) -> impl Iterator<Item = &'html Element<'html>> {
        let index = unsafe { arena.index_of(self) };
        arena.iter_from(index)
    }

    /// Look up child elements matched by a **nested query** (one added
    /// via [`QueryBuilder::then`](crate::QueryBuilder::then)).
    ///
    /// The `key` parameter is the CSS selector string of the child query.
    ///
    /// Returns `None` if this element has no nested query results for the
    /// given selector.
    pub fn get(
        &self,
        dom: &'html Store,
        key: &str,
    ) -> Option<impl Iterator<Item = &'html Element<'html>>> {
        let first_query_id = self.first_child_query;
        first_query_id
            .and_then(|id| dom.queries.iter_from(id).find(|q| q.query == key))
            .map(|query_node| query_node.elements.start())
            .map(|element_id| dom.elements.iter_from(element_id))
    }

    /// Return all attributes of this element as a slice.
    ///
    /// Returns `None` if the element had no extra attributes beyond
    /// `class` and `id` (which are stored directly on the [`Element`]).
    pub fn attributes(&self, dom: &'html Store) -> Option<&'html [Attribute<'html>]> {
        self.attributes
            .as_ref()
            .map(|range| &dom.attributes.deref()[(range.start as usize)..(range.end as usize)])
    }
    /// Look up a single attribute value by name.
    ///
    /// Returns the attribute's value, or `None` if the attribute is not
    /// present or has no value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use scah::{Query, Save, parse};
    ///
    /// let html = r#"<a href="https://example.com">Link</a>"#;
    /// let queries = &[Query::all("a", Save::all())
    ///     .expect("valid selector")
    ///     .build()];
    /// let store = parse(html, queries).expect("parse succeeds");
    ///
    /// let a = store.get("a").unwrap().next().unwrap();
    /// assert_eq!(a.attribute(&store, "href"), Some("https://example.com"));
    /// ```
    pub fn attribute(&self, dom: &'html Store, key: &str) -> Option<&'html str> {
        self.attributes.as_ref().and_then(|range| {
            dom.attributes.deref()[(range.start as usize)..(range.end as usize)]
                .iter()
                .find(|attr| attr.key.eq_ignore_ascii_case(key))
                .and_then(|kv| kv.value)
        })
    }
    /// Get the element's source-preserving descendant text.
    ///
    /// Only populated when [`Save::raw_text`](crate::Save::raw_text) was `true`.
    pub fn raw_text(&self, dom: &'html Store) -> Option<&'html str> {
        let element_id = unsafe { dom.elements.index_of(self) };
        dom.element_text_range(element_id)
            .and_then(|ranges| ranges.raw_text.as_ref())
            .map(|range| dom.text.raw_text.slice(range.clone()))
    }

    /// Get the element's normalized, human-readable descendant text.
    ///
    /// Returns the whitespace-trimmed, concatenated text nodes within
    /// this element. Only populated when [`Save::text`](crate::Save::text) was `true`.
    pub fn text(&self, dom: &'html Store) -> Option<&'html str> {
        let element_id = unsafe { dom.elements.index_of(self) };
        dom.element_text_range(element_id)
            .and_then(|ranges| ranges.text.as_ref())
            .map(|range| dom.text.text.slice(range.clone()))
    }

    /// Returns whether this element captured a raw-text range.
    ///
    /// Distinguishes uncaptured content (`false`) from captured empty
    /// content (`true` with [`Element::raw_text`] returning `Some("")`).
    pub fn has_raw_text(&self, dom: &Store<'_, '_>) -> bool {
        let element_id = unsafe { dom.elements.index_of(self) };
        dom.element_text_range(element_id)
            .is_some_and(|ranges| ranges.raw_text.is_some())
    }

    /// Returns whether this element captured a normalized-text range.
    ///
    /// Distinguishes uncaptured content (`false`) from captured empty
    /// content (`true` with [`Element::text`] returning `Some("")`).
    pub fn has_text(&self, dom: &Store<'_, '_>) -> bool {
        let element_id = unsafe { dom.elements.index_of(self) };
        dom.element_text_range(element_id)
            .is_some_and(|ranges| ranges.text.is_some())
    }
}
