//! Owned store that retains HTML and query backing storage.

use crate::owned_query::OwnedQuery;
use scah::{ParseError, Query, Store};
use std::mem::ManuallyDrop;
use std::sync::Arc;

/// Parsed result store with all lifetime backing storage retained.
///
/// # Invariants
///
/// - Element / attribute / inner-html string slices borrow `html`.
/// - Query-node selector strings borrow tapes owned by `queries`.
/// - [`Drop`] destroys `store` before `html` and `queries`.
pub(crate) struct OwnedStore {
    store: ManuallyDrop<Store<'static, 'static>>,
    /// HTML backing storage; must outlive `store`.
    #[allow(dead_code)]
    html: Arc<str>,
    /// Query tape owners; must outlive `store`.
    #[allow(dead_code)]
    queries: Vec<Arc<OwnedQuery>>,
}

impl Drop for OwnedStore {
    fn drop(&mut self) {
        // SAFETY: drop the store (and its borrows) before html/query owners.
        unsafe {
            ManuallyDrop::drop(&mut self.store);
        }
    }
}

impl OwnedStore {
    #[inline]
    pub(crate) fn store(&self) -> &Store<'static, 'static> {
        &self.store
    }

    pub(crate) fn parse(html: &str, queries: &[Arc<OwnedQuery>]) -> Result<Self, ParseError> {
        if queries.is_empty() {
            return Err(ParseError::EmptyQueries);
        }

        let owned_queries: Vec<Arc<OwnedQuery>> = queries.to_vec();
        let html: Arc<str> = Arc::from(html);

        // SAFETY: `OwnedStore` retains `html` for as long as `store` exists.
        // All HTML-derived slices inside `Store` borrow this allocation.
        let html_str: &'static str = unsafe { std::mem::transmute(html.as_ref()) };

        let mut queries_rs: Vec<Query<'static>> = Vec::with_capacity(owned_queries.len());
        for q in &owned_queries {
            queries_rs.push(q.query().clone());
        }

        // SAFETY:
        // `scah::parse` requires `'a: 'query` on the query-slice borrow. Here
        // `'query` is `'static` because each `Query` already owns its selector
        // strings via `OwnedQuery`'s tape. The local `queries_rs` Vec is only
        // read during parsing; `Store` retains selector string data from the
        // queries (backed by `owned_queries`), not a reference into the Vec
        // allocation itself. Extending the slice lifetime satisfies the bound
        // without leaking.
        let queries_slice: &'static [Query<'static>] =
            unsafe { std::slice::from_raw_parts(queries_rs.as_ptr(), queries_rs.len()) };

        let store = scah::parse(html_str, queries_slice)?;

        Ok(Self {
            store: ManuallyDrop::new(store),
            html,
            queries: owned_queries,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owned_query::OwnedQueryBuilder;
    use scah::Save;

    #[test]
    fn parse_and_lookup() {
        let q = Arc::new(
            OwnedQueryBuilder::new_all("a".into(), Save::all())
                .build()
                .unwrap(),
        );
        let html = String::from("<div><a href='x'>hi</a></div>");
        let owned = OwnedStore::parse(&html, &[q]).unwrap();
        drop(html);

        let a = owned.store().get("a").unwrap().next().unwrap();
        assert_eq!(a.name, "a");
        assert_eq!(a.attribute(owned.store(), "href"), Some("x"));
        assert_eq!(a.text_content(owned.store()), Some("hi"));
    }

    #[test]
    fn empty_queries_error() {
        assert!(matches!(
            OwnedStore::parse("<a></a>", &[]),
            Err(ParseError::EmptyQueries)
        ));
    }

    #[test]
    fn query_handle_can_be_dropped_after_parse() {
        let q = Arc::new(
            OwnedQueryBuilder::new_all("a".into(), Save::only_text_content())
                .build()
                .unwrap(),
        );
        let owned = OwnedStore::parse("<a>text</a>", std::slice::from_ref(&q)).unwrap();
        drop(q);
        assert_eq!(
            owned
                .store()
                .get("a")
                .unwrap()
                .next()
                .unwrap()
                .text_content(owned.store()),
            Some("text")
        );
    }
}
