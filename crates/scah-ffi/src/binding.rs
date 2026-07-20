//! Rust binding helpers over the same owned-store model as the C ABI.
//!
//! Language bindings that link `scah-ffi` as an `rlib` should use this module
//! for hot result-access paths. It preserves centralized ownership in
//! `scah-ffi` while matching the one-pass `Arc` + element-id shape of the
//! pre-FFI wrappers (no per-result C heap allocation, no intermediate ID
//! vector before constructing language objects).

use crate::error::ScahStatus;
use crate::owned_store::OwnedStore;
use crate::query::ScahQuery;
use crate::store::ScahElementId;
use scah::{ElementId, ParseError};
use std::sync::Arc;

/// Shared store owner for language bindings.
#[derive(Clone)]
pub struct BindingStore {
    store: Arc<OwnedStore>,
}

impl BindingStore {
    /// Parse HTML against compiled query handles.
    ///
    /// # Safety
    ///
    /// Every pointer in `queries` must be a live [`ScahQuery`] returned by
    /// scah-ffi and remain valid for the duration of this call.
    pub unsafe fn parse(html: &str, queries: &[*const ScahQuery]) -> Result<Self, ParseError> {
        let owned = unsafe {
            queries
                .iter()
                .map(|ptr| (&**ptr).inner.clone())
                .collect::<Vec<_>>()
        };
        Ok(Self {
            store: Arc::new(OwnedStore::parse(html, &owned)?),
        })
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.store.store().elements.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One-pass match collection: invoke `f` once per match while walking.
    pub fn get_with<T>(
        &self,
        query: &str,
        mut f: impl FnMut(BindingStore, ScahElementId) -> T,
    ) -> Option<Vec<T>> {
        self.store.store().get(query).map(|iter| {
            iter.map(|e| {
                // SAFETY: `e` is borrowed from this store's arena.
                let id: ElementId = unsafe { self.store.store().elements.index_of(e) };
                f(self.clone(), id.index())
            })
            .collect()
        })
    }

    fn element(&self, id: ScahElementId) -> Result<&scah::Element<'static>, ScahStatus> {
        self.store
            .store()
            .elements
            .get(id)
            .ok_or(ScahStatus::IndexOutOfBounds)
    }

    pub fn name(&self, id: ScahElementId) -> Result<&str, ScahStatus> {
        Ok(self.element(id)?.name)
    }

    pub fn id_attr(&self, id: ScahElementId) -> Result<Option<&str>, ScahStatus> {
        Ok(self.element(id)?.id)
    }

    pub fn class_name(&self, id: ScahElementId) -> Result<Option<&str>, ScahStatus> {
        Ok(self.element(id)?.class)
    }

    pub fn inner_html(&self, id: ScahElementId) -> Result<Option<&str>, ScahStatus> {
        Ok(self.element(id)?.inner_html)
    }

    pub fn text_content(&self, id: ScahElementId) -> Result<Option<&str>, ScahStatus> {
        Ok(self.element(id)?.text_content(self.store.store()))
    }

    pub fn get_attribute(&self, id: ScahElementId, key: &str) -> Result<Option<&str>, ScahStatus> {
        Ok(self.element(id)?.attribute(self.store.store(), key))
    }

    /// Invoke `f` for each extra attribute (excluding dedicated class/id fields).
    pub fn for_each_attribute(
        &self,
        id: ScahElementId,
        mut f: impl FnMut(&str, Option<&str>),
    ) -> Result<(), ScahStatus> {
        if let Some(attrs) = self.element(id)?.attributes(self.store.store()) {
            for attr in attrs {
                f(attr.key, attr.value);
            }
        }
        Ok(())
    }

    pub fn child_get_with<T>(
        &self,
        id: ScahElementId,
        query: &str,
        mut f: impl FnMut(BindingStore, ScahElementId) -> T,
    ) -> Result<Option<Vec<T>>, ScahStatus> {
        let el = self.element(id)?;
        Ok(el.get(self.store.store(), query).map(|iter| {
            iter.map(|e| {
                // SAFETY: `e` is borrowed from this store's arena.
                let child_id: ElementId = unsafe { self.store.store().elements.index_of(e) };
                f(self.clone(), child_id.index())
            })
            .collect()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owned_query::OwnedQueryBuilder;
    use crate::owned_store::OwnedStore;
    use scah::Save;

    #[test]
    fn one_pass_get_matches_count() {
        let q = OwnedQueryBuilder::new_all("a".into(), Save::all())
            .build()
            .unwrap();
        let store = BindingStore {
            store: Arc::new(OwnedStore::parse("<a></a><a></a>", &[Arc::new(q)]).unwrap()),
        };
        let ids = store.get_with("a", |_s, id| id).expect("matches");
        assert_eq!(ids.len(), 2);
        assert_eq!(store.name(ids[0]).unwrap(), "a");
    }
}
