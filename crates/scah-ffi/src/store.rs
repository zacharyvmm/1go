//! Opaque store/element-list handles and C ABI entry points.
//!
//! Result elements are exposed as borrowed [`ScahElementId`] values owned by a
//! [`ScahElementList`]. There is no per-result C heap allocation: language
//! bindings keep one list owner and copy integer IDs.

use crate::error::{
    ScahError, ScahStatus, clear_out_error, ffi_guard, ffi_guard_leaf, ffi_guard_value,
    ffi_guard_void, set_error,
};
use crate::owned_store::OwnedStore;
use crate::query::ScahQuery;
use crate::string::{ScahOptionalStringView, ScahStringView};
use scah::ParseError;
use std::sync::Arc;

/// Opaque parsed store handle.
pub struct ScahStore {
    pub(crate) inner: Arc<OwnedStore>,
}

/// Store-local element identifier.
///
/// An element ID may only be used with the [`ScahElementList`] that produced
/// it, or a descendant list returned from an element belonging to the same
/// owner. Not a heap handle. Do not exchange IDs between arbitrary lists even
/// when they happen to share a store.
pub type ScahElementId = usize;

/// Opaque list of element ids sharing a store.
///
/// Match results are stored as a linked-list span (`first` + `len`) rather than
/// an eagerly copied ID vector. [`scah_element_list_fill_ids`] and
/// [`scah_store_get_ids_fill`] copy IDs into a caller buffer. [`scah_element_list_ids`]
/// lazily materializes a cache on first use for C callers that need a borrowed
/// pointer.
///
/// # Lifetime invariants
///
/// - [`scah_element_list_ids`] returns a pointer into this list's lazily
///   materialized ID cache. The pointer remains valid until the list is freed.
/// - String views obtained via element getters remain valid while this list
///   (which retains the [`OwnedStore`]) remains alive.
/// - Freeing the original [`ScahStore`] does not invalidate elements accessed
///   through this list.
/// - An element ID may only be used with this list, or a descendant list
///   returned from [`scah_element_get`] on an element from this owner.
pub struct ScahElementList {
    pub(crate) store: Arc<OwnedStore>,
    pub(crate) first: Option<ScahElementId>,
    pub(crate) len: usize,
    ids_cache: std::sync::OnceLock<Vec<ScahElementId>>,
}

/// Borrowed key/value view for one attribute.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ScahAttributeView {
    pub key: ScahStringView,
    pub value: ScahOptionalStringView,
}

/// Fixed-field snapshot of an element. All string views borrow store-owned HTML
/// and remain valid while the element-list owner remains alive.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ScahElementView {
    pub name: ScahStringView,
    pub id: ScahOptionalStringView,
    pub class_name: ScahOptionalStringView,
    pub inner_html: ScahOptionalStringView,
    pub text_content: ScahOptionalStringView,
    pub attribute_count: usize,
}

/// # Safety
///
/// `ptr` must be non-null and point to a live `T` for the returned lifetime.
unsafe fn require_ref<'a, T>(ptr: *const T) -> Result<&'a T, ScahStatus> {
    if ptr.is_null() {
        Err(ScahStatus::NullPointer)
    } else {
        // SAFETY: caller guarantees a live `T`.
        Ok(unsafe { &*ptr })
    }
}

/// # Safety
///
/// When `out` is non-null, it must be valid for writing one `*mut T`.
unsafe fn write_ptr<T>(out: *mut *mut T, value: Box<T>) -> Result<(), ScahStatus> {
    if out.is_null() {
        return Err(ScahStatus::NullPointer);
    }
    unsafe {
        *out = Box::into_raw(value);
    }
    Ok(())
}

/// # Safety
///
/// When `out` is non-null, it must be valid for writing one `*mut T`.
unsafe fn clear_out_ptr<T>(out: *mut *mut T) {
    if !out.is_null() {
        unsafe {
            *out = std::ptr::null_mut();
        }
    }
}

/// # Safety
///
/// When `out` is non-null, it must be valid for writing one `u8`.
unsafe fn clear_out_u8(out: *mut u8) {
    if !out.is_null() {
        unsafe {
            *out = 0;
        }
    }
}

/// # Safety
///
/// When `out` is non-null, it must be valid for writing one `usize`.
unsafe fn clear_out_usize(out: *mut usize) {
    if !out.is_null() {
        unsafe {
            *out = 0;
        }
    }
}

/// # Safety
///
/// When `out` is non-null, it must be valid for writing one [`ScahStringView`].
unsafe fn clear_out_string_view(out: *mut ScahStringView) {
    if !out.is_null() {
        unsafe {
            *out = ScahStringView::empty();
        }
    }
}

/// # Safety
///
/// When `out` is non-null, it must be valid for writing one
/// [`ScahOptionalStringView`].
unsafe fn clear_out_optional_string_view(out: *mut ScahOptionalStringView) {
    if !out.is_null() {
        unsafe {
            *out = ScahOptionalStringView::none();
        }
    }
}

/// # Safety
///
/// When `out` is non-null, it must be valid for writing one [`ScahElementView`].
unsafe fn clear_out_element_view(out: *mut ScahElementView) {
    if !out.is_null() {
        unsafe {
            *out = ScahElementView {
                name: ScahStringView::empty(),
                id: ScahOptionalStringView::none(),
                class_name: ScahOptionalStringView::none(),
                inner_html: ScahOptionalStringView::none(),
                text_content: ScahOptionalStringView::none(),
                attribute_count: 0,
            };
        }
    }
}

/// # Safety
///
/// `view` must satisfy [`ScahStringView::as_str`]. When `out_error` is
/// non-null, it must be valid for writing one `*mut ScahError`.
unsafe fn parse_string_view<'a>(
    view: ScahStringView,
    out_error: *mut *mut ScahError,
) -> Result<&'a str, ScahStatus> {
    // SAFETY: caller guarantees the string-view contract.
    match unsafe { view.as_str() } {
        Ok(s) => Ok(s),
        Err(ScahStatus::NullPointer) => {
            unsafe {
                set_error(out_error, "null string pointer with nonzero length");
            }
            Err(ScahStatus::NullPointer)
        }
        Err(ScahStatus::InvalidUtf8) => {
            unsafe {
                set_error(out_error, "string view is not valid UTF-8");
            }
            Err(ScahStatus::InvalidUtf8)
        }
        Err(other) => Err(other),
    }
}

/// Leaf-path string view parse that skips UTF-8 re-validation.
///
/// # Safety
///
/// Same as [`parse_string_view`], and `view` bytes must already be valid UTF-8.
#[inline(always)]
unsafe fn parse_string_view_unchecked<'a>(view: ScahStringView) -> Result<&'a str, ScahStatus> {
    unsafe { view.as_str_unchecked() }
}

fn resolve_element(
    list: &ScahElementList,
    element: ScahElementId,
) -> Result<&scah::Element<'static>, ScahStatus> {
    list.store
        .store()
        .elements
        .get(element)
        .ok_or(ScahStatus::IndexOutOfBounds)
}

#[inline(always)]
unsafe fn leaf_name_status(
    owner: *const ScahElementList,
    element: ScahElementId,
    out_name: *mut ScahStringView,
) -> ScahStatus {
    if out_name.is_null() {
        return ScahStatus::NullPointer;
    }
    // Always clear so failed calls leave a defined empty view.
    unsafe {
        *out_name = ScahStringView::empty();
    }
    if owner.is_null() {
        return ScahStatus::NullPointer;
    }
    // SAFETY: caller guarantees a live list when non-null.
    let list = unsafe { &*owner };
    match list.store.store().elements.get(element) {
        Some(el) => {
            unsafe {
                *out_name = ScahStringView::borrow(el.name);
            }
            ScahStatus::Ok
        }
        None => ScahStatus::IndexOutOfBounds,
    }
}

#[inline(always)]
unsafe fn leaf_optional_field_status(
    owner: *const ScahElementList,
    element: ScahElementId,
    out: *mut ScahOptionalStringView,
    field: impl for<'a> FnOnce(&'a scah::Element<'static>, &'a OwnedStore) -> Option<&'a str>,
) -> ScahStatus {
    if out.is_null() {
        return ScahStatus::NullPointer;
    }
    unsafe {
        *out = ScahOptionalStringView::none();
    }
    if owner.is_null() {
        return ScahStatus::NullPointer;
    }
    let list = unsafe { &*owner };
    match list.store.store().elements.get(element) {
        Some(el) => {
            unsafe {
                *out = ScahOptionalStringView::from_option(field(el, &list.store));
            }
            ScahStatus::Ok
        }
        None => ScahStatus::IndexOutOfBounds,
    }
}

/// Panic-free attribute fill used by the hot binding path.
#[inline(always)]
unsafe fn leaf_attributes_fill_status(
    owner: *const ScahElementList,
    element: ScahElementId,
    out_attributes: *mut ScahAttributeView,
    capacity: usize,
    out_written: *mut usize,
) -> ScahStatus {
    if out_written.is_null() {
        return ScahStatus::NullPointer;
    }
    unsafe {
        *out_written = 0;
    }
    if capacity > 0 && out_attributes.is_null() {
        return ScahStatus::NullPointer;
    }
    if owner.is_null() {
        return ScahStatus::NullPointer;
    }
    let list = unsafe { &*owner };
    let Some(el) = list.store.store().elements.get(element) else {
        return ScahStatus::IndexOutOfBounds;
    };
    let attrs = el.attributes(list.store.store());
    let count = attrs.map(|a| a.len()).unwrap_or(0);
    if count > capacity {
        unsafe {
            *out_written = count;
        }
        return ScahStatus::BufferTooSmall;
    }
    if let Some(attrs) = attrs {
        for (i, attr) in attrs.iter().enumerate() {
            unsafe {
                *out_attributes.add(i) = ScahAttributeView {
                    key: ScahStringView::borrow(attr.key),
                    value: ScahOptionalStringView::from_option(attr.value),
                };
            }
        }
    }
    unsafe {
        *out_written = count;
    }
    ScahStatus::Ok
}

#[inline(always)]
unsafe fn leaf_fill_from_status(
    store: &OwnedStore,
    first: Option<ScahElementId>,
    len: usize,
    out_ids: *mut ScahElementId,
    capacity: usize,
    out_written: *mut usize,
) -> ScahStatus {
    if out_written.is_null() {
        return ScahStatus::NullPointer;
    }
    unsafe {
        *out_written = 0;
    }
    if capacity > 0 && out_ids.is_null() {
        return ScahStatus::NullPointer;
    }
    if len > capacity {
        unsafe {
            *out_written = len;
        }
        return ScahStatus::BufferTooSmall;
    }
    walk_from_first(store, first, len, out_ids, capacity);
    unsafe {
        *out_written = len;
    }
    ScahStatus::Ok
}

#[inline(always)]
unsafe fn leaf_store_name_status(
    store: *const ScahStore,
    element: ScahElementId,
    out_name: *mut ScahStringView,
) -> ScahStatus {
    if out_name.is_null() {
        return ScahStatus::NullPointer;
    }
    unsafe {
        *out_name = ScahStringView::empty();
    }
    if store.is_null() {
        return ScahStatus::NullPointer;
    }
    let store = unsafe { &*store };
    match store.inner.store().elements.get(element) {
        Some(el) => {
            unsafe {
                *out_name = ScahStringView::borrow(el.name);
            }
            ScahStatus::Ok
        }
        None => ScahStatus::IndexOutOfBounds,
    }
}

#[inline(always)]
unsafe fn leaf_store_optional_field_status(
    store: *const ScahStore,
    element: ScahElementId,
    out: *mut ScahOptionalStringView,
    field: impl for<'a> FnOnce(&'a scah::Element<'static>, &'a OwnedStore) -> Option<&'a str>,
) -> ScahStatus {
    if out.is_null() {
        return ScahStatus::NullPointer;
    }
    unsafe {
        *out = ScahOptionalStringView::none();
    }
    if store.is_null() {
        return ScahStatus::NullPointer;
    }
    let store = unsafe { &*store };
    match store.inner.store().elements.get(element) {
        Some(el) => {
            unsafe {
                *out = ScahOptionalStringView::from_option(field(el, &store.inner));
            }
            ScahStatus::Ok
        }
        None => ScahStatus::IndexOutOfBounds,
    }
}

#[inline(always)]
unsafe fn leaf_store_attributes_fill_status(
    store: *const ScahStore,
    element: ScahElementId,
    out_attributes: *mut ScahAttributeView,
    capacity: usize,
    out_written: *mut usize,
) -> ScahStatus {
    if out_written.is_null() {
        return ScahStatus::NullPointer;
    }
    unsafe {
        *out_written = 0;
    }
    if capacity > 0 && out_attributes.is_null() {
        return ScahStatus::NullPointer;
    }
    if store.is_null() {
        return ScahStatus::NullPointer;
    }
    let store = unsafe { &*store };
    let Some(el) = store.inner.store().elements.get(element) else {
        return ScahStatus::IndexOutOfBounds;
    };
    let attrs = el.attributes(store.inner.store());
    let count = attrs.map(|a| a.len()).unwrap_or(0);
    if count > capacity {
        unsafe {
            *out_written = count;
        }
        return ScahStatus::BufferTooSmall;
    }
    if let Some(attrs) = attrs {
        for (i, attr) in attrs.iter().enumerate() {
            unsafe {
                *out_attributes.add(i) = ScahAttributeView {
                    key: ScahStringView::borrow(attr.key),
                    value: ScahOptionalStringView::from_option(attr.value),
                };
            }
        }
    }
    unsafe {
        *out_written = count;
    }
    ScahStatus::Ok
}

/// Resolve match metadata in O(query-node count); optionally fill caller buffers
/// from the contiguous match-id list.
///
/// Returns `(first_id, total_len)`. When `len > capacity`, returns metadata
/// without copying so callers can allocate exactly and retry.
fn walk_match_ids(
    store: &OwnedStore,
    query: &str,
    out_ids: *mut ScahElementId,
    out_names: *mut ScahStringView,
    capacity: usize,
) -> Option<(Option<ScahElementId>, usize)> {
    let ids = store.store().result_ids(query)?;
    let len = ids.len();
    let first = Some(ids[0].index());
    if len > capacity || capacity == 0 || (out_ids.is_null() && out_names.is_null()) {
        return Some((first, len));
    }
    for (i, id) in ids.iter().enumerate() {
        let idx = id.index();
        if !out_ids.is_null() {
            unsafe {
                *out_ids.add(i) = idx;
            }
        }
        if !out_names.is_null() {
            let name = store
                .store()
                .elements
                .get(idx)
                .map(|e| e.name)
                .unwrap_or("");
            unsafe {
                *out_names.add(i) = ScahStringView::borrow(name);
            }
        }
    }
    Some((first, len))
}

fn walk_from_ids(ids: &[scah::ElementId], out_ids: *mut ScahElementId, capacity: usize) -> usize {
    let write_len = ids.len().min(capacity);
    for (i, id) in ids.iter().take(write_len).enumerate() {
        if !out_ids.is_null() {
            unsafe {
                *out_ids.add(i) = id.index();
            }
        }
    }
    ids.len()
}

fn walk_from_first(
    store: &OwnedStore,
    first: Option<ScahElementId>,
    len: usize,
    out_ids: *mut ScahElementId,
    capacity: usize,
) -> usize {
    let Some(mut cursor) = first else {
        return 0;
    };
    let write_len = len.min(capacity);
    for i in 0..len {
        if i < write_len && !out_ids.is_null() {
            unsafe {
                *out_ids.add(i) = cursor;
            }
        }
        if i + 1 == len {
            break;
        }
        let next = store
            .store()
            .elements
            .get(cursor)
            .and_then(|el| el.next_sibling)
            .map(|id| id.index());
        match next {
            Some(n) => cursor = n,
            None => return i + 1,
        }
    }
    len
}

fn child_match_ids(
    store: &OwnedStore,
    el: &scah::Element<'static>,
    query: &str,
    out_ids: *mut ScahElementId,
    capacity: usize,
) -> Option<(Option<ScahElementId>, usize)> {
    let ids = el.result_ids(store.store(), query)?;
    let first = Some(ids[0].index());
    let len = ids.len();
    // Skip the copy when the caller must retry with a larger buffer.
    if len <= capacity && !out_ids.is_null() && capacity > 0 {
        walk_from_ids(ids, out_ids, capacity);
    }
    Some((first, len))
}

fn new_element_list(
    store: Arc<OwnedStore>,
    first: Option<ScahElementId>,
    len: usize,
) -> ScahElementList {
    ScahElementList {
        store,
        first,
        len,
        ids_cache: std::sync::OnceLock::new(),
    }
}

/// Current C ABI version.
#[unsafe(no_mangle)]
pub extern "C" fn scah_abi_version() -> u32 {
    ffi_guard_value(1, || 1)
}

/// Parse HTML against one or more compiled queries.
///
/// Returned string views from the store/elements borrow store-owned HTML and
/// remain valid only while the owning store or element-list handle is alive.
///
/// # Safety
///
/// `html` must satisfy [`ScahStringView::as_str`] and remain valid for the
/// duration of this call. When `query_count > 0`, `queries` must point to an
/// array of `query_count` readable query-handle pointers; every entry must
/// point to a live [`ScahQuery`]. `out_store` must be non-null and valid for
/// writing one `*mut ScahStore`. When `out_error` is non-null, it must be valid
/// for writing one `*mut ScahError`. On success the caller owns the store and
/// must free it with [`scah_store_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_parse(
    html: ScahStringView,
    queries: *const *const ScahQuery,
    query_count: usize,
    out_store: *mut *mut ScahStore,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_ptr(out_store);
        ffi_guard(out_error, || {
            if query_count == 0 {
                set_error(out_error, "parse requires at least one query");
                return Err(ScahStatus::EmptyQueries);
            }
            if queries.is_null() {
                return Err(ScahStatus::NullPointer);
            }

            let html = parse_string_view(html, out_error)?;

            let mut owned_queries = Vec::with_capacity(query_count);
            for i in 0..query_count {
                let ptr = *queries.add(i);
                let query = require_ref(ptr)?;
                owned_queries.push(query.inner.clone());
            }

            match OwnedStore::parse(html, &owned_queries) {
                Ok(owned) => {
                    write_ptr(
                        out_store,
                        Box::new(ScahStore {
                            inner: Arc::new(owned),
                        }),
                    )?;
                    Ok(())
                }
                Err(ParseError::EmptyQueries) => {
                    set_error(out_error, "parse requires at least one query");
                    Err(ScahStatus::EmptyQueries)
                }
                Err(ParseError::MaximumDepthExceeded) => {
                    set_error(
                        out_error,
                        "HTML nesting depth exceeds the maximum supported depth",
                    );
                    Err(ScahStatus::MaximumDepthExceeded)
                }
            }
        })
    }
}

/// Number of elements in the store.
///
/// # Safety
///
/// `store` must either be null or point to a live [`ScahStore`] returned by
/// scah-ffi. `out_len`, when non-null, must be valid for writing one `usize`.
/// When `out_error` is non-null, it must be valid for writing one
/// `*mut ScahError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_len(
    store: *const ScahStore,
    out_len: *mut usize,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_usize(out_len);
        ffi_guard_leaf(out_error, || {
            let store = require_ref(store)?;
            if out_len.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            *out_len = store.inner.store().elements.len();
            Ok(())
        })
    }
}

/// Look up elements matched by `query`.
///
/// When the selector is absent, `*out_found` is set to 0 and this is not an error.
/// The returned list stores a match span (`first` + `len`) rather than an
/// eagerly copied ID vector. Use [`scah_element_list_fill_ids`] or
/// [`scah_store_get_ids_fill`] to copy IDs into a caller buffer.
///
/// # Safety
///
/// `store` must point to a live [`ScahStore`]. `query` must satisfy
/// [`ScahStringView::as_str`]. `out_elements` and `out_found` must be non-null
/// and valid for writing. When `out_error` is non-null, it must be valid for
/// writing one `*mut ScahError`. On success with `*out_found == 1`, the caller
/// owns the list and must free it with [`scah_element_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_get(
    store: *const ScahStore,
    query: ScahStringView,
    out_elements: *mut *mut ScahElementList,
    out_found: *mut u8,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_ptr(out_elements);
        clear_out_u8(out_found);
        ffi_guard(out_error, || {
            let store = require_ref(store)?;
            if out_elements.is_null() || out_found.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let query = parse_string_view(query, out_error)?;
            let owned = store.inner.clone();
            match store.inner.store().result_meta(query) {
                None => {
                    *out_found = 0;
                    *out_elements = std::ptr::null_mut();
                    Ok(())
                }
                Some((first, len)) => {
                    *out_found = 1;
                    write_ptr(
                        out_elements,
                        Box::new(new_element_list(owned, Some(first.index()), len)),
                    )?;
                    Ok(())
                }
            }
        })
    }
}

/// Create a store-retaining owner list with an empty span.
///
/// Language bindings allocate one owner per parsed store and reuse it for all
/// element getters, avoiding a boxed list allocation on every lookup. Prefer
/// [`scah_store_get_span`] + [`scah_store_fill_ids`] for result materialization.
///
/// # Safety
///
/// `store` must point to a live [`ScahStore`]. `out_owner` must be non-null.
/// On success the caller owns the list and must free it with
/// [`scah_element_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_owner(
    store: *const ScahStore,
    out_owner: *mut *mut ScahElementList,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_ptr(out_owner);
        ffi_guard(out_error, || {
            let store = require_ref(store)?;
            if out_owner.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            write_ptr(
                out_owner,
                Box::new(new_element_list(store.inner.clone(), None, 0)),
            )?;
            Ok(())
        })
    }
}

/// Look up match span metadata without allocating a result list.
///
/// On success with `*out_found == 1`, writes the first element id and the
/// match count (O(1) after query-node resolution). No owned outputs are
/// transferred.
///
/// # Safety
///
/// `store` must point to a live [`ScahStore`]. `out_first`, `out_len`, and
/// `out_found` must be non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_get_span(
    store: *const ScahStore,
    query: ScahStringView,
    out_first: *mut ScahElementId,
    out_len: *mut usize,
    out_found: *mut u8,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_usize(out_len);
        clear_out_u8(out_found);
        if !out_first.is_null() {
            *out_first = 0;
        }
        ffi_guard(out_error, || {
            let store = require_ref(store)?;
            if out_first.is_null() || out_len.is_null() || out_found.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let query = parse_string_view(query, out_error)?;
            match store.inner.store().result_meta(query) {
                None => {
                    *out_found = 0;
                    Ok(())
                }
                Some((first, len)) => {
                    *out_found = 1;
                    *out_first = first.index();
                    *out_len = len;
                    Ok(())
                }
            }
        })
    }
}

/// Copy `len` result IDs starting at `first` into a caller buffer.
///
/// When `capacity < len`, returns [`ScahStatus::BufferTooSmall`] with
/// `*out_written = len` and no diagnostic allocation.
///
/// # Safety
///
/// `store` must point to a live [`ScahStore`]. When `capacity > 0`, `out_ids`
/// must be writable for `capacity` IDs. `out_written` must be non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_fill_ids(
    store: *const ScahStore,
    first: ScahElementId,
    len: usize,
    out_ids: *mut ScahElementId,
    capacity: usize,
    out_written: *mut usize,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_usize(out_written);
        ffi_guard_leaf(out_error, || {
            let store = require_ref(store)?;
            if out_written.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            if capacity > 0 && out_ids.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            if len > capacity {
                *out_written = len;
                return Err(ScahStatus::BufferTooSmall);
            }
            walk_from_first(&store.inner, Some(first), len, out_ids, capacity);
            *out_written = len;
            Ok(())
        })
    }
}

/// Copy `len` result IDs starting at `first` using a list owner's store.
///
/// Same sizing contract as [`scah_store_fill_ids`].
///
/// # Safety
///
/// `owner` must point to a live [`ScahElementList`]. When `capacity > 0`,
/// `out_ids` must be writable for `capacity` IDs. `out_written` must be non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_list_fill_from(
    owner: *const ScahElementList,
    first: ScahElementId,
    len: usize,
    out_ids: *mut ScahElementId,
    capacity: usize,
    out_written: *mut usize,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_error(out_error);
        if owner.is_null() {
            clear_out_usize(out_written);
            return ScahStatus::NullPointer;
        }
        let list = &*owner;
        leaf_fill_from_status(
            &list.store,
            Some(first),
            len,
            out_ids,
            capacity,
            out_written,
        )
    }
}

/// Look up matches and copy IDs (and optionally names) into caller buffers.
///
/// When `capacity` is smaller than the match count, returns
/// [`ScahStatus::BufferTooSmall`] and writes the required count to
/// `*out_written`. No owned list is transferred and no [`ScahError`] is
/// allocated for this expected capacity negotiation — use [`scah_store_get`]
/// then [`scah_element_list_fill_ids`] when the caller needs a list owner.
///
/// Pass `out_elements == NULL` to only fill IDs (caller must keep the store
/// alive for subsequent element access). Pass `out_names == NULL` to skip
/// name materialization; when non-null it must have `capacity` slots.
///
/// Owned outputs are transferred only when status is [`ScahStatus::Ok`].
///
/// # Safety
///
/// Same string/store requirements as [`scah_store_get`]. When `capacity > 0`,
/// `out_ids` must point to a writable array of `capacity` IDs. `out_written`
/// and `out_found` must be non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_get_ids_fill(
    store: *const ScahStore,
    query: ScahStringView,
    out_ids: *mut ScahElementId,
    out_names: *mut ScahStringView,
    capacity: usize,
    out_written: *mut usize,
    out_elements: *mut *mut ScahElementList,
    out_found: *mut u8,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_usize(out_written);
        clear_out_u8(out_found);
        clear_out_ptr(out_elements);
        clear_out_error(out_error);

        // Binding hot path: no owned list → audited panic-free leaf (no Arc clone).
        if out_elements.is_null() {
            if store.is_null() || out_written.is_null() || out_found.is_null() {
                return ScahStatus::NullPointer;
            }
            if capacity > 0 && out_ids.is_null() {
                return ScahStatus::NullPointer;
            }
            let store = &*store;
            let query = match parse_string_view_unchecked(query) {
                Ok(q) => q,
                Err(status) => return status,
            };
            return match walk_match_ids(&store.inner, query, out_ids, out_names, capacity) {
                None => {
                    *out_found = 0;
                    *out_written = 0;
                    ScahStatus::Ok
                }
                Some((_first, len)) => {
                    *out_found = 1;
                    *out_written = len;
                    if len > capacity {
                        ScahStatus::BufferTooSmall
                    } else {
                        ScahStatus::Ok
                    }
                }
            };
        }

        // List-owning path may allocate — keep catch_unwind.
        ffi_guard(out_error, || {
            let store = require_ref(store)?;
            if out_written.is_null() || out_found.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            if capacity > 0 && out_ids.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let query = parse_string_view(query, out_error)?;
            let owned = store.inner.clone();
            match walk_match_ids(&owned, query, out_ids, out_names, capacity) {
                None => {
                    *out_found = 0;
                    *out_written = 0;
                    Ok(())
                }
                Some((first, len)) => {
                    *out_found = 1;
                    *out_written = len;
                    if len > capacity {
                        return Err(ScahStatus::BufferTooSmall);
                    }
                    write_ptr(out_elements, Box::new(new_element_list(owned, first, len)))?;
                    Ok(())
                }
            }
        })
    }
}

/// Free a store handle. Null is a no-op.
///
/// # Safety
///
/// A non-null `store` must have been returned by scah-ffi, must not already
/// have been freed, and must not be used again after this call. Element-list
/// handles that retain an `Arc` to the same store remain valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_free(store: *mut ScahStore) {
    ffi_guard_void(|| {
        if store.is_null() {
            return;
        }
        unsafe {
            drop(Box::from_raw(store));
        }
    });
}

/// Length of an element list.
///
/// # Safety
///
/// `list` must either be null or point to a live [`ScahElementList`].
/// `out_len`, when non-null, must be valid for writing one `usize`. When
/// `out_error` is non-null, it must be valid for writing one `*mut ScahError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_list_len(
    list: *const ScahElementList,
    out_len: *mut usize,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_usize(out_len);
        ffi_guard_leaf(out_error, || {
            let list = require_ref(list)?;
            if out_len.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            *out_len = list.len;
            Ok(())
        })
    }
}

/// Fill a caller-provided buffer with the list's element IDs.
///
/// When `capacity` is smaller than the list length, returns
/// [`ScahStatus::BufferTooSmall`] and writes the required count to
/// `*out_written`.
///
/// # Safety
///
/// `list` must point to a live [`ScahElementList`]. When `capacity > 0`,
/// `out_ids` must point to a writable array of `capacity` IDs. `out_written`
/// must be non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_list_fill_ids(
    list: *const ScahElementList,
    out_ids: *mut ScahElementId,
    capacity: usize,
    out_written: *mut usize,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_usize(out_written);
        ffi_guard_leaf(out_error, || {
            let list = require_ref(list)?;
            if out_written.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            if capacity > 0 && out_ids.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            if list.len > capacity {
                *out_written = list.len;
                return Err(ScahStatus::BufferTooSmall);
            }
            walk_from_first(&list.store, list.first, list.len, out_ids, capacity);
            *out_written = list.len;
            Ok(())
        })
    }
}

/// Borrow the complete ID slice for a result list.
///
/// Lazily materializes an ID cache on first call. Prefer
/// [`scah_element_list_fill_ids`] or [`scah_store_get_ids_fill`] to avoid the
/// cache allocation. The returned pointer remains valid until the list is freed.
///
/// # Safety
///
/// `list` must point to a live [`ScahElementList`]. `out_ids` and `out_len` must
/// be non-null and valid for writing. When `out_error` is non-null, it must be
/// valid for writing one `*mut ScahError`. The pointer written to `*out_ids`
/// borrows the list and must not be used after [`scah_element_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_list_ids(
    list: *const ScahElementList,
    out_ids: *mut *const ScahElementId,
    out_len: *mut usize,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        if !out_ids.is_null() {
            *out_ids = std::ptr::null();
        }
        clear_out_usize(out_len);
        ffi_guard(out_error, || {
            let list = require_ref(list)?;
            if out_ids.is_null() || out_len.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let cache = list.ids_cache.get_or_init(|| {
                let mut ids = vec![0; list.len];
                walk_from_first(
                    &list.store,
                    list.first,
                    list.len,
                    ids.as_mut_ptr(),
                    ids.len(),
                );
                ids
            });
            *out_ids = cache.as_ptr();
            *out_len = cache.len();
            Ok(())
        })
    }
}

/// Free an element list. Null is a no-op.
///
/// # Safety
///
/// A non-null `list` must have been returned by scah-ffi, must not already
/// have been freed, and must not be used again after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_list_free(list: *mut ScahElementList) {
    ffi_guard_void(|| {
        if list.is_null() {
            return;
        }
        unsafe {
            drop(Box::from_raw(list));
        }
    });
}

/// Element tag name. Borrowed from store-owned HTML.
///
/// # Safety
///
/// `owner` must point to a live [`ScahElementList`]. `element` must be a
/// bounds-valid store element id. `out_name` must be non-null and valid for
/// writing one [`ScahStringView`]. The returned view is valid only while
/// `owner` remains alive. When `out_error` is non-null, it must be valid for
/// writing one `*mut ScahError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_name(
    owner: *const ScahElementList,
    element: ScahElementId,
    out_name: *mut ScahStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    // Audited panic-free leaf: no allocation, no indexing panic, no UTF-8
    // parse, no OnceLock. Other getters stay behind catch_unwind.
    unsafe {
        clear_out_error(out_error);
        leaf_name_status(owner, element, out_name)
    }
}

/// Fill tag names for many element IDs in one call.
///
/// Writes `count` borrowed name views into `out_names`. Prefer this over
/// repeated [`scah_element_name`] when materializing a large result list.
///
/// # Safety
///
/// `owner` must point to a live [`ScahElementList`]. `ids` and `out_names` must
/// each be valid for `count` elements. Returned views remain valid while
/// `owner` remains alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_names_fill(
    owner: *const ScahElementList,
    ids: *const ScahElementId,
    count: usize,
    out_names: *mut ScahStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    // Iteration over caller IDs — always catch_unwind.
    unsafe {
        ffi_guard(out_error, || {
            let list = require_ref(owner)?;
            if count > 0 && (ids.is_null() || out_names.is_null()) {
                return Err(ScahStatus::NullPointer);
            }
            for i in 0..count {
                let id = *ids.add(i);
                let el = resolve_element(list, id)?;
                *out_names.add(i) = ScahStringView::borrow(el.name);
            }
            Ok(())
        })
    }
}

/// Element `id` attribute, if present.
///
/// # Safety
///
/// Same owner/element requirements as [`scah_element_name`]. `out_id` must be
/// non-null. Returned string data is valid only while `owner` remains alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_id(
    owner: *const ScahElementList,
    element: ScahElementId,
    out_id: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_error(out_error);
        leaf_optional_field_status(owner, element, out_id, |el, _| el.id)
    }
}

/// Element `class` attribute, if present.
///
/// # Safety
///
/// Same requirements as [`scah_element_id`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_class_name(
    owner: *const ScahElementList,
    element: ScahElementId,
    out_class: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_error(out_error);
        leaf_optional_field_status(owner, element, out_class, |el, _| el.class)
    }
}

/// Element inner HTML, if captured.
///
/// # Safety
///
/// Same requirements as [`scah_element_id`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_inner_html(
    owner: *const ScahElementList,
    element: ScahElementId,
    out_html: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_error(out_error);
        leaf_optional_field_status(owner, element, out_html, |el, _| el.inner_html)
    }
}

/// Element text content, if captured.
///
/// # Safety
///
/// Same requirements as [`scah_element_id`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_text_content(
    owner: *const ScahElementList,
    element: ScahElementId,
    out_text: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_error(out_error);
        leaf_optional_field_status(owner, element, out_text, |el, store| {
            el.text_content(store.store())
        })
    }
}

/// Fixed-field element snapshot in one ABI call.
///
/// # Safety
///
/// Same owner/element requirements as [`scah_element_name`]. `out_view` must be
/// non-null. All string views remain valid while `owner` remains alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_view(
    owner: *const ScahElementList,
    element: ScahElementId,
    out_view: *mut ScahElementView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_element_view(out_view);
        ffi_guard_leaf(out_error, || {
            let list = require_ref(owner)?;
            if out_view.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = resolve_element(list, element)?;
            let attribute_count = el
                .attributes(list.store.store())
                .map(|attrs| attrs.len())
                .unwrap_or(0);
            *out_view = ScahElementView {
                name: ScahStringView::borrow(el.name),
                id: ScahOptionalStringView::from_option(el.id),
                class_name: ScahOptionalStringView::from_option(el.class),
                inner_html: ScahOptionalStringView::from_option(el.inner_html),
                text_content: ScahOptionalStringView::from_option(
                    el.text_content(list.store.store()),
                ),
                attribute_count,
            };
            Ok(())
        })
    }
}

/// Look up a single attribute by name.
///
/// # Safety
///
/// `owner` must point to a live [`ScahElementList`]. `key` must satisfy
/// [`ScahStringView::as_str`]. `out_value` must be non-null. Returned string
/// data is valid only while `owner` remains alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_get_attribute(
    owner: *const ScahElementList,
    element: ScahElementId,
    key: ScahStringView,
    out_value: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_error(out_error);
        if out_value.is_null() {
            return ScahStatus::NullPointer;
        }
        *out_value = ScahOptionalStringView::none();
        if owner.is_null() {
            return ScahStatus::NullPointer;
        }
        let list = &*owner;
        let key = match parse_string_view_unchecked(key) {
            Ok(k) => k,
            Err(status) => return status,
        };
        let Some(el) = list.store.store().elements.get(element) else {
            return ScahStatus::IndexOutOfBounds;
        };
        *out_value = ScahOptionalStringView::from_option(el.attribute(list.store.store(), key));
        ScahStatus::Ok
    }
}

/// Number of extra attributes (excluding dedicated class/id fields).
///
/// # Safety
///
/// `owner` must point to a live [`ScahElementList`]. `out_count` must be
/// non-null and valid for writing one `usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_attribute_count(
    owner: *const ScahElementList,
    element: ScahElementId,
    out_count: *mut usize,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_usize(out_count);
        ffi_guard_leaf(out_error, || {
            let list = require_ref(owner)?;
            if out_count.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = resolve_element(list, element)?;
            let count = el
                .attributes(list.store.store())
                .map(|attrs| attrs.len())
                .unwrap_or(0);
            *out_count = count;
            Ok(())
        })
    }
}

/// Indexed attribute access without building a hash map.
///
/// Distinguishes missing attribute values from explicitly empty ones:
///
/// - `out_value.is_some == 0`: attribute had no value (`None`)
/// - `out_value.is_some != 0` with `out_value.value.len == 0`: explicitly empty (`Some("")`)
///
/// # Safety
///
/// Same owner/element requirements as [`scah_element_name`]. `out_key` and
/// `out_value` must be non-null. Returned string data is valid only while
/// `owner` remains alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_attribute_at(
    owner: *const ScahElementList,
    element: ScahElementId,
    index: usize,
    out_key: *mut ScahStringView,
    out_value: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_string_view(out_key);
        clear_out_optional_string_view(out_value);
        ffi_guard_leaf(out_error, || {
            let list = require_ref(owner)?;
            if out_key.is_null() || out_value.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = resolve_element(list, element)?;
            let attrs = el
                .attributes(list.store.store())
                .ok_or(ScahStatus::IndexOutOfBounds)?;
            let attr = attrs.get(index).ok_or(ScahStatus::IndexOutOfBounds)?;
            *out_key = ScahStringView::borrow(attr.key);
            *out_value = ScahOptionalStringView::from_option(attr.value);
            Ok(())
        })
    }
}

/// Fill a caller-provided buffer with borrowed attribute views.
///
/// When `capacity` is smaller than the attribute count, returns
/// [`ScahStatus::BufferTooSmall`] and writes the required count to
/// `*out_written` when that pointer is non-null.
///
/// # Safety
///
/// `owner` must point to a live [`ScahElementList`]. When `capacity > 0`,
/// `out_attributes` must point to a writable array of `capacity`
/// [`ScahAttributeView`] values. `out_written` must be non-null. Returned
/// string data is valid only while `owner` remains alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_attributes_fill(
    owner: *const ScahElementList,
    element: ScahElementId,
    out_attributes: *mut ScahAttributeView,
    capacity: usize,
    out_written: *mut usize,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    // Audited panic-free leaf: bounds-checked gets only, no allocation.
    unsafe {
        clear_out_error(out_error);
        leaf_attributes_fill_status(owner, element, out_attributes, capacity, out_written)
    }
}

/// Nested query lookup on an element.
///
/// When the child query is absent, `*out_found` is 0 (not an error).
///
/// # Safety
///
/// `owner` must point to a live [`ScahElementList`]. `query` must satisfy
/// [`ScahStringView::as_str`]. `out_elements` and `out_found` must be non-null
/// and valid for writing. On success with `*out_found == 1`, the caller owns
/// the child list and must free it with [`scah_element_list_free`]. The child
/// list retains its own store `Arc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_get(
    owner: *const ScahElementList,
    element: ScahElementId,
    query: ScahStringView,
    out_elements: *mut *mut ScahElementList,
    out_found: *mut u8,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_ptr(out_elements);
        clear_out_u8(out_found);
        ffi_guard(out_error, || {
            let list = require_ref(owner)?;
            if out_elements.is_null() || out_found.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let query = parse_string_view(query, out_error)?;
            let el = resolve_element(list, element)?;
            match el.result_meta(list.store.store(), query) {
                None => {
                    *out_found = 0;
                    *out_elements = std::ptr::null_mut();
                    Ok(())
                }
                Some((first, len)) => {
                    *out_found = 1;
                    write_ptr(
                        out_elements,
                        Box::new(new_element_list(
                            list.store.clone(),
                            Some(first.index()),
                            len,
                        )),
                    )?;
                    Ok(())
                }
            }
        })
    }
}

/// Nested lookup that returns span metadata without allocating a child list.
///
/// # Safety
///
/// Same owner/element/query requirements as [`scah_element_get`]. `out_first`,
/// `out_len`, and `out_found` must be non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_get_span(
    owner: *const ScahElementList,
    element: ScahElementId,
    query: ScahStringView,
    out_first: *mut ScahElementId,
    out_len: *mut usize,
    out_found: *mut u8,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_usize(out_len);
        clear_out_u8(out_found);
        if !out_first.is_null() {
            *out_first = 0;
        }
        ffi_guard(out_error, || {
            let list = require_ref(owner)?;
            if out_first.is_null() || out_len.is_null() || out_found.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let query = parse_string_view(query, out_error)?;
            let el = resolve_element(list, element)?;
            match el.result_meta(list.store.store(), query) {
                None => {
                    *out_found = 0;
                    Ok(())
                }
                Some((first, len)) => {
                    *out_found = 1;
                    *out_first = first.index();
                    *out_len = len;
                    Ok(())
                }
            }
        })
    }
}

/// Nested lookup that copies child IDs into a caller buffer in one pass.
///
/// # Safety
///
/// Same requirements as [`scah_element_get`]. When `capacity > 0`, `out_ids`
/// must be writable for `capacity` IDs. `out_written` and `out_found` must be
/// non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_get_ids_fill(
    owner: *const ScahElementList,
    element: ScahElementId,
    query: ScahStringView,
    out_ids: *mut ScahElementId,
    capacity: usize,
    out_written: *mut usize,
    out_elements: *mut *mut ScahElementList,
    out_found: *mut u8,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_usize(out_written);
        clear_out_u8(out_found);
        clear_out_ptr(out_elements);
        clear_out_error(out_error);

        // Binding nested hot path: no child list allocation.
        if out_elements.is_null() {
            if owner.is_null() || out_written.is_null() || out_found.is_null() {
                return ScahStatus::NullPointer;
            }
            if capacity > 0 && out_ids.is_null() {
                return ScahStatus::NullPointer;
            }
            let list = &*owner;
            let query = match parse_string_view_unchecked(query) {
                Ok(q) => q,
                Err(status) => return status,
            };
            let Some(el) = list.store.store().elements.get(element) else {
                return ScahStatus::IndexOutOfBounds;
            };
            return match child_match_ids(&list.store, el, query, out_ids, capacity) {
                None => {
                    *out_found = 0;
                    *out_written = 0;
                    ScahStatus::Ok
                }
                Some((_first, len)) => {
                    *out_found = 1;
                    *out_written = len;
                    if len > capacity {
                        ScahStatus::BufferTooSmall
                    } else {
                        ScahStatus::Ok
                    }
                }
            };
        }

        ffi_guard(out_error, || {
            let list = require_ref(owner)?;
            if out_written.is_null() || out_found.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            if capacity > 0 && out_ids.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let query = parse_string_view(query, out_error)?;
            let el = resolve_element(list, element)?;
            match child_match_ids(&list.store, el, query, out_ids, capacity) {
                None => {
                    *out_found = 0;
                    *out_written = 0;
                    Ok(())
                }
                Some((first, len)) => {
                    *out_found = 1;
                    *out_written = len;
                    if len > capacity {
                        return Err(ScahStatus::BufferTooSmall);
                    }
                    write_ptr(
                        out_elements,
                        Box::new(new_element_list(list.store.clone(), first, len)),
                    )?;
                    Ok(())
                }
            }
        })
    }
}

/// Element tag name via store handle (no result-list owner required).
///
/// # Safety
///
/// `store` must point to a live [`ScahStore`]. `element` must be a bounds-valid
/// store element id. `out_name` must be non-null and valid for writing one
/// [`ScahStringView`]. The returned view is valid only while `store` remains
/// alive. When `out_error` is non-null, it must be valid for writing one
/// `*mut ScahError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_element_name(
    store: *const ScahStore,
    element: ScahElementId,
    out_name: *mut ScahStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_error(out_error);
        leaf_store_name_status(store, element, out_name)
    }
}

/// Element `id` attribute via store handle.
///
/// # Safety
///
/// Same store/element requirements as [`scah_store_element_name`]. `out_id`
/// must be non-null. Returned string data is valid only while `store` remains
/// alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_element_id(
    store: *const ScahStore,
    element: ScahElementId,
    out_id: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_error(out_error);
        leaf_store_optional_field_status(store, element, out_id, |el, _| el.id)
    }
}

/// Element `class` attribute via store handle.
///
/// # Safety
///
/// Same requirements as [`scah_store_element_id`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_element_class_name(
    store: *const ScahStore,
    element: ScahElementId,
    out_class: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_error(out_error);
        leaf_store_optional_field_status(store, element, out_class, |el, _| el.class)
    }
}

/// Element inner HTML via store handle.
///
/// # Safety
///
/// Same requirements as [`scah_store_element_id`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_element_inner_html(
    store: *const ScahStore,
    element: ScahElementId,
    out_html: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_error(out_error);
        leaf_store_optional_field_status(store, element, out_html, |el, _| el.inner_html)
    }
}

/// Element text content via store handle.
///
/// # Safety
///
/// Same requirements as [`scah_store_element_id`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_element_text_content(
    store: *const ScahStore,
    element: ScahElementId,
    out_text: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_error(out_error);
        leaf_store_optional_field_status(store, element, out_text, |el, store| {
            el.text_content(store.store())
        })
    }
}

/// Look up a single attribute by name via store handle.
///
/// # Safety
///
/// `store` must point to a live [`ScahStore`]. `key` must satisfy
/// [`ScahStringView::as_str`]. `out_value` must be non-null. Returned string
/// data is valid only while `store` remains alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_element_get_attribute(
    store: *const ScahStore,
    element: ScahElementId,
    key: ScahStringView,
    out_value: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_error(out_error);
        if out_value.is_null() {
            return ScahStatus::NullPointer;
        }
        *out_value = ScahOptionalStringView::none();
        if store.is_null() {
            return ScahStatus::NullPointer;
        }
        let key = match parse_string_view_unchecked(key) {
            Ok(k) => k,
            Err(status) => return status,
        };
        let store = &*store;
        let Some(el) = store.inner.store().elements.get(element) else {
            return ScahStatus::IndexOutOfBounds;
        };
        *out_value = ScahOptionalStringView::from_option(el.attribute(store.inner.store(), key));
        ScahStatus::Ok
    }
}

/// Fill extra attributes via store handle.
///
/// # Safety
///
/// `store` must point to a live [`ScahStore`]. When `capacity > 0`,
/// `out_attributes` must point to a writable array of `capacity`
/// [`ScahAttributeView`] values. `out_written` must be non-null. Returned
/// string data is valid only while `store` remains alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_element_attributes_fill(
    store: *const ScahStore,
    element: ScahElementId,
    out_attributes: *mut ScahAttributeView,
    capacity: usize,
    out_written: *mut usize,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_error(out_error);
        leaf_store_attributes_fill_status(store, element, out_attributes, capacity, out_written)
    }
}

/// Nested lookup via store handle (no child list allocation).
///
/// # Safety
///
/// `store` must point to a live [`ScahStore`]. `query` must satisfy
/// [`ScahStringView::as_str`]. When `capacity > 0`, `out_ids` must be writable
/// for `capacity` IDs. `out_written` and `out_found` must be non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_element_get_ids_fill(
    store: *const ScahStore,
    element: ScahElementId,
    query: ScahStringView,
    out_ids: *mut ScahElementId,
    capacity: usize,
    out_written: *mut usize,
    out_found: *mut u8,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_usize(out_written);
        clear_out_u8(out_found);
        clear_out_error(out_error);
        if store.is_null() || out_written.is_null() || out_found.is_null() {
            return ScahStatus::NullPointer;
        }
        if capacity > 0 && out_ids.is_null() {
            return ScahStatus::NullPointer;
        }
        let store = &*store;
        let query = match parse_string_view_unchecked(query) {
            Ok(q) => q,
            Err(status) => return status,
        };
        let Some(el) = store.inner.store().elements.get(element) else {
            return ScahStatus::IndexOutOfBounds;
        };
        match child_match_ids(&store.inner, el, query, out_ids, capacity) {
            None => {
                *out_found = 0;
                *out_written = 0;
                ScahStatus::Ok
            }
            Some((_first, len)) => {
                *out_found = 1;
                *out_written = len;
                if len > capacity {
                    ScahStatus::BufferTooSmall
                } else {
                    ScahStatus::Ok
                }
            }
        }
    }
}

/// Fixed-field element snapshot via store handle.
///
/// # Safety
///
/// Same store/element requirements as [`scah_store_element_name`]. `out_view`
/// must be non-null. All string views remain valid while `store` remains alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_store_element_view(
    store: *const ScahStore,
    element: ScahElementId,
    out_view: *mut ScahElementView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_element_view(out_view);
        ffi_guard_leaf(out_error, || {
            let store = require_ref(store)?;
            if out_view.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = store
                .inner
                .store()
                .elements
                .get(element)
                .ok_or(ScahStatus::IndexOutOfBounds)?;
            let attribute_count = el
                .attributes(store.inner.store())
                .map(|attrs| attrs.len())
                .unwrap_or(0);
            *out_view = ScahElementView {
                name: ScahStringView::borrow(el.name),
                id: ScahOptionalStringView::from_option(el.id),
                class_name: ScahOptionalStringView::from_option(el.class),
                inner_html: ScahOptionalStringView::from_option(el.inner_html),
                text_content: ScahOptionalStringView::from_option(
                    el.text_content(store.inner.store()),
                ),
                attribute_count,
            };
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::scah_error_free;
    use crate::query::{
        ScahQueryBuilder, scah_query_all, scah_query_builder_append, scah_query_builder_build,
        scah_query_builder_current_section, scah_query_builder_free, scah_query_free,
    };
    use crate::string::{scah_save_all, scah_save_only_text_content};

    fn view(s: &str) -> ScahStringView {
        ScahStringView::borrow(s)
    }

    fn build_simple_query(selector: &str) -> *mut ScahQuery {
        let mut builder: *mut ScahQueryBuilder = std::ptr::null_mut();
        let mut query: *mut ScahQuery = std::ptr::null_mut();
        let mut err: *mut ScahError = std::ptr::null_mut();
        assert_eq!(
            unsafe { scah_query_all(view(selector), scah_save_all(), &mut builder, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(
            unsafe { scah_query_builder_build(builder, &mut query, &mut err) },
            ScahStatus::Ok
        );
        unsafe {
            scah_query_builder_free(builder);
        }
        query
    }

    fn first_id(list: *const ScahElementList) -> ScahElementId {
        let mut ids: *const ScahElementId = std::ptr::null();
        let mut len = 0usize;
        let mut err: *mut ScahError = std::ptr::null_mut();
        assert_eq!(
            unsafe { scah_element_list_ids(list, &mut ids, &mut len, &mut err) },
            ScahStatus::Ok
        );
        assert!(len >= 1);
        unsafe { *ids }
    }

    #[test]
    fn parse_get_and_free_orders() {
        let query = build_simple_query("a");
        let html = b"<div><a href=\"https://ex.com\" class=\"c\" id=\"i\">hi</a></div>";
        let mut store: *mut ScahStore = std::ptr::null_mut();
        let mut err: *mut ScahError = std::ptr::null_mut();
        let queries = [query as *const ScahQuery];
        assert_eq!(
            unsafe {
                scah_parse(
                    ScahStringView {
                        data: html.as_ptr(),
                        len: html.len(),
                    },
                    queries.as_ptr(),
                    1,
                    &mut store,
                    &mut err,
                )
            },
            ScahStatus::Ok
        );
        unsafe {
            scah_query_free(query);
        }

        let mut list: *mut ScahElementList = std::ptr::null_mut();
        let mut found = 0u8;
        assert_eq!(
            unsafe { scah_store_get(store, view("a"), &mut list, &mut found, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(found, 1);

        let mut ids: *const ScahElementId = std::ptr::null();
        let mut len = 0usize;
        assert_eq!(
            unsafe { scah_element_list_ids(list, &mut ids, &mut len, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(len, 1);
        let element = unsafe { *ids };

        // Free store while list remains alive.
        unsafe {
            scah_store_free(store);
        }

        let mut name = ScahStringView::empty();
        assert_eq!(
            unsafe { scah_element_name(list, element, &mut name, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(
            unsafe { std::str::from_utf8(std::slice::from_raw_parts(name.data, name.len)) }
                .unwrap(),
            "a"
        );

        let mut href = ScahOptionalStringView::none();
        assert_eq!(
            unsafe { scah_element_get_attribute(list, element, view("href"), &mut href, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(href.is_some, 1);

        let mut text = ScahOptionalStringView::none();
        assert_eq!(
            unsafe { scah_element_text_content(list, element, &mut text, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(text.is_some, 1);

        // Borrowed ID pointer remains valid until list free.
        assert_eq!(unsafe { *ids }, element);

        unsafe {
            scah_element_list_free(list);
        }
    }

    #[test]
    fn empty_queries_and_not_found() {
        let mut store: *mut ScahStore = std::ptr::null_mut();
        let mut err: *mut ScahError = std::ptr::null_mut();
        let status =
            unsafe { scah_parse(view("<a></a>"), std::ptr::null(), 0, &mut store, &mut err) };
        assert_eq!(status, ScahStatus::EmptyQueries);
        unsafe {
            scah_error_free(err);
        }

        let query = build_simple_query("a");
        let queries = [query as *const ScahQuery];
        assert_eq!(
            unsafe {
                scah_parse(
                    view("<div></div>"),
                    queries.as_ptr(),
                    1,
                    &mut store,
                    &mut err,
                )
            },
            ScahStatus::Ok
        );
        let mut list: *mut ScahElementList = std::ptr::null_mut();
        let mut found = 1u8;
        assert_eq!(
            unsafe { scah_store_get(store, view("missing"), &mut list, &mut found, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(found, 0);
        assert!(list.is_null());

        unsafe {
            scah_query_free(query);
            scah_store_free(store);
            scah_store_free(std::ptr::null_mut());
            scah_element_list_free(std::ptr::null_mut());
        }
    }

    fn sentinel_string() -> ScahStringView {
        ScahStringView::borrow("sentinel")
    }

    fn sentinel_optional() -> ScahOptionalStringView {
        ScahOptionalStringView {
            value: sentinel_string(),
            is_some: 1,
        }
    }

    fn sentinel_view() -> ScahElementView {
        ScahElementView {
            name: sentinel_string(),
            id: sentinel_optional(),
            class_name: sentinel_optional(),
            inner_html: sentinel_optional(),
            text_content: sentinel_optional(),
            attribute_count: 99,
        }
    }

    fn assert_string_cleared(view: ScahStringView) {
        assert!(view.data.is_null());
        assert_eq!(view.len, 0);
    }

    fn assert_optional_cleared(view: ScahOptionalStringView) {
        assert_eq!(view.is_some, 0);
        assert_string_cleared(view.value);
    }

    fn assert_element_view_cleared(view: ScahElementView) {
        assert_string_cleared(view.name);
        assert_optional_cleared(view.id);
        assert_optional_cleared(view.class_name);
        assert_optional_cleared(view.inner_html);
        assert_optional_cleared(view.text_content);
        assert_eq!(view.attribute_count, 0);
    }

    #[test]
    fn invalid_element_id_and_null_owner_clear_outputs() {
        let mut builder: *mut ScahQueryBuilder = std::ptr::null_mut();
        let mut query: *mut ScahQuery = std::ptr::null_mut();
        let mut err: *mut ScahError = std::ptr::null_mut();
        unsafe {
            scah_query_all(
                view("a"),
                scah_save_only_text_content(),
                &mut builder,
                &mut err,
            );
            scah_query_builder_build(builder, &mut query, &mut err);
            scah_query_builder_free(builder);
        }

        let mut store: *mut ScahStore = std::ptr::null_mut();
        let queries = [query as *const ScahQuery];
        unsafe {
            scah_parse(view("<a></a>"), queries.as_ptr(), 1, &mut store, &mut err);
            scah_query_free(query);
        }

        let mut list: *mut ScahElementList = std::ptr::null_mut();
        let mut found = 0u8;
        unsafe {
            scah_store_get(store, view("a"), &mut list, &mut found, &mut err);
        }
        let element = first_id(list);

        let cases: &[(*const ScahElementList, ScahElementId)] =
            &[(list, usize::MAX), (std::ptr::null(), element)];

        for &(owner, id) in cases {
            let mut name = sentinel_string();
            assert_ne!(
                unsafe { scah_element_name(owner, id, &mut name, &mut err) },
                ScahStatus::Ok
            );
            assert_string_cleared(name);

            let mut opt = sentinel_optional();
            assert_ne!(
                unsafe { scah_element_id(owner, id, &mut opt, &mut err) },
                ScahStatus::Ok
            );
            assert_optional_cleared(opt);

            opt = sentinel_optional();
            assert_ne!(
                unsafe { scah_element_class_name(owner, id, &mut opt, &mut err) },
                ScahStatus::Ok
            );
            assert_optional_cleared(opt);

            opt = sentinel_optional();
            assert_ne!(
                unsafe { scah_element_inner_html(owner, id, &mut opt, &mut err) },
                ScahStatus::Ok
            );
            assert_optional_cleared(opt);

            opt = sentinel_optional();
            assert_ne!(
                unsafe { scah_element_text_content(owner, id, &mut opt, &mut err) },
                ScahStatus::Ok
            );
            assert_optional_cleared(opt);

            opt = sentinel_optional();
            assert_ne!(
                unsafe { scah_element_get_attribute(owner, id, view("href"), &mut opt, &mut err) },
                ScahStatus::Ok
            );
            assert_optional_cleared(opt);

            let mut key = sentinel_string();
            let mut value = sentinel_optional();
            assert_ne!(
                unsafe { scah_element_attribute_at(owner, id, 0, &mut key, &mut value, &mut err) },
                ScahStatus::Ok
            );
            assert_string_cleared(key);
            assert_optional_cleared(value);

            let mut snap = sentinel_view();
            assert_ne!(
                unsafe { scah_element_view(owner, id, &mut snap, &mut err) },
                ScahStatus::Ok
            );
            assert_element_view_cleared(snap);
        }

        // Null required outputs still clear nothing unsafe; status is NullPointer.
        let name = sentinel_string();
        assert_eq!(
            unsafe { scah_element_name(list, element, std::ptr::null_mut(), &mut err) },
            ScahStatus::NullPointer
        );
        // Caller buffer was not passed; local sentinel is untouched.
        assert_eq!(unsafe { name.as_str().unwrap() }, "sentinel");

        unsafe {
            scah_element_list_free(list);
            scah_store_free(store);
        }
    }

    #[test]
    fn nested_get_via_branches() {
        let mut root: *mut ScahQueryBuilder = std::ptr::null_mut();
        let mut child: *mut ScahQueryBuilder = std::ptr::null_mut();
        let mut err: *mut ScahError = std::ptr::null_mut();
        unsafe {
            scah_query_all(view("div"), scah_save_all(), &mut root, &mut err);
            scah_query_all(view("a"), scah_save_all(), &mut child, &mut err);
            let mut parent = 0usize;
            scah_query_builder_current_section(root, &mut parent, &mut err);
            scah_query_builder_append(root, parent, child, &mut err);

            let mut query: *mut ScahQuery = std::ptr::null_mut();
            scah_query_builder_build(root, &mut query, &mut err);
            scah_query_builder_free(root);
            scah_query_builder_free(child);

            let mut store: *mut ScahStore = std::ptr::null_mut();
            let queries = [query as *const ScahQuery];
            scah_parse(
                view("<div><a href='1'>x</a></div>"),
                queries.as_ptr(),
                1,
                &mut store,
                &mut err,
            );
            scah_query_free(query);

            let mut list: *mut ScahElementList = std::ptr::null_mut();
            let mut found = 0u8;
            scah_store_get(store, view("div"), &mut list, &mut found, &mut err);
            let div = first_id(list);

            let mut children: *mut ScahElementList = std::ptr::null_mut();
            scah_element_get(list, div, view("a"), &mut children, &mut found, &mut err);
            assert_eq!(found, 1);
            let mut len = 0usize;
            scah_element_list_len(children, &mut len, &mut err);
            assert_eq!(len, 1);

            // Free original store; child list retains the store.
            scah_store_free(store);
            let child_id = first_id(children);
            let mut name = ScahStringView::empty();
            assert_eq!(
                scah_element_name(children, child_id, &mut name, &mut err),
                ScahStatus::Ok
            );

            scah_element_list_free(children);
            scah_element_list_free(list);
        }
    }

    #[test]
    fn missing_nested_query_sets_found_zero() {
        let query = build_simple_query("div");
        let mut store: *mut ScahStore = std::ptr::null_mut();
        let mut err: *mut ScahError = std::ptr::null_mut();
        let queries = [query as *const ScahQuery];
        unsafe {
            scah_parse(
                view("<div></div>"),
                queries.as_ptr(),
                1,
                &mut store,
                &mut err,
            );
            scah_query_free(query);
        }
        let mut list: *mut ScahElementList = std::ptr::null_mut();
        let mut found = 1u8;
        unsafe {
            scah_store_get(store, view("div"), &mut list, &mut found, &mut err);
        }
        let div = first_id(list);
        let mut children: *mut ScahElementList = std::ptr::null_mut();
        found = 1;
        assert_eq!(
            unsafe { scah_element_get(list, div, view("a"), &mut children, &mut found, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(found, 0);
        assert!(children.is_null());
        unsafe {
            scah_element_list_free(list);
            scah_store_free(store);
        }
    }

    #[test]
    fn abi_version_is_one() {
        assert_eq!(scah_abi_version(), 1);
    }

    #[test]
    fn store_owns_html_after_caller_buffer_destroyed() {
        let query = build_simple_query("input");
        let mut store: *mut ScahStore = std::ptr::null_mut();
        let mut err: *mut ScahError = std::ptr::null_mut();

        {
            let mut html = String::from("<input disabled value=\"\">");
            let view = ScahStringView {
                data: html.as_ptr(),
                len: html.len(),
            };
            let queries = [query as *const ScahQuery];
            assert_eq!(
                unsafe { scah_parse(view, queries.as_ptr(), 1, &mut store, &mut err) },
                ScahStatus::Ok
            );
            html.replace_range(.., &"X".repeat(html.len()));
            drop(html);
        }

        unsafe {
            scah_query_free(query);
        }

        let mut list: *mut ScahElementList = std::ptr::null_mut();
        let mut found = 0u8;
        assert_eq!(
            unsafe { scah_store_get(store, view("input"), &mut list, &mut found, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(found, 1);

        let element = first_id(list);
        let mut name = ScahStringView::empty();
        assert_eq!(
            unsafe { scah_element_name(list, element, &mut name, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(
            unsafe { std::str::from_utf8(std::slice::from_raw_parts(name.data, name.len)) }
                .unwrap(),
            "input"
        );

        let mut value = ScahOptionalStringView::none();
        assert_eq!(
            unsafe {
                scah_element_get_attribute(list, element, view("value"), &mut value, &mut err)
            },
            ScahStatus::Ok
        );
        assert_eq!(value.is_some, 1);
        assert_eq!(value.value.len, 0);

        unsafe {
            scah_element_list_free(list);
            scah_store_free(store);
        }
    }

    #[test]
    fn attribute_at_and_fill_preserve_missing_versus_empty() {
        let query = build_simple_query("input");
        let mut store: *mut ScahStore = std::ptr::null_mut();
        let mut err: *mut ScahError = std::ptr::null_mut();
        let queries = [query as *const ScahQuery];
        assert_eq!(
            unsafe {
                scah_parse(
                    view("<input disabled value=\"\">"),
                    queries.as_ptr(),
                    1,
                    &mut store,
                    &mut err,
                )
            },
            ScahStatus::Ok
        );
        unsafe {
            scah_query_free(query);
        }

        let mut list: *mut ScahElementList = std::ptr::null_mut();
        let mut found = 0u8;
        unsafe {
            scah_store_get(store, view("input"), &mut list, &mut found, &mut err);
        }
        let element = first_id(list);

        let mut count = 0usize;
        assert_eq!(
            unsafe { scah_element_attribute_count(list, element, &mut count, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(count, 2);

        let mut buf = [ScahAttributeView {
            key: ScahStringView::empty(),
            value: ScahOptionalStringView::none(),
        }; 2];
        let mut written = 0usize;
        assert_eq!(
            unsafe {
                scah_element_attributes_fill(
                    list,
                    element,
                    buf.as_mut_ptr(),
                    buf.len(),
                    &mut written,
                    &mut err,
                )
            },
            ScahStatus::Ok
        );
        assert_eq!(written, 2);

        let mut too_small = 0usize;
        assert_eq!(
            unsafe {
                scah_element_attributes_fill(
                    list,
                    element,
                    buf.as_mut_ptr(),
                    1,
                    &mut too_small,
                    &mut err,
                )
            },
            ScahStatus::BufferTooSmall
        );
        assert_eq!(too_small, 2);
        assert!(err.is_null());

        let mut saw_disabled = false;
        let mut saw_value = false;
        for view in &buf[..written] {
            let key_str = unsafe {
                std::str::from_utf8(std::slice::from_raw_parts(view.key.data, view.key.len))
            }
            .unwrap();
            match key_str {
                "disabled" => {
                    assert_eq!(view.value.is_some, 0);
                    saw_disabled = true;
                }
                "value" => {
                    assert_eq!(view.value.is_some, 1);
                    assert_eq!(view.value.value.len, 0);
                    saw_value = true;
                }
                other => panic!("unexpected attribute key: {other}"),
            }
        }
        assert!(saw_disabled && saw_value);

        let mut snap = ScahElementView {
            name: ScahStringView::empty(),
            id: ScahOptionalStringView::none(),
            class_name: ScahOptionalStringView::none(),
            inner_html: ScahOptionalStringView::none(),
            text_content: ScahOptionalStringView::none(),
            attribute_count: 0,
        };
        assert_eq!(
            unsafe { scah_element_view(list, element, &mut snap, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(snap.attribute_count, 2);

        unsafe {
            scah_element_list_free(list);
            scah_store_free(store);
        }
    }

    #[test]
    fn output_slots_cleared_on_failure() {
        let mut err: *mut ScahError = std::ptr::null_mut();
        let mut found = 1u8;
        let mut list: *mut ScahElementList = std::ptr::null_mut();
        // Force a null store → NullPointer; out slots must be cleared.
        let status =
            unsafe { scah_store_get(std::ptr::null(), view("a"), &mut list, &mut found, &mut err) };
        assert_eq!(status, ScahStatus::NullPointer);
        assert_eq!(found, 0);
        assert!(list.is_null());
        unsafe {
            scah_error_free(err);
        }
    }

    /// Caller-buffer contract: only `written` prefix slots are initialized.
    #[test]
    fn ids_fill_exposes_only_written_prefix() {
        let html = "<a>1</a><a>2</a><b>x</b>";
        let q = build_simple_query("a");
        let mut store: *mut ScahStore = std::ptr::null_mut();
        let mut err: *mut ScahError = std::ptr::null_mut();
        let queries = [q as *const ScahQuery];
        assert_eq!(
            unsafe { scah_parse(view(html), queries.as_ptr(), 1, &mut store, &mut err) },
            ScahStatus::Ok
        );

        // Capacity 16, but only 2 matches — caller must not treat the rest as live.
        let mut ids = [usize::MAX; 16];
        let mut written = 0usize;
        let mut list: *mut ScahElementList = std::ptr::null_mut();
        let mut found = 0u8;
        assert_eq!(
            unsafe {
                scah_store_get_ids_fill(
                    store,
                    view("a"),
                    ids.as_mut_ptr(),
                    std::ptr::null_mut(),
                    ids.len(),
                    &mut written,
                    &mut list,
                    &mut found,
                    &mut err,
                )
            },
            ScahStatus::Ok
        );
        assert_eq!(found, 1);
        assert_eq!(written, 2);
        assert_ne!(ids[0], usize::MAX);
        assert_ne!(ids[1], usize::MAX);
        // Unwritten suffix must remain untouched.
        assert!(ids[2..].iter().all(|&id| id == usize::MAX));

        // Exact small capacity then BufferTooSmall: written reports full count,
        // no owned list transfer, and no diagnostic allocation.
        let mut tiny = [0usize; 1];
        let mut need = 0usize;
        found = 0;
        let mut list2: *mut ScahElementList = std::ptr::null_mut();
        err = std::ptr::null_mut();
        assert_eq!(
            unsafe {
                scah_store_get_ids_fill(
                    store,
                    view("a"),
                    tiny.as_mut_ptr(),
                    std::ptr::null_mut(),
                    tiny.len(),
                    &mut need,
                    &mut list2,
                    &mut found,
                    &mut err,
                )
            },
            ScahStatus::BufferTooSmall
        );
        assert_eq!(need, 2);
        assert_eq!(found, 1);
        assert!(list2.is_null());
        assert!(err.is_null());

        // Successful list-first path: get span list, then fill IDs once.
        let mut list3: *mut ScahElementList = std::ptr::null_mut();
        found = 0;
        assert_eq!(
            unsafe { scah_store_get(store, view("a"), &mut list3, &mut found, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(found, 1);
        let mut filled = [0usize; 2];
        let mut filled_n = 0usize;
        assert_eq!(
            unsafe {
                scah_element_list_fill_ids(list3, filled.as_mut_ptr(), 2, &mut filled_n, &mut err)
            },
            ScahStatus::Ok
        );
        assert_eq!(filled_n, 2);
        assert_eq!(filled[0], ids[0]);
        assert_eq!(filled[1], ids[1]);

        // Span API: O(1) metadata + single fill, no list allocation.
        let mut first = 0usize;
        let mut span_len = 0usize;
        found = 0;
        assert_eq!(
            unsafe {
                scah_store_get_span(
                    store,
                    view("a"),
                    &mut first,
                    &mut span_len,
                    &mut found,
                    &mut err,
                )
            },
            ScahStatus::Ok
        );
        assert_eq!(found, 1);
        assert_eq!(span_len, 2);
        let mut span_ids = [0usize; 2];
        let mut span_written = 0usize;
        assert_eq!(
            unsafe {
                scah_store_fill_ids(
                    store,
                    first,
                    span_len,
                    span_ids.as_mut_ptr(),
                    2,
                    &mut span_written,
                    &mut err,
                )
            },
            ScahStatus::Ok
        );
        assert_eq!(span_written, 2);
        assert_eq!(span_ids, filled);

        unsafe {
            scah_element_list_free(list);
            scah_element_list_free(list3);
            scah_store_free(store);
            scah_query_free(q);
        }
    }

    /// Dependency-free buffer finalization helper for Miri / binding reuse.
    #[test]
    fn finalize_filled_vec_exposes_only_written() {
        unsafe fn finalize_filled_vec<T>(buf: &mut Vec<T>, written: usize) {
            debug_assert!(written <= buf.capacity());
            unsafe {
                buf.set_len(written);
            }
        }

        let capacity = 16usize;
        let mut ids: Vec<usize> = Vec::with_capacity(capacity);
        unsafe {
            *ids.as_mut_ptr().add(0) = 10;
            *ids.as_mut_ptr().add(1) = 20;
            finalize_filled_vec(&mut ids, 2);
        }
        assert_eq!(ids, vec![10, 20]);
    }
}
