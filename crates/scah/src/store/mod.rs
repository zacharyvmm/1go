use crate::Attribute;
use crate::QuerySection;
use std::ops::Range;

mod text_content;
pub(crate) use text_content::TextContent;
mod arena;
mod attributes;
mod element;
mod query_node;

pub(crate) use arena::id::Nullable;
use arena::span::Span;
pub use arena::{
    Arena,
    id::{AttributeId, ElementId, QueryId},
};

pub use element::Element;
pub use query_node::QueryNode;

/// The result set returned by [`parse`](crate::parse).
///
/// A `Store` is an arena-based container that holds all elements, attributes,
/// and text content captured during parsing. You query it by CSS selector
/// string using [`Store::get`].
///
/// # Example
///
/// ```rust
/// use scah::{Query, Save, parse};
///
/// let html = "<div><a href='x'>Link1</a><a href='y'>Link2</a></div>";
/// let queries = &[Query::all("a", Save::all())
///     .expect("valid selector")
///     .build()];
/// let store = parse(html, queries).expect("parse succeeds");
///
/// // Retrieve all matched <a> elements
/// let anchors: Vec<_> = store.get("a").unwrap().collect();
/// assert_eq!(anchors.len(), 2);
///
/// // Access attributes
/// assert_eq!(anchors[0].attribute(&store, "href"), Some("x"));
/// ```
#[derive(Debug, PartialEq)]
pub struct Store<'html, 'query> {
    /// Arena of matched elements.
    pub elements: Arena<Element<'html>, ElementId>,
    /// Arena of attributes belonging to matched elements.
    pub attributes: Arena<Attribute<'html>, AttributeId>,
    /// Arena of query nodes that link selectors to their matched elements.
    pub queries: Arena<QueryNode<'query>, QueryId>,
    /// Accumulated text-content buffer shared by all elements.
    pub text_content: TextContent,
    #[cfg(any(debug_assertions, test))]
    pub trace: crate::debug::TraceStore<'html, 'query>,
}

/// Advanced allocation tuning for [`Store::with_capacity_options`].
///
/// Controls how the `Store` pre-allocates arena capacity from a total
/// HTML byte-length hint. The defaults are the optimized parser/store
/// heuristics used by [`Store::with_capacity`] and are suitable for the
/// vast majority of workloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapacityOptions {
    /// Approximate HTML bytes per reserved element slot.
    ///
    /// Default: 48 (derived from valgrind massif profiling).
    pub element_bytes_per_slot: usize,

    /// Approximate HTML bytes per reserved attribute slot.
    ///
    /// Default: 24.
    pub attribute_bytes_per_slot: usize,

    /// Whether to reserve the text-content buffer using the full input
    /// capacity.  Set to `false` when no query needs text content.
    ///
    /// Default: `true`, preserving the public [`Store::with_capacity`]
    /// behaviour of reserving text-content storage.
    pub reserve_text_content: bool,

    /// Maximum trace-log preallocation.  Only used behind
    /// `cfg(any(debug_assertions, test))`; harmless otherwise.
    ///
    /// Default: 4096.
    pub trace_capacity_limit: usize,
}

impl Default for CapacityOptions {
    fn default() -> Self {
        Self {
            element_bytes_per_slot: 48,
            attribute_bytes_per_slot: 24,
            reserve_text_content: true,
            trace_capacity_limit: 4096,
        }
    }
}

impl<'html, 'query: 'html> Default for Store<'html, 'query> {
    fn default() -> Self {
        Self {
            elements: Arena::new(),
            queries: Arena::new(),
            text_content: TextContent::new(),
            attributes: Arena::new(),
            #[cfg(any(debug_assertions, test))]
            trace: crate::debug::TraceStore::new(),
        }
    }
}

impl<'html, 'query: 'html> Store<'html, 'query> {
    /// Creates a `Store` with pre-allocated capacity for the arenas.
    ///
    /// The `capacity` parameter is the total HTML byte length. From this we
    /// derive conservative reservations for the element and attribute arenas
    /// using the default [`CapacityOptions`].
    ///
    /// This is the non-breaking public API. For advanced tuning (e.g.
    /// skipping text-content reservation or adjusting element/attribute
    /// ratios), use [`Store::with_capacity_options`].
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_capacity_options(capacity, CapacityOptions::default())
    }

    /// Creates a `Store` with pre-allocated capacity controlled by
    /// [`CapacityOptions`].
    ///
    /// # Allocation policy
    ///
    /// | Arena        | Reservation                        |
    /// |------------- |----------------------------------- |
    /// | elements     | `capacity / element_bytes_per_slot` |
    /// | attributes   | `capacity / attribute_bytes_per_slot`|
    /// | text_content | `capacity` if `reserve_text_content` |
    /// | queries      | none (fixed small per-query alloc)   |
    ///
    /// Every divisor is clamped to at least 1 to avoid division by zero.
    pub fn with_capacity_options(capacity: usize, options: CapacityOptions) -> Self {
        Self::with_capacity_requirements(capacity, options, true)
    }

    pub(crate) fn with_capacity_requirements(
        capacity: usize,
        options: CapacityOptions,
        reserve_attributes: bool,
    ) -> Self {
        let element_divisor = options.element_bytes_per_slot.max(1);
        let attribute_divisor = options.attribute_bytes_per_slot.max(1);

        Self {
            elements: Arena::with_capacity(capacity / element_divisor),
            queries: Arena::new(),
            text_content: if options.reserve_text_content {
                TextContent::with_capacity(capacity)
            } else {
                TextContent::new()
            },
            attributes: if reserve_attributes {
                Arena::with_capacity(capacity / attribute_divisor)
            } else {
                Arena::new()
            },
            #[cfg(any(debug_assertions, test))]
            trace: crate::debug::TraceStore::with_capacity(
                (capacity / element_divisor).min(options.trace_capacity_limit),
            ),
        }
    }

    #[inline(always)]
    #[cfg_attr(not(any(debug_assertions, test)), allow(dead_code))]
    pub(crate) fn trace_event(
        &mut self,
        #[cfg(any(debug_assertions, test))] event: crate::debug::TraceEvent<'html, 'query>,
    ) {
        #[cfg(any(debug_assertions, test))]
        {
            self.trace.push(event);
        }
    }

    /// Look up all elements that matched a given CSS selector string.
    ///
    /// The `query` parameter must be the **exact same string** used when
    /// building the [`Query`](crate::Query) (e.g. `"main > section > a[href]"`).
    ///
    /// Returns `None` if no elements were matched by any query, or if
    /// the given selector string was not part of the executed queries.
    ///
    /// # Example
    ///
    /// ```rust
    /// use scah::{Query, Save, parse};
    ///
    /// let html = "<ul><li>A</li><li>B</li></ul>";
    /// let queries = &[Query::all("li", Save::only_text_content())
    ///     .expect("valid selector")
    ///     .build()];
    /// let store = parse(html, queries).expect("parse succeeds");
    ///
    /// for li in store.get("li").unwrap() {
    ///     println!("{}", li.text_content(&store).unwrap_or_default());
    /// }
    /// ```
    pub fn get(&'html self, query: &str) -> Option<impl Iterator<Item = &'html Element<'html>>> {
        self.result_meta(query)
            .map(|(first, _)| self.elements.iter_from(first))
    }

    /// Resolve the first match id and length for `query` in O(number of query
    /// nodes), without walking the result linked list.
    ///
    /// Returns `None` when the selector has no stored matches.
    pub fn result_meta(&self, query: &str) -> Option<(ElementId, usize)> {
        self.result_ids(query).map(|ids| (ids[0], ids.len()))
    }

    /// Borrow the contiguous match-id list for `query`.
    pub fn result_ids(&self, query: &str) -> Option<&[ElementId]> {
        if self.queries.is_empty() {
            return None;
        }

        self.queries
            .iter_from(QueryId(0))
            .find(|q| q.query == query)
            .map(|query_node| query_node.match_ids.as_slice())
            .filter(|ids| !ids.is_empty())
    }

    fn link_query_to_query(&mut self, query: QueryId, mut root: QueryId) {
        loop {
            if root == query {
                return;
            }
            let query_node = &self.queries[root];
            match query_node.next_sibling {
                Some(sibling) => root = sibling,
                None => {
                    self.queries[root].next_sibling = Some(query);
                    break;
                }
            }
        }
    }

    fn link_query_to_element(&mut self, query: QueryId, element: ElementId) {
        let id = self.elements[element].first_child_query;

        match id {
            Some(id) => {
                self.link_query_to_query(query, id);
            }
            None => {
                self.elements[element].first_child_query = Some(query);
            }
        }
    }

    fn link_element_to_query(&mut self, query: QueryId, element: ElementId) {
        let id = self.queries[query].elements.end();

        if id == element {
            return;
        }

        assert!(self.elements[id].next_sibling.is_none());
        self.elements[id].next_sibling = Some(element);
        self.queries[query].elements.set_end(element);
        self.queries[query].match_count += 1;
        self.queries[query].match_ids.push(element);
    }

    pub fn push(
        &mut self,
        from: ElementId,
        selection: &QuerySection<'query>,
        element: crate::XHtmlElement<'html>,
    ) -> ElementId {
        let new_element = Element {
            name: element.name,
            class: selection.save.attributes.then_some(element.class).flatten(),
            id: selection.save.attributes.then_some(element.id).flatten(),
            attributes: if selection.save.attributes {
                self.attributes.attribute_slice_to_range(element.attributes)
            } else {
                None
            },
            ..Default::default()
        };

        assert!(from.is_null() || from.0 < self.elements.len());

        let existing_id = {
            if !from.is_null() {
                self.elements[from].first_child_query.and_then(|query| {
                    self.queries
                        .iter_from(query)
                        .find(|q| q.query == selection.source)
                        .map(|q| unsafe { self.queries.index_of(q) })
                })
            } else if !self.queries.is_empty() {
                self.queries
                    .iter_from(QueryId(0))
                    .find(|q| q.query == selection.source)
                    .map(|q| unsafe { self.queries.index_of(q) })
            } else {
                None
            }
        };

        let index = ElementId(self.elements.len());
        self.elements.push(new_element);

        let query_id = match existing_id {
            Some(id) => id,
            None => {
                self.queries.push(QueryNode {
                    query: selection.source,
                    elements: Span::new(index),
                    next_sibling: None,
                    match_count: 1,
                    match_ids: vec![index],
                });

                QueryId(self.queries.len() - 1)
            }
        };

        assert!(!self.queries.is_empty());
        assert!(query_id.index() < self.queries.len());

        if !from.is_null() {
            self.link_query_to_element(query_id, from);
        } else {
            self.link_query_to_query(query_id, QueryId(0));
        }

        self.link_element_to_query(query_id, index);

        //let query = &mut self.queries[query_id.0];

        // let element = &mut self.elements[query.last_element.0];
        // element.next_sibling = Some(index);
        // children.last_element = index;

        index
    }

    pub fn set_content(
        &mut self,
        element_id: ElementId,
        inner_html: Option<&'html str>,
        text_content: Option<Range<usize>>,
    ) {
        assert!(!self.elements.is_empty());
        assert!(element_id.index() < self.elements.len());

        #[cfg(any(debug_assertions, test))]
        let tag = self.elements[element_id].name;
        #[cfg(any(debug_assertions, test))]
        let has_inner_html = inner_html.is_some();
        #[cfg(any(debug_assertions, test))]
        let has_text_content = text_content.is_some();

        let element = &mut self.elements[element_id];
        element.inner_html = inner_html;
        element.text_content = text_content;

        crate::scah_trace!(
            self,
            crate::debug::TraceEvent::ContentFinalized {
                element_id,
                tag,
                has_inner_html,
                has_text_content,
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Query, Save};

    #[test]
    fn with_capacity_reserves_arenas_conservatively() {
        let store = Store::with_capacity(30_000);

        assert_eq!(store.elements.capacity(), 30_000 / 48);
        assert_eq!(store.attributes.capacity(), 30_000 / 24);
        assert_eq!(store.text_content.content.capacity(), 30_000);
    }

    #[test]
    fn with_capacity_options_can_skip_text_content_reservation() {
        let store = Store::with_capacity_options(
            30_000,
            CapacityOptions {
                reserve_text_content: false,
                ..CapacityOptions::default()
            },
        );

        assert_eq!(store.elements.capacity(), 30_000 / 48);
        assert_eq!(store.attributes.capacity(), 30_000 / 24);
        assert_eq!(store.text_content.content.capacity(), 0);
    }

    #[test]
    fn with_capacity_options_can_reserve_text_content() {
        let store = Store::with_capacity_options(
            30_000,
            CapacityOptions {
                reserve_text_content: true,
                ..CapacityOptions::default()
            },
        );

        assert_eq!(store.elements.capacity(), 30_000 / 48);
        assert_eq!(store.attributes.capacity(), 30_000 / 24);
        assert_eq!(store.text_content.content.capacity(), 30_000);
    }

    #[test]
    fn with_capacity_options_uses_custom_element_and_attribute_ratios() {
        let store = Store::with_capacity_options(
            1200,
            CapacityOptions {
                element_bytes_per_slot: 12,
                attribute_bytes_per_slot: 6,
                ..CapacityOptions::default()
            },
        );

        assert_eq!(store.elements.capacity(), 100);
        assert_eq!(store.attributes.capacity(), 200);
    }

    #[test]
    fn with_capacity_options_handles_zero_ratios_without_panicking() {
        let store = Store::with_capacity_options(
            16,
            CapacityOptions {
                element_bytes_per_slot: 0,
                attribute_bytes_per_slot: 0,
                ..CapacityOptions::default()
            },
        );

        assert_eq!(store.elements.capacity(), 16);
        assert_eq!(store.attributes.capacity(), 16);
    }

    #[test]
    fn test_find_next_query() {
        let mut store = Store::default();
        store.queries.inner = vec![
            QueryNode {
                query: "1",
                next_sibling: Some(QueryId(1)),
                ..Default::default()
            },
            QueryNode {
                query: "2",
                next_sibling: Some(QueryId(2)),
                ..Default::default()
            },
            QueryNode {
                query: "3",
                next_sibling: Some(QueryId(3)),
                ..Default::default()
            },
            QueryNode {
                // Shouldn't be possible, but still a giid test
                query: "3",
                next_sibling: None,
                ..Default::default()
            },
        ];

        assert_eq!(
            store
                .queries
                .iter_from(QueryId(0))
                .find(|q| q.query == "1")
                .map(|q| unsafe { store.queries.index_of(q) }),
            Some(QueryId(0))
        );
        assert_eq!(
            store
                .queries
                .iter_from(QueryId(0))
                .find(|q| q.query == "2")
                .map(|q| unsafe { store.queries.index_of(q) }),
            Some(QueryId(1))
        );
        assert_eq!(
            store
                .queries
                .iter_from(QueryId(0))
                .find(|q| q.query == "3")
                .map(|q| unsafe { store.queries.index_of(q) }),
            Some(QueryId(2))
        );
        assert_eq!(
            store
                .queries
                .iter_from(QueryId(0))
                .find(|q| q.query == "not in list")
                .map(|q| unsafe { store.queries.index_of(q) }),
            None
        );
    }

    #[test]
    fn test_link_query_to_element() {
        let mut store = Store::default();

        store.elements.inner = vec![
            Element {
                first_child_query: Some(QueryId(0)),
                ..Default::default()
            },
            Element {
                first_child_query: None,
                ..Default::default()
            },
        ];

        store.queries.inner = vec![
            QueryNode {
                next_sibling: Some(QueryId(1)),
                ..Default::default()
            },
            QueryNode {
                next_sibling: None,
                ..Default::default()
            },
        ];

        store.link_query_to_element(QueryId(0), ElementId(1));

        assert_eq!(
            store.queries.inner,
            vec![
                QueryNode {
                    next_sibling: Some(QueryId(1)),
                    ..Default::default()
                },
                QueryNode {
                    next_sibling: None,
                    // first_element: ElementId(1),
                    // last_element: ElementId(1),
                    ..Default::default()
                }
            ]
        );
    }

    #[test]
    fn test_branching_next_query() {
        let mut store = Store::default();

        let q = Query::all("1", Save::all())
            .unwrap()
            .then(|ctx| Ok([ctx.all("2", Save::all())?, ctx.all("3", Save::all())?]))
            .unwrap();

        // `1` MATCH
        store.push(
            ElementId::default(),
            &q.selection[0],
            crate::XHtmlElement::default(),
        );

        assert_eq!(
            store.queries.inner,
            vec![QueryNode {
                query: "1",
                next_sibling: None,
                elements: Span::new(ElementId(0)),
                match_count: 1,
                match_ids: vec![ElementId(0)],
            }]
        );

        assert_eq!(store.elements.inner, vec![Element::default(),]);

        // `2` MATCH
        store.push(
            ElementId(0),
            &q.selection[1],
            crate::XHtmlElement::default(),
        );

        assert_eq!(
            store.queries.inner,
            vec![
                QueryNode {
                    query: "1",
                    next_sibling: None,
                    elements: Span::new(ElementId(0)),
                    match_count: 1,
                    match_ids: vec![ElementId(0)],
                },
                QueryNode {
                    query: "2",
                    next_sibling: None,
                    elements: Span::new(ElementId(1)),
                    match_count: 1,
                    match_ids: vec![ElementId(1)],
                }
            ]
        );

        assert_eq!(
            store.elements.inner,
            vec![
                Element {
                    first_child_query: Some(QueryId(1)),
                    ..Default::default()
                },
                Element {
                    ..Default::default()
                },
            ]
        );

        // `3` MATCH
        store.push(
            ElementId(0),
            &q.selection[2],
            crate::XHtmlElement::default(),
        );

        assert_eq!(
            store.queries.inner,
            vec![
                QueryNode {
                    query: "1",
                    next_sibling: None,
                    elements: Span::new(ElementId(0)),
                    match_count: 1,
                    match_ids: vec![ElementId(0)],
                },
                QueryNode {
                    query: "2",
                    next_sibling: Some(QueryId(2)),
                    elements: Span::new(ElementId(1)),
                    match_count: 1,
                    match_ids: vec![ElementId(1)],
                },
                QueryNode {
                    query: "3",
                    next_sibling: None,
                    elements: Span::new(ElementId(2)),
                    match_count: 1,
                    match_ids: vec![ElementId(2)],
                }
            ]
        );

        assert_eq!(
            store.elements.inner,
            vec![
                Element {
                    first_child_query: Some(QueryId(1)),
                    ..Default::default()
                },
                Element {
                    ..Default::default()
                },
                Element {
                    ..Default::default()
                },
            ]
        );
    }
    #[test]
    fn test_push_multi_section() {
        let query = Query::all("main > section", Save::all())
            .unwrap()
            .then(|section| {
                Ok([
                    section.all("> a[href]", Save::all())?,
                    section.all("div a", Save::all())?,
                ])
            })
            .unwrap()
            .build();

        let mut store = Store::default();

        store.push(
            ElementId::default(),
            &query.queries[0],
            crate::XHtmlElement {
                name: "section",
                ..Default::default()
            },
        );

        assert_eq!(
            store
                .queries
                .iter_from(QueryId(0))
                .find(|q| q.query == query.queries[0].source)
                .map(|q| unsafe { store.queries.index_of(q) }),
            Some(QueryId(0))
        );

        store.push(
            ElementId::default(),
            &query.queries[0],
            crate::XHtmlElement {
                name: "section",
                ..Default::default()
            },
        );

        assert_eq!(
            store.elements.inner,
            vec![
                Element {
                    name: "section",
                    next_sibling: Some(ElementId(1)),
                    ..Default::default()
                },
                Element {
                    name: "section",
                    ..Default::default()
                },
            ]
        );

        assert_eq!(
            store.queries.inner,
            vec![QueryNode {
                query: "main > section",
                next_sibling: None,
                elements: Span::from(ElementId(0), ElementId(1)),
                match_count: 2,
                match_ids: vec![ElementId(0), ElementId(1)],
            },]
        );

        store.push(
            ElementId(1),
            &query.queries[1],
            crate::XHtmlElement {
                name: "a",
                ..Default::default()
            },
        );

        assert_eq!(
            store.queries.inner,
            vec![
                QueryNode {
                    query: "main > section",
                    next_sibling: None,
                    elements: Span::from(ElementId(0), ElementId(1)),
                    match_count: 2,
                    match_ids: vec![ElementId(0), ElementId(1)],
                },
                QueryNode {
                    query: "> a[href]",
                    next_sibling: None,
                    elements: Span::new(ElementId(2)),
                    match_count: 1,
                    match_ids: vec![ElementId(2)],
                }
            ]
        );

        assert_eq!(
            store.elements.inner,
            vec![
                Element {
                    name: "section",
                    next_sibling: Some(ElementId(1)),
                    ..Default::default()
                },
                Element {
                    name: "section",
                    first_child_query: Some(QueryId(1)),
                    ..Default::default()
                },
                Element {
                    name: "a",
                    ..Default::default()
                },
            ]
        );
    }

    #[test]
    fn test_multi_root_queries() {
        let queries = &[
            Query::all("span", Save::all()).unwrap().build(),
            Query::all("a", Save::all()).unwrap().build(),
        ];

        let mut store = Store::default();

        store.push(
            ElementId::default(),
            &queries[0].queries[0],
            crate::XHtmlElement {
                name: "span",
                ..Default::default()
            },
        );
        store.push(
            ElementId::default(),
            &queries[1].queries[0],
            crate::XHtmlElement {
                name: "a",
                ..Default::default()
            },
        );

        assert!(store.get("span").is_some());
        assert_eq!(store.get("span").iter().count(), 1);

        assert!(store.get("a").is_some());
        assert_eq!(store.get("a").iter().count(), 1);
    }

    #[test]
    fn result_meta_match_count_tracks_linked_list_length() {
        use crate::{Query, Save, parse};

        let html = (0..50)
            .map(|i| format!("<a href='{i}'></a>"))
            .collect::<String>();
        let queries = &[Query::all("a", Save::none()).unwrap().build()];
        let store = parse(&html, queries).unwrap();

        let (first, count) = store.result_meta("a").expect("matches");
        assert_eq!(count, 50);
        assert_eq!(store.get("a").unwrap().count(), count);
        assert_eq!(first, ElementId(0));

        // First-only queries keep a single match.
        let first_q = &[Query::first("a", Save::none()).unwrap().build()];
        let first_store = parse(&html, first_q).unwrap();
        let (_first, first_count) = first_store.result_meta("a").expect("one match");
        assert_eq!(first_count, 1);
        assert_eq!(first_store.get("a").unwrap().count(), 1);

        // Nested counts are independent per parent.
        let nested = &[Query::all("div", Save::none())
            .unwrap()
            .then(|div| Ok([div.all("span", Save::none())?]))
            .unwrap()
            .build()];
        let nested_html = "<div><span></span><span></span></div><div><span></span></div>";
        let nested_store = parse(nested_html, nested).unwrap();
        let divs: Vec<_> = nested_store.get("div").unwrap().collect();
        assert_eq!(divs[0].result_meta(&nested_store, "span").unwrap().1, 2);
        assert_eq!(divs[1].result_meta(&nested_store, "span").unwrap().1, 1);
        assert!(nested_store.result_meta("missing").is_none());
    }
}
