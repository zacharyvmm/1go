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
    /// based on measured HTML element density (~1 element per 20-50 bytes,
    /// of which only matched elements land in the arena).
    ///
    /// Ratios chosen from valgrind massif profiling (`capacity_profile` example):
    ///
    /// | Arena       | Ratio            | Rationale                             |
    /// |------------ |----------------- |---------------------------------------|
    /// | elements    | `capacity / 48`  | ~1 matched element per 48 input bytes  |
    /// | attributes  | `capacity / 24`  | ~2-3 attributes per matched element    |
    /// | text_content| `capacity`        | worst case: every byte is text          |
    /// | queries     | none              | fixed small per-query allocation       |
    ///
    /// These are deliberately conservative to avoid the over-reservation that
    /// `capacity / 3` caused while still preventing reallocations in typical
    /// workloads. Queries targeting 90%+ of elements (e.g. `*`) may still
    /// reallocate; this is an acceptable tradeoff.
    ///
    /// Pass `capture_text_content: false` when no query needs text content
    /// to avoid allocating a buffer proportional to the full HTML input.
    /// Element and attribute arenas are always reserved regardless of this
    /// flag, using the same ratios.
    pub fn with_capacity(capacity: usize, capture_text_content: bool) -> Self {
        Self {
            elements: Arena::with_capacity(capacity / 48),
            queries: Arena::new(),
            text_content: if capture_text_content {
                TextContent::with_capacity(capacity)
            } else {
                TextContent::new()
            },
            attributes: Arena::with_capacity(capacity / 24),
            #[cfg(any(debug_assertions, test))]
            trace: crate::debug::TraceStore::with_capacity(capacity.min(4096)),
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
        if self.queries.is_empty() {
            return None;
        }

        self.queries
            .iter_from(QueryId(0))
            .find(|q| q.query == query)
            .map(|query_node| query_node.elements.start())
            .map(|element_id| self.elements.iter_from(element_id))
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
    }

    pub fn push(
        &mut self,
        from: ElementId,
        selection: &QuerySection<'query>,
        element: crate::XHtmlElement<'html>,
    ) -> ElementId {
        let new_element = Element {
            name: element.name,
            class: element.class,
            id: element.id,
            attributes: self.attributes.attribute_slice_to_range(element.attributes),
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
        let store = Store::with_capacity(30_000, true);

        assert_eq!(store.elements.capacity(), 30_000 / 48);
        assert_eq!(store.attributes.capacity(), 30_000 / 24);
        assert_eq!(store.text_content.content.capacity(), 30_000);
    }

    #[test]
    fn with_capacity_can_skip_text_content_reservation() {
        let store = Store::with_capacity(30_000, false);

        assert_eq!(store.elements.capacity(), 30_000 / 48);
        assert_eq!(store.attributes.capacity(), 30_000 / 24);
        assert_eq!(store.text_content.content.capacity(), 0);
    }

    #[test]
    fn with_capacity_can_reserve_text_content() {
        let store = Store::with_capacity(30_000, true);

        assert_eq!(store.elements.capacity(), 30_000 / 48);
        assert_eq!(store.attributes.capacity(), 30_000 / 24);
        assert_eq!(store.text_content.content.capacity(), 30_000);
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
                elements: Span::new(ElementId(0))
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
                    elements: Span::new(ElementId(0))
                },
                QueryNode {
                    query: "2",
                    next_sibling: None,
                    elements: Span::new(ElementId(1))
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
                    elements: Span::new(ElementId(0))
                },
                QueryNode {
                    query: "2",
                    next_sibling: Some(QueryId(2)),
                    elements: Span::new(ElementId(1))
                },
                QueryNode {
                    query: "3",
                    next_sibling: None,
                    elements: Span::new(ElementId(2))
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
                elements: Span::from(ElementId(0), ElementId(1))
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
                    elements: Span::from(ElementId(0), ElementId(1))
                },
                QueryNode {
                    query: "> a[href]",
                    next_sibling: None,
                    elements: Span::new(ElementId(2))
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
}
