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

/// Shared owner for one C ABI result list (one allocation, one ID vector).
pub struct ElementListOwner {
    handle: NonNull<ScahElementList>,
}

impl ElementListOwner {
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

pub fn take_store_get<T>(
    store: *const scah_ffi::ScahStore,
    query: &str,
    capacity_hint: Option<usize>,
    mut make: impl FnMut(Arc<ElementListOwner>, ScahElementId) -> T,
) -> Result<Option<Vec<T>>> {
    use scah_ffi::{scah_error_free, scah_store_get_ids_fill, scah_store_len};

    let hint = if let Some(hint) = capacity_hint {
        hint
    } else {
        let mut hint = 0usize;
        let mut error = std::ptr::null_mut();
        let status = unsafe { scah_store_len(store, &mut hint, &mut error) };
        map_status(status, error)?;
        hint
    };

    let mut ids = Vec::with_capacity(hint);
    // SAFETY: get_ids_fill writes every slot up to `written` (<= hint on success).
    unsafe {
        ids.set_len(hint);
    }
    let mut written = 0usize;
    let mut list: *mut ScahElementList = std::ptr::null_mut();
    let mut found = 0u8;
    let mut error = std::ptr::null_mut();
    let status = unsafe {
        scah_store_get_ids_fill(
            store,
            string_view(query),
            ids.as_mut_ptr(),
            std::ptr::null_mut(),
            ids.len(),
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
        ids = Vec::with_capacity(written);
        unsafe {
            ids.set_len(written);
        }
        error = std::ptr::null_mut();
        let status = unsafe {
            scah_store_get_ids_fill(
                store,
                string_view(query),
                ids.as_mut_ptr(),
                std::ptr::null_mut(),
                ids.len(),
                &mut written,
                &mut list,
                &mut found,
                &mut error,
            )
        };
        map_status(status, error)?;
    } else {
        map_status(status, error)?;
    }

    if found == 0 {
        return Ok(None);
    }

    let handle = NonNull::new(list).ok_or_else(|| {
        Error::from_reason("successful lookup returned null element list".to_owned())
    })?;
    let owner = Arc::new(ElementListOwner { handle });
    ids.truncate(written);
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

    let mut stack = [0usize; 4];
    let mut written = 0usize;
    let mut found = 0u8;
    let status = unsafe {
        scah_element_get_ids_fill(
            parent_owner.as_ptr(),
            element,
            string_view(query),
            stack.as_mut_ptr(),
            stack.len(),
            &mut written,
            std::ptr::null_mut(),
            &mut found,
            std::ptr::null_mut(),
        )
    };
    if status == ScahStatus::BufferTooSmall {
        let mut ids = Vec::with_capacity(written);
        unsafe {
            ids.set_len(written);
        }
        let status = unsafe {
            scah_element_get_ids_fill(
                parent_owner.as_ptr(),
                element,
                string_view(query),
                ids.as_mut_ptr(),
                ids.len(),
                &mut written,
                std::ptr::null_mut(),
                &mut found,
                std::ptr::null_mut(),
            )
        };
        map_status(status, std::ptr::null_mut())?;
        if found == 0 {
            return Ok(None);
        }
        ids.truncate(written);
        return Ok(Some(
            ids.into_iter()
                .map(|id| make(parent_owner.clone(), id))
                .collect(),
        ));
    }
    map_status(status, std::ptr::null_mut())?;
    if found == 0 {
        return Ok(None);
    }
    Ok(Some(
        stack[..written]
            .iter()
            .copied()
            .map(|id| make(parent_owner.clone(), id))
            .collect(),
    ))
}

pub fn fetch_attributes(
    owner: *const ScahElementList,
    id: ScahElementId,
) -> Result<Vec<ScahAttributeView>> {
    let mut stack: [MaybeUninit<ScahAttributeView>; 8] =
        [const { MaybeUninit::uninit() }; 8];
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
