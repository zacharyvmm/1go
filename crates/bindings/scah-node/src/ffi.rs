//! Helpers for calling the scah-ffi C ABI from napi bindings.

use napi::bindgen_prelude::*;
use scah_ffi::{
    ScahAttributeView, ScahElementId, ScahElementList, ScahError, ScahOptionalStringView,
    ScahStatus, ScahStringView, scah_element_attributes_fill, scah_element_list_free,
    scah_error_free, scah_error_message,
};
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::sync::Arc;

/// Initial stack capacity for selective and mid-size lookups.
///
/// 1024 × usize ≈ 8 KiB — fits the 100 / 1_000 gate cases in one pass.
/// Larger results use BufferTooSmall → `scah_element_list_fill_ids`.
pub const INLINE_LOOKUP_CAPACITY: usize = 1024;

pub fn string_view(s: &str) -> ScahStringView {
    ScahStringView {
        data: s.as_ptr(),
        len: s.len(),
    }
}

/// Borrow a FFI string view as `&str` without allocating.
///
/// # Safety
///
/// `view` must borrow live handle-owned UTF-8 for `'a`.
#[inline]
pub unsafe fn view_as_str<'a>(view: ScahStringView) -> &'a str {
    if view.data.is_null() || view.len == 0 {
        return "";
    }
    // SAFETY: caller guarantees a live UTF-8 borrow for `'a`.
    let bytes = unsafe { std::slice::from_raw_parts(view.data, view.len) };
    debug_assert!(std::str::from_utf8(bytes).is_ok());
    // SAFETY: ABI contract — successful views are valid UTF-8.
    unsafe { std::str::from_utf8_unchecked(bytes) }
}

#[inline]
pub fn optional_view_to_option<'a>(view: ScahOptionalStringView) -> Option<&'a str> {
    if view.is_some == 0 {
        None
    } else {
        // SAFETY: successful optional views borrow store-owned UTF-8.
        Some(unsafe { view_as_str(view.value) })
    }
}

/// Convert an FFI status + optional error handle into a napi error.
///
/// Always frees `err` when non-null.
pub fn status_to_error(status: ScahStatus, err: *mut ScahError) -> Error {
    let message = if err.is_null() {
        format!("scah-ffi error: {status:?}")
    } else {
        // SAFETY: `err` is a live scah-ffi error handle.
        let view = unsafe { scah_error_message(err) };
        let msg = unsafe { view_as_str(view) }.to_owned();
        unsafe {
            scah_error_free(err);
        }
        if msg.is_empty() {
            format!("scah-ffi error: {status:?}")
        } else {
            msg
        }
    };

    let napi_status = match status {
        ScahStatus::EmptyQueries => Status::ArrayExpected,
        _ => Status::GenericFailure,
    };
    Error::new(napi_status, message)
}

pub fn map_status(status: ScahStatus, err: *mut ScahError) -> Result<()> {
    if status == ScahStatus::Ok {
        if !err.is_null() {
            unsafe {
                scah_error_free(err);
            }
        }
        return Ok(());
    }
    Err(status_to_error(status, err))
}

fn free_list_if_any(list: *mut ScahElementList) {
    if !list.is_null() {
        unsafe {
            scah_element_list_free(list);
        }
    }
}

/// Shared owner for one C ABI result list (retains the store via the list).
pub struct ElementListOwner {
    handle: NonNull<ScahElementList>,
}

impl ElementListOwner {
    #[inline]
    pub fn new(handle: NonNull<ScahElementList>) -> Self {
        Self { handle }
    }

    #[inline]
    pub fn as_ptr(&self) -> *const ScahElementList {
        self.handle.as_ptr()
    }
}

impl Drop for ElementListOwner {
    fn drop(&mut self) {
        unsafe {
            scah_element_list_free(self.handle.as_ptr());
        }
    }
}

// SAFETY: exclusive Arc ownership; C ABI element access is read-only.
unsafe impl Send for ElementListOwner {}
unsafe impl Sync for ElementListOwner {}

/// Finalize a Vec after an ABI fill initialized exactly `written` prefix slots.
///
/// # Safety
///
/// The first `written` elements of `buf`'s allocated storage must already be
/// initialized by a successful ABI write, and `written <= buf.capacity()`.
#[inline]
pub unsafe fn finalize_filled_vec<T>(buf: &mut Vec<T>, written: usize) {
    debug_assert!(written <= buf.capacity());
    unsafe {
        buf.set_len(written);
    }
}

pub fn take_store_get<T>(
    store: *const scah_ffi::ScahStore,
    query: &str,
    mut make: impl FnMut(Arc<ElementListOwner>, ScahElementId) -> T,
) -> Result<Option<Vec<T>>> {
    use scah_ffi::scah_store_get_ids_fill;

    let mut ids_stack: [MaybeUninit<ScahElementId>; INLINE_LOOKUP_CAPACITY] =
        [const { MaybeUninit::uninit() }; INLINE_LOOKUP_CAPACITY];
    let mut written = 0usize;
    let mut list: *mut ScahElementList = std::ptr::null_mut();
    let mut found = 0u8;
    let mut error = std::ptr::null_mut();
    let status = unsafe {
        scah_store_get_ids_fill(
            store,
            string_view(query),
            ids_stack.as_mut_ptr() as *mut ScahElementId,
            std::ptr::null_mut(),
            INLINE_LOOKUP_CAPACITY,
            &mut written,
            &mut list,
            &mut found,
            &mut error,
        )
    };

    if status == ScahStatus::BufferTooSmall {
        if !error.is_null() {
            unsafe {
                scah_error_free(error);
            }
        }
        let capacity = written;
        let mut ids: Vec<ScahElementId> = Vec::with_capacity(capacity);
        if list.is_null() {
            written = 0;
            found = 0;
            error = std::ptr::null_mut();
            let status = unsafe {
                scah_store_get_ids_fill(
                    store,
                    string_view(query),
                    ids.as_mut_ptr(),
                    std::ptr::null_mut(),
                    capacity,
                    &mut written,
                    &mut list,
                    &mut found,
                    &mut error,
                )
            };
            if status != ScahStatus::Ok {
                free_list_if_any(list);
                map_status(status, error)?;
                unreachable!();
            }
            map_status(status, error)?;
        } else {
            written = 0;
            error = std::ptr::null_mut();
            let status = unsafe {
                scah_ffi::scah_element_list_fill_ids(
                    list,
                    ids.as_mut_ptr(),
                    capacity,
                    &mut written,
                    &mut error,
                )
            };
            if status != ScahStatus::Ok {
                free_list_if_any(list);
                map_status(status, error)?;
                unreachable!();
            }
            map_status(status, error)?;
            found = 1;
        }
        if written > capacity {
            free_list_if_any(list);
            return Err(Error::from_reason(
                "scah_store_get_ids_fill wrote beyond caller capacity".to_owned(),
            ));
        }
        // SAFETY: successful ABI call initialized exactly the first `written` entries.
        unsafe {
            finalize_filled_vec(&mut ids, written);
        }
        return finish_store_get(list, found, ids, &mut make);
    }

    if status != ScahStatus::Ok {
        free_list_if_any(list);
        map_status(status, error)?;
        unreachable!();
    }
    map_status(status, error)?;
    if written > INLINE_LOOKUP_CAPACITY {
        free_list_if_any(list);
        return Err(Error::from_reason(
            "scah_store_get_ids_fill wrote beyond caller capacity".to_owned(),
        ));
    }
    if found == 0 {
        free_list_if_any(list);
        return Ok(None);
    }
    let handle = NonNull::new(list).ok_or_else(|| {
        Error::from_reason("successful lookup returned null element list".to_owned())
    })?;
    let owner = Arc::new(ElementListOwner::new(handle));
    Ok(Some(
        ids_stack[..written]
            .iter()
            .map(|slot| make(owner.clone(), unsafe { slot.assume_init() }))
            .collect(),
    ))
}

fn finish_store_get<T>(
    list: *mut ScahElementList,
    found: u8,
    ids: Vec<ScahElementId>,
    make: &mut impl FnMut(Arc<ElementListOwner>, ScahElementId) -> T,
) -> Result<Option<Vec<T>>> {
    if found == 0 {
        free_list_if_any(list);
        return Ok(None);
    }
    let handle = NonNull::new(list).ok_or_else(|| {
        Error::from_reason("successful lookup returned null element list".to_owned())
    })?;
    let owner = Arc::new(ElementListOwner::new(handle));
    Ok(Some(
        ids.into_iter().map(|id| make(owner.clone(), id)).collect(),
    ))
}

pub fn take_element_get<T>(
    parent_owner: &Arc<ElementListOwner>,
    element: ScahElementId,
    query: &str,
    mut make: impl FnMut(Arc<ElementListOwner>, ScahElementId) -> T,
) -> Result<Option<Vec<T>>> {
    use scah_ffi::scah_element_get_ids_fill;

    const NESTED_INLINE: usize = 32;
    let mut stack: [MaybeUninit<ScahElementId>; NESTED_INLINE] =
        [const { MaybeUninit::uninit() }; NESTED_INLINE];
    let mut written = 0usize;
    let mut found = 0u8;
    let mut error = std::ptr::null_mut();
    let status = unsafe {
        scah_element_get_ids_fill(
            parent_owner.as_ptr(),
            element,
            string_view(query),
            stack.as_mut_ptr() as *mut ScahElementId,
            stack.len(),
            &mut written,
            std::ptr::null_mut(),
            &mut found,
            &mut error,
        )
    };
    if status == ScahStatus::BufferTooSmall {
        if !error.is_null() {
            unsafe {
                scah_error_free(error);
            }
        }
        let capacity = written;
        let mut ids: Vec<ScahElementId> = Vec::with_capacity(capacity);
        written = 0;
        found = 0;
        error = std::ptr::null_mut();
        let status = unsafe {
            scah_element_get_ids_fill(
                parent_owner.as_ptr(),
                element,
                string_view(query),
                ids.as_mut_ptr(),
                capacity,
                &mut written,
                std::ptr::null_mut(),
                &mut found,
                &mut error,
            )
        };
        map_status(status, error)?;
        if written > capacity {
            return Err(Error::from_reason(
                "scah_element_get_ids_fill wrote beyond caller capacity".to_owned(),
            ));
        }
        // SAFETY: successful ABI retry initialized exactly `written` entries.
        unsafe {
            finalize_filled_vec(&mut ids, written);
        }
        if found == 0 {
            return Ok(None);
        }
        return Ok(Some(
            ids.into_iter()
                .map(|id| make(parent_owner.clone(), id))
                .collect(),
        ));
    }
    map_status(status, error)?;
    if found == 0 {
        return Ok(None);
    }
    if written > NESTED_INLINE {
        return Err(Error::from_reason(
            "scah_element_get_ids_fill wrote beyond caller capacity".to_owned(),
        ));
    }
    Ok(Some(
        stack[..written]
            .iter()
            .map(|slot| make(parent_owner.clone(), unsafe { slot.assume_init() }))
            .collect(),
    ))
}

pub fn fetch_attributes(
    owner: *const ScahElementList,
    id: ScahElementId,
) -> Result<Vec<ScahAttributeView>> {
    let mut stack: [MaybeUninit<ScahAttributeView>; 8] = [const { MaybeUninit::uninit() }; 8];
    let mut written = 0usize;
    let mut error = std::ptr::null_mut();
    let status = unsafe {
        scah_element_attributes_fill(
            owner,
            id,
            stack.as_mut_ptr() as *mut ScahAttributeView,
            stack.len(),
            &mut written,
            &mut error,
        )
    };
    if status != ScahStatus::BufferTooSmall {
        map_status(status, error)?;
        return Ok(stack
            .into_iter()
            .take(written)
            .map(|slot| unsafe { slot.assume_init() })
            .collect());
    }
    if !error.is_null() {
        unsafe {
            scah_error_free(error);
        }
    }
    let capacity = written.max(1);
    let mut buf: Vec<MaybeUninit<ScahAttributeView>> = Vec::with_capacity(capacity);
    unsafe {
        buf.set_len(capacity);
    }
    written = 0;
    error = std::ptr::null_mut();
    let status = unsafe {
        scah_element_attributes_fill(
            owner,
            id,
            buf.as_mut_ptr() as *mut ScahAttributeView,
            capacity,
            &mut written,
            &mut error,
        )
    };
    map_status(status, error)?;
    Ok(buf
        .into_iter()
        .take(written)
        .map(|slot| unsafe { slot.assume_init() })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn mock_partial_fill(dst: *mut usize, capacity: usize, written_out: *mut usize) {
        assert!(capacity >= 2);
        unsafe {
            *dst.add(0) = 10;
            *dst.add(1) = 20;
            *written_out = 2;
        }
    }

    #[test]
    fn finalize_exposes_only_written_prefix() {
        let capacity = 16usize;
        let mut ids: Vec<usize> = Vec::with_capacity(capacity);
        let mut written = 0usize;
        unsafe {
            mock_partial_fill(ids.as_mut_ptr(), capacity, &mut written);
            finalize_filled_vec(&mut ids, written);
        }
        assert_eq!(written, 2);
        assert_eq!(ids.len(), 2);
        assert_eq!(ids, vec![10, 20]);
    }
}
