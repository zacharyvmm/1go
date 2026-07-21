//! Helpers for calling the scah-ffi C ABI from napi bindings.

use napi::bindgen_prelude::*;
use scah_ffi::{
    ScahAttributeView, ScahElementId, ScahError, ScahOptionalStringView, ScahStatus, ScahStore,
    ScahStringView, scah_error_free, scah_error_message, scah_store_element_attributes_fill,
    scah_store_free,
};
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::sync::Arc;

pub const INLINE_LOOKUP_CAPACITY: usize = 1024;

pub fn string_view(s: &str) -> ScahStringView {
    ScahStringView {
        data: s.as_ptr(),
        len: s.len(),
    }
}

#[inline]
pub unsafe fn view_as_str<'a>(view: ScahStringView) -> &'a str {
    if view.data.is_null() || view.len == 0 {
        return "";
    }
    let bytes = unsafe { std::slice::from_raw_parts(view.data, view.len) };
    debug_assert!(std::str::from_utf8(bytes).is_ok());
    unsafe { std::str::from_utf8_unchecked(bytes) }
}

#[inline]
pub fn optional_view_to_option<'a>(view: ScahOptionalStringView) -> Option<&'a str> {
    if view.is_some == 0 {
        None
    } else {
        Some(unsafe { view_as_str(view.value) })
    }
}

pub fn status_to_error(status: ScahStatus, err: *mut ScahError) -> Error {
    let message = if err.is_null() {
        format!("scah-ffi error: {status:?}")
    } else {
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

/// Shared owner for the parsed store handle.
pub struct StoreOwner {
    handle: NonNull<ScahStore>,
}

impl StoreOwner {
    #[inline]
    pub fn new(handle: NonNull<ScahStore>) -> Self {
        Self { handle }
    }

    #[inline]
    pub fn as_ptr(&self) -> *const ScahStore {
        self.handle.as_ptr()
    }
}

impl Drop for StoreOwner {
    fn drop(&mut self) {
        unsafe {
            scah_store_free(self.handle.as_ptr());
        }
    }
}

unsafe impl Send for StoreOwner {}
unsafe impl Sync for StoreOwner {}

#[inline]
pub unsafe fn finalize_filled_vec<T>(buf: &mut Vec<T>, written: usize) {
    debug_assert!(written <= buf.capacity());
    unsafe {
        buf.set_len(written);
    }
}

pub fn take_store_get<T>(
    owner: &Arc<StoreOwner>,
    query: &str,
    mut make: impl FnMut(Arc<StoreOwner>, ScahElementId, Option<&'static str>) -> T,
) -> Result<Option<Vec<T>>> {
    use scah_ffi::scah_store_get_ids_fill;

    let mut ids_stack: [MaybeUninit<ScahElementId>; INLINE_LOOKUP_CAPACITY] =
        [const { MaybeUninit::uninit() }; INLINE_LOOKUP_CAPACITY];
    let mut names_stack: [MaybeUninit<ScahStringView>; INLINE_LOOKUP_CAPACITY] =
        [const { MaybeUninit::uninit() }; INLINE_LOOKUP_CAPACITY];
    let mut written = 0usize;
    let mut found = 0u8;
    let mut error = std::ptr::null_mut();
    let status = unsafe {
        scah_store_get_ids_fill(
            owner.as_ptr(),
            string_view(query),
            ids_stack.as_mut_ptr().cast(),
            names_stack.as_mut_ptr().cast(),
            INLINE_LOOKUP_CAPACITY,
            &mut written,
            std::ptr::null_mut(),
            &mut found,
            &mut error,
        )
    };

    if status == ScahStatus::BufferTooSmall {
        debug_assert!(error.is_null());
        let capacity = written;
        let mut ids: Vec<ScahElementId> = Vec::with_capacity(capacity);
        let mut names: Vec<ScahStringView> = Vec::with_capacity(capacity);
        written = 0;
        found = 0;
        error = std::ptr::null_mut();
        let status = unsafe {
            scah_store_get_ids_fill(
                owner.as_ptr(),
                string_view(query),
                ids.as_mut_ptr(),
                names.as_mut_ptr(),
                capacity,
                &mut written,
                std::ptr::null_mut(),
                &mut found,
                &mut error,
            )
        };
        map_status(status, error)?;
        unsafe {
            finalize_filled_vec(&mut ids, written);
            finalize_filled_vec(&mut names, written);
        }
        if found == 0 {
            return Ok(None);
        }
        return Ok(Some(
            ids.into_iter()
                .zip(names)
                .map(|(id, name)| make(owner.clone(), id, Some(unsafe { view_as_str(name) })))
                .collect(),
        ));
    }

    map_status(status, error)?;
    if found == 0 {
        return Ok(None);
    }
    Ok(Some(
        (0..written)
            .map(|i| {
                let id = unsafe { ids_stack[i].assume_init() };
                let name = unsafe { view_as_str(names_stack[i].assume_init()) };
                make(owner.clone(), id, Some(name))
            })
            .collect(),
    ))
}

pub fn take_element_get<T>(
    owner: &Arc<StoreOwner>,
    element: ScahElementId,
    query: &str,
    mut make: impl FnMut(Arc<StoreOwner>, ScahElementId, Option<&'static str>) -> T,
) -> Result<Option<Vec<T>>> {
    use scah_ffi::scah_store_element_get_ids_fill;

    const NESTED_INLINE: usize = 8;
    let mut stack: [MaybeUninit<ScahElementId>; NESTED_INLINE] =
        [const { MaybeUninit::uninit() }; NESTED_INLINE];
    let mut written = 0usize;
    let mut found = 0u8;
    let mut error = std::ptr::null_mut();
    let status = unsafe {
        scah_store_element_get_ids_fill(
            owner.as_ptr(),
            element,
            string_view(query),
            stack.as_mut_ptr().cast(),
            NESTED_INLINE,
            &mut written,
            &mut found,
            &mut error,
        )
    };
    if status == ScahStatus::BufferTooSmall {
        debug_assert!(error.is_null());
        let capacity = written;
        let mut ids: Vec<ScahElementId> = Vec::with_capacity(capacity);
        written = 0;
        found = 0;
        error = std::ptr::null_mut();
        let status = unsafe {
            scah_store_element_get_ids_fill(
                owner.as_ptr(),
                element,
                string_view(query),
                ids.as_mut_ptr(),
                capacity,
                &mut written,
                &mut found,
                &mut error,
            )
        };
        map_status(status, error)?;
        unsafe {
            finalize_filled_vec(&mut ids, written);
        }
        if found == 0 {
            return Ok(None);
        }
        return Ok(Some(
            ids.into_iter()
                .map(|id| make(owner.clone(), id, None))
                .collect(),
        ));
    }
    map_status(status, error)?;
    if found == 0 {
        return Ok(None);
    }
    Ok(Some(
        stack[..written]
            .iter()
            .map(|slot| make(owner.clone(), unsafe { slot.assume_init() }, None))
            .collect(),
    ))
}

pub fn with_attributes<R>(
    store: *const ScahStore,
    id: ScahElementId,
    f: impl FnOnce(&[ScahAttributeView]) -> Result<R>,
) -> Result<R> {
    let mut stack: [MaybeUninit<ScahAttributeView>; 8] = [const { MaybeUninit::uninit() }; 8];
    let mut written = 0usize;
    let mut error = std::ptr::null_mut();
    let status = unsafe {
        scah_store_element_attributes_fill(
            store,
            id,
            stack.as_mut_ptr().cast(),
            stack.len(),
            &mut written,
            &mut error,
        )
    };
    if status == ScahStatus::Ok {
        let attrs = unsafe {
            std::slice::from_raw_parts(stack.as_ptr().cast::<ScahAttributeView>(), written)
        };
        return f(attrs);
    }
    if status != ScahStatus::BufferTooSmall {
        map_status(status, error)?;
        unreachable!();
    }
    debug_assert!(error.is_null());
    let required = written.max(1);
    let mut heap: Vec<MaybeUninit<ScahAttributeView>> = Vec::with_capacity(required);
    unsafe {
        heap.set_len(required);
    }
    written = 0;
    error = std::ptr::null_mut();
    let status = unsafe {
        scah_store_element_attributes_fill(
            store,
            id,
            heap.as_mut_ptr().cast(),
            required,
            &mut written,
            &mut error,
        )
    };
    map_status(status, error)?;
    let attrs =
        unsafe { std::slice::from_raw_parts(heap.as_ptr().cast::<ScahAttributeView>(), written) };
    f(attrs)
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
