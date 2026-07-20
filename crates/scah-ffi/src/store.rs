//! Opaque store/element handles and C ABI entry points.

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

/// Opaque element handle. Keeps its store alive.
pub struct ScahElement {
    pub(crate) store: Arc<OwnedStore>,
    pub(crate) id: ElementId,
}

/// Opaque list of element ids sharing a store.
pub struct ScahElementList {
    pub(crate) store: Arc<OwnedStore>,
    pub(crate) ids: Vec<ElementId>,
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

fn element_ref(element: &ScahElement) -> Result<&scah::Element<'static>, ScahStatus> {
    element
        .store
        .store()
        .elements
        .get(element.id.index())
        .ok_or(ScahStatus::IndexOutOfBounds)
}

/// Current C ABI version.
#[unsafe(no_mangle)]
pub extern "C" fn scah_abi_version() -> u32 {
    ffi_guard_value(1, || 1)
}

/// Parse HTML against one or more compiled queries.
///
/// Returned string views from the store/elements borrow store-owned HTML and
/// remain valid only while those handles are alive.
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

            let html = parse_string_view(html, out_error)?.to_owned();

            let mut owned_queries = Vec::with_capacity(query_count);
            for i in 0..query_count {
                let ptr = *queries.add(i);
                let query = require_ref(ptr)?;
                owned_queries.push(query.inner.clone());
            }

            match OwnedStore::parse(&html, &owned_queries) {
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
        ffi_guard(out_error, || {
            let store = require_ref(store)?;
            if out_elements.is_null() || out_found.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let query = parse_string_view(query, out_error)?;
            let owned = store.inner.clone();
            let found_ids: Option<Vec<ElementId>> = owned.store().get(query).map(|iter| {
                // SAFETY: each `e` is borrowed from this store's arena.
                iter.map(|e| owned.store().elements.index_of(e)).collect()
            });
            match found_ids {
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
/// have been freed, and must not be used again after this call. Element and
/// list handles that retain an `Arc` to the same store remain valid.
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

/// Get an element handle from a list. The element retains its own store `Arc`.
///
/// # Safety
///
/// `list` must point to a live [`ScahElementList`]. `out_element` must be
/// non-null and valid for writing one `*mut ScahElement`. When `out_error` is
/// non-null, it must be valid for writing one `*mut ScahError`. On success the
/// caller owns the element and must free it with [`scah_element_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_list_get(
    list: *const ScahElementList,
    index: usize,
    out_element: *mut *mut ScahElement,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_ptr(out_element);
        ffi_guard(out_error, || {
            let list = require_ref(list)?;
            let id = list
                .ids
                .get(index)
                .copied()
                .ok_or(ScahStatus::IndexOutOfBounds)?;
            write_ptr(
                out_element,
                Box::new(ScahElement {
                    store: list.store.clone(),
                    id,
                }),
            )?;
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
/// `element` must point to a live [`ScahElement`]. `out_name` must be non-null
/// and valid for writing one [`ScahStringView`]. The returned view is valid
/// only while the element's store remains alive. When `out_error` is non-null,
/// it must be valid for writing one `*mut ScahError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_name(
    element: *const ScahElement,
    out_name: *mut ScahStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        ffi_guard(out_error, || {
            let element = require_ref(element)?;
            if out_name.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = element_ref(element)?;
            *out_name = ScahStringView::borrow(el.name);
            Ok(())
        })
    }
}

/// Element `id` attribute, if present.
///
/// # Safety
///
/// `element` must point to a live [`ScahElement`]. `out_id` must be non-null
/// and valid for writing one [`ScahOptionalStringView`]. Returned string data
/// is valid only while the element's store remains alive. When `out_error` is
/// non-null, it must be valid for writing one `*mut ScahError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_id(
    element: *const ScahElement,
    out_id: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        ffi_guard(out_error, || {
            let element = require_ref(element)?;
            if out_id.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = element_ref(element)?;
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
    element: *const ScahElement,
    out_class: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        ffi_guard(out_error, || {
            let element = require_ref(element)?;
            if out_class.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = element_ref(element)?;
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
    element: *const ScahElement,
    out_html: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        ffi_guard(out_error, || {
            let element = require_ref(element)?;
            if out_html.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = element_ref(element)?;
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
    element: *const ScahElement,
    out_text: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        ffi_guard(out_error, || {
            let element = require_ref(element)?;
            if out_text.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = element_ref(element)?;
            let text = el.text_content(element.store.store());
            *out_text = ScahOptionalStringView::from_option(text);
            Ok(())
        })
    }
}

/// Look up a single attribute by name.
///
/// # Safety
///
/// `element` must point to a live [`ScahElement`]. `key` must satisfy
/// [`ScahStringView::as_str`]. `out_value` must be non-null and valid for
/// writing one [`ScahOptionalStringView`]. Returned string data is valid only
/// while the element's store remains alive. When `out_error` is non-null, it
/// must be valid for writing one `*mut ScahError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_get_attribute(
    element: *const ScahElement,
    key: ScahStringView,
    out_value: *mut ScahOptionalStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        ffi_guard(out_error, || {
            let element = require_ref(element)?;
            if out_value.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let key = parse_string_view(key, out_error)?;
            let el = element_ref(element)?;
            let value = el.attribute(element.store.store(), key);
            *out_value = ScahOptionalStringView::from_option(value);
            Ok(())
        })
    }
}

/// Number of extra attributes (excluding dedicated class/id fields).
///
/// # Safety
///
/// `element` must point to a live [`ScahElement`]. `out_count` must be
/// non-null and valid for writing one `usize`. When `out_error` is non-null,
/// it must be valid for writing one `*mut ScahError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_attribute_count(
    element: *const ScahElement,
    out_count: *mut usize,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        ffi_guard(out_error, || {
            let element = require_ref(element)?;
            if out_count.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = element_ref(element)?;
            let count = el
                .attributes(element.store.store())
                .map(|attrs| attrs.len())
                .unwrap_or(0);
            *out_count = count;
            Ok(())
        })
    }
}

/// Indexed attribute access without building a hash map.
///
/// When an attribute has no value, `out_value` is an empty string view.
///
/// # Safety
///
/// `element` must point to a live [`ScahElement`]. `out_key` and `out_value`
/// must be non-null and valid for writing. Returned string data is valid only
/// while the element's store remains alive. When `out_error` is non-null, it
/// must be valid for writing one `*mut ScahError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_attribute_at(
    element: *const ScahElement,
    index: usize,
    out_key: *mut ScahStringView,
    out_value: *mut ScahStringView,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        ffi_guard(out_error, || {
            let element = require_ref(element)?;
            if out_key.is_null() || out_value.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let el = element_ref(element)?;
            let attrs = el
                .attributes(element.store.store())
                .ok_or(ScahStatus::IndexOutOfBounds)?;
            let attr = attrs.get(index).ok_or(ScahStatus::IndexOutOfBounds)?;
            *out_key = ScahStringView::borrow(attr.key);
            *out_value = match attr.value {
                Some(v) => ScahStringView::borrow(v),
                None => ScahStringView::empty(),
            };
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
/// `element` must point to a live [`ScahElement`]. `query` must satisfy
/// [`ScahStringView::as_str`]. `out_elements` and `out_found` must be non-null
/// and valid for writing. When `out_error` is non-null, it must be valid for
/// writing one `*mut ScahError`. On success with `*out_found == 1`, the caller
/// owns the list and must free it with [`scah_element_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_get(
    element: *const ScahElement,
    query: ScahStringView,
    out_elements: *mut *mut ScahElementList,
    out_found: *mut u8,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_ptr(out_elements);
        ffi_guard(out_error, || {
            let element = require_ref(element)?;
            if out_elements.is_null() || out_found.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            let query = parse_string_view(query, out_error)?;
            let el = element_ref(element)?;
            match el.get(element.store.store(), query) {
                None => {
                    *out_found = 0;
                    *out_elements = std::ptr::null_mut();
                    Ok(())
                }
                Some(iter) => {
                    let store = element.store.clone();
                    // SAFETY: each `e` is borrowed from this store's arena.
                    let ids = iter.map(|e| store.store().elements.index_of(e)).collect();
                    *out_found = 1;
                    write_ptr(out_elements, Box::new(ScahElementList { store, ids }))?;
                    Ok(())
                }
            }
        })
    }
}

/// Free an element handle. Null is a no-op.
///
/// # Safety
///
/// A non-null `element` must have been returned by scah-ffi, must not already
/// have been freed, and must not be used again after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_element_free(element: *mut ScahElement) {
    ffi_guard_void(|| {
        if element.is_null() {
            return;
        }
        unsafe {
            drop(Box::from_raw(element));
        }
    });
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

        let mut element: *mut ScahElement = std::ptr::null_mut();
        assert_eq!(
            unsafe { scah_element_list_get(list, 0, &mut element, &mut err) },
            ScahStatus::Ok
        );

        // Free list and store while element remains alive.
        unsafe {
            scah_element_list_free(list);
            scah_store_free(store);
        }

        let mut name = ScahStringView::empty();
        assert_eq!(
            unsafe { scah_element_name(element, &mut name, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(
            unsafe { std::str::from_utf8(std::slice::from_raw_parts(name.data, name.len)) }
                .unwrap(),
            "a"
        );

        let mut href = ScahOptionalStringView::none();
        assert_eq!(
            unsafe { scah_element_get_attribute(element, view("href"), &mut href, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(href.is_some, 1);

        let mut text = ScahOptionalStringView::none();
        assert_eq!(
            unsafe { scah_element_text_content(element, &mut text, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(text.is_some, 1);

        unsafe {
            scah_element_free(element);
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
            scah_element_free(std::ptr::null_mut());
            scah_element_list_free(std::ptr::null_mut());
        }
    }

    #[test]
    fn list_oob_and_optional_empty() {
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
        let mut element: *mut ScahElement = std::ptr::null_mut();
        assert_eq!(
            unsafe { scah_element_list_get(list, 5, &mut element, &mut err) },
            ScahStatus::IndexOutOfBounds
        );

        unsafe {
            scah_element_list_get(list, 0, &mut element, &mut err);
        }
        let mut text = ScahOptionalStringView::none();
        unsafe {
            scah_element_text_content(element, &mut text, &mut err);
        }
        // Empty text may be Some("") or None depending on capture; missing id is None.
        let mut id = ScahOptionalStringView {
            value: ScahStringView::borrow("sentinel"),
            is_some: 1,
        };
        unsafe {
            scah_element_id(element, &mut id, &mut err);
        }
        assert_eq!(id.is_some, 0);

        unsafe {
            scah_element_free(element);
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
            let mut div: *mut ScahElement = std::ptr::null_mut();
            scah_element_list_get(list, 0, &mut div, &mut err);
            scah_element_list_free(list);

            let mut children: *mut ScahElementList = std::ptr::null_mut();
            scah_element_get(div, view("a"), &mut children, &mut found, &mut err);
            assert_eq!(found, 1);
            let mut len = 0usize;
            scah_element_list_len(children, &mut len, &mut err);
            assert_eq!(len, 1);

            scah_element_list_free(children);
            scah_element_free(div);
            scah_store_free(store);
        }
    }

    #[test]
    fn abi_version_is_one() {
        assert_eq!(scah_abi_version(), 1);
    }
}
