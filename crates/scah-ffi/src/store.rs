//! Opaque store/element-list handles and C ABI entry points.
//!
//! Result elements are exposed as borrowed [`ScahElementId`] values owned by a
//! [`ScahElementList`]. There is no per-result C heap allocation: language
//! bindings keep one list owner and copy integer IDs.

use crate::error::{ScahError, ScahStatus, ffi_guard, ffi_guard_value, ffi_guard_void, set_error};
use crate::owned_store::OwnedStore;
use crate::query::ScahQuery;
use crate::string::{ScahOptionalStringView, ScahStringView};
use scah::{ElementId, ParseError};
use std::sync::Arc;

/// Opaque parsed store handle.
pub struct ScahStore {
    pub(crate) inner: Arc<OwnedStore>,
}

/// Store-local element identifier. Valid only with the [`ScahElementList`] that
/// produced it (or another list retaining the same store for bounds-checked
/// access). Not a heap handle.
pub type ScahElementId = usize;

/// Opaque list of element ids sharing a store.
///
/// # Lifetime invariants
///
/// - [`scah_element_list_ids`] returns a pointer into this list's immutable
///   `ids` vector. The pointer remains valid until the list is freed.
/// - IDs never mutate after list creation.
/// - String views obtained via element getters remain valid while this list
///   (which retains the [`OwnedStore`]) remains alive.
/// - Freeing the original [`ScahStore`] does not invalidate elements accessed
///   through this list.
/// - An element ID must not be used with a different list owner.
pub struct ScahElementList {
    pub(crate) store: Arc<OwnedStore>,
    pub(crate) ids: Vec<ScahElementId>,
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

fn collect_match_ids(store: &OwnedStore, query: &str) -> Option<Vec<ScahElementId>> {
    store.store().get(query).map(|iter| {
        // SAFETY: each `e` is borrowed from this store's arena.
        iter.map(|e| {
            let id: ElementId = unsafe { store.store().elements.index_of(e) };
            id.index()
        })
        .collect()
    })
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
        ffi_guard(out_error, || {
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
            match collect_match_ids(&owned, query) {
                None => {
                    *out_found = 0;
                    *out_elements = std::ptr::null_mut();
                    Ok(())
                }
                Some(ids) => {
                    *out_found = 1;
                    write_ptr(
                        out_elements,
                        Box::new(ScahElementList { store: owned, ids }),
                    )?;
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
        ffi_guard(out_error, || {
            let list = require_ref(list)?;
            if out_len.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            *out_len = list.ids.len();
            Ok(())
        })
    }
}

/// Borrow the complete ID slice for a result list.
///
/// The returned pointer remains valid until the list is freed. IDs never mutate
/// after list creation.
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
            *out_ids = list.ids.as_ptr();
            *out_len = list.ids.len();
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
    unsafe {
        ffi_guard(out_error, || {
            let list = require_ref(owner)?;
            if out_name.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = resolve_element(list, element)?;
            *out_name = ScahStringView::borrow(el.name);
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
        ffi_guard(out_error, || {
            let list = require_ref(owner)?;
            if out_id.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = resolve_element(list, element)?;
            *out_id = ScahOptionalStringView::from_option(el.id);
            Ok(())
        })
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
        ffi_guard(out_error, || {
            let list = require_ref(owner)?;
            if out_class.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = resolve_element(list, element)?;
            *out_class = ScahOptionalStringView::from_option(el.class);
            Ok(())
        })
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
        ffi_guard(out_error, || {
            let list = require_ref(owner)?;
            if out_html.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = resolve_element(list, element)?;
            *out_html = ScahOptionalStringView::from_option(el.inner_html);
            Ok(())
        })
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
        ffi_guard(out_error, || {
            let list = require_ref(owner)?;
            if out_text.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = resolve_element(list, element)?;
            let text = el.text_content(list.store.store());
            *out_text = ScahOptionalStringView::from_option(text);
            Ok(())
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
        ffi_guard(out_error, || {
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
        ffi_guard(out_error, || {
            let list = require_ref(owner)?;
            if out_value.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let key = parse_string_view(key, out_error)?;
            let el = resolve_element(list, element)?;
            let value = el.attribute(list.store.store(), key);
            *out_value = ScahOptionalStringView::from_option(value);
            Ok(())
        })
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
        ffi_guard(out_error, || {
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
        ffi_guard(out_error, || {
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
    unsafe {
        clear_out_usize(out_written);
        ffi_guard(out_error, || {
            let list = require_ref(owner)?;
            if out_written.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            if capacity > 0 && out_attributes.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = resolve_element(list, element)?;
            let attrs = el.attributes(list.store.store());
            let count = attrs.map(|a| a.len()).unwrap_or(0);
            if count > capacity {
                *out_written = count;
                set_error(
                    out_error,
                    "attribute buffer capacity is smaller than attribute count",
                );
                return Err(ScahStatus::BufferTooSmall);
            }
            if let Some(attrs) = attrs {
                for (i, attr) in attrs.iter().enumerate() {
                    *out_attributes.add(i) = ScahAttributeView {
                        key: ScahStringView::borrow(attr.key),
                        value: ScahOptionalStringView::from_option(attr.value),
                    };
                }
            }
            *out_written = count;
            Ok(())
        })
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
            match el.get(list.store.store(), query) {
                None => {
                    *out_found = 0;
                    *out_elements = std::ptr::null_mut();
                    Ok(())
                }
                Some(iter) => {
                    let store = list.store.clone();
                    // SAFETY: each `e` is borrowed from this store's arena.
                    let ids = iter
                        .map(|e| store.store().elements.index_of(e).index())
                        .collect();
                    *out_found = 1;
                    write_ptr(out_elements, Box::new(ScahElementList { store, ids }))?;
                    Ok(())
                }
            }
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

    #[test]
    fn invalid_element_id_and_null_owner() {
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

        let mut name = ScahStringView::empty();
        assert_eq!(
            unsafe { scah_element_name(list, usize::MAX, &mut name, &mut err) },
            ScahStatus::IndexOutOfBounds
        );
        assert_eq!(
            unsafe { scah_element_name(std::ptr::null(), element, &mut name, &mut err) },
            ScahStatus::NullPointer
        );

        let mut id = ScahOptionalStringView {
            value: ScahStringView::borrow("sentinel"),
            is_some: 1,
        };
        unsafe {
            scah_element_id(list, element, &mut id, &mut err);
        }
        assert_eq!(id.is_some, 0);

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
        unsafe {
            scah_error_free(err);
            err = std::ptr::null_mut();
        }

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
}
