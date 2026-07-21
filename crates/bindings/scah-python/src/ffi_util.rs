use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
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

#[inline]
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

pub fn take_error_message(err: *mut ScahError) -> String {
    if err.is_null() {
        return String::new();
    }
    // SAFETY: `err` is a live scah-ffi error handle owned by the caller.
    let view = unsafe { scah_error_message(err) };
    let msg = unsafe { view_as_str(view) }.to_owned();
    unsafe {
        scah_error_free(err);
    }
    msg
}

pub fn map_status(status: ScahStatus, err: *mut ScahError) -> PyResult<()> {
    if status == ScahStatus::Ok {
        if !err.is_null() {
            unsafe {
                scah_error_free(err);
            }
        }
        return Ok(());
    }

    let msg = take_error_message(err);
    let fallback = status_fallback(status);
    let message = if msg.is_empty() {
        fallback.to_string()
    } else {
        msg
    };

    Err(status_err_message(status, message))
}

fn status_fallback(status: ScahStatus) -> &'static str {
    match status {
        ScahStatus::InvalidSelector => "invalid selector",
        ScahStatus::EmptyQueries => "parse requires at least one query",
        ScahStatus::MaximumDepthExceeded => {
            "HTML nesting depth exceeds the maximum supported depth"
        }
        ScahStatus::InvalidSection => "invalid query section",
        ScahStatus::IndexOutOfBounds => "index out of bounds",
        ScahStatus::BufferTooSmall => "buffer too small",
        ScahStatus::NullPointer => "null pointer",
        ScahStatus::InvalidUtf8 => "invalid UTF-8",
        ScahStatus::InternalPanic => "internal panic in scah-ffi",
        ScahStatus::Ok => "ok",
    }
}

fn status_err_message(status: ScahStatus, message: String) -> PyErr {
    match status {
        ScahStatus::InvalidSelector
        | ScahStatus::EmptyQueries
        | ScahStatus::MaximumDepthExceeded
        | ScahStatus::InvalidSection
        | ScahStatus::IndexOutOfBounds
        | ScahStatus::BufferTooSmall => PyValueError::new_err(message),
        _ => PyRuntimeError::new_err(message),
    }
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
        // SAFETY: handle was returned by scah-ffi and is freed exactly once.
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

/// One-pass store lookup: small stack buffer, exact heap retry on overflow.
///
/// Does not allocate from total store size. Tag names are not prefetched;
/// callers use `scah_element_name` on demand.
pub fn take_store_get<T>(
    store: *const scah_ffi::ScahStore,
    query: &str,
    mut make: impl FnMut(Arc<ElementListOwner>, ScahElementId) -> T,
) -> PyResult<Option<Vec<T>>> {
    use scah_ffi::scah_store_get_ids_fill;

    // Avoid zeroing 8 KiB on every lookup — ABI writes the initialized prefix.
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
        // Trust `written` only as required capacity; ignore any partial prefix.
        if !error.is_null() {
            unsafe {
                scah_error_free(error);
            }
        }
        let capacity = written;
        let mut ids: Vec<ScahElementId> = Vec::with_capacity(capacity);
        if list.is_null() {
            // Fallback: exact retry of the query fill (no list was returned).
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
            // Prefer fill_ids on the returned span list — avoids re-running the query.
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
            return Err(PyRuntimeError::new_err(
                "scah_store_get_ids_fill wrote beyond caller capacity",
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
        return Err(PyRuntimeError::new_err(
            "scah_store_get_ids_fill wrote beyond caller capacity",
        ));
    }
    if found == 0 {
        free_list_if_any(list);
        return Ok(None);
    }
    let handle = NonNull::new(list)
        .ok_or_else(|| PyRuntimeError::new_err("successful lookup returned null element list"))?;
    let owner = Arc::new(ElementListOwner::new(handle));
    Ok(Some(
        ids_stack[..written]
            .iter()
            // SAFETY: ABI initialized the first `written` slots.
            .map(|slot| make(owner.clone(), unsafe { slot.assume_init() }))
            .collect(),
    ))
}

fn finish_store_get<T>(
    list: *mut ScahElementList,
    found: u8,
    ids: Vec<ScahElementId>,
    make: &mut impl FnMut(Arc<ElementListOwner>, ScahElementId) -> T,
) -> PyResult<Option<Vec<T>>> {
    if found == 0 {
        free_list_if_any(list);
        return Ok(None);
    }
    let handle = match NonNull::new(list) {
        Some(h) => h,
        None => {
            return Err(PyRuntimeError::new_err(
                "successful lookup returned null element list",
            ));
        }
    };
    let owner = Arc::new(ElementListOwner::new(handle));
    Ok(Some(
        ids.into_iter().map(|id| make(owner.clone(), id)).collect(),
    ))
}

/// Nested element lookup with stack buffer + exact heap retry.
///
/// Child elements share `parent_owner` (same store lifetime).
pub fn take_element_get<T>(
    parent_owner: &Arc<ElementListOwner>,
    element: ScahElementId,
    query: &str,
    mut make: impl FnMut(Arc<ElementListOwner>, ScahElementId) -> T,
) -> PyResult<Option<Vec<T>>> {
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
            return Err(PyRuntimeError::new_err(
                "scah_element_get_ids_fill wrote beyond caller capacity",
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
        return Err(PyRuntimeError::new_err(
            "scah_element_get_ids_fill wrote beyond caller capacity",
        ));
    }
    Ok(Some(
        stack[..written]
            .iter()
            .map(|slot| make(parent_owner.clone(), unsafe { slot.assume_init() }))
            .collect(),
    ))
}

/// Fetch all attributes into a borrowed FFI buffer.
///
/// Returned views remain valid while `owner` remains alive.
pub fn fetch_attributes(
    owner: *const ScahElementList,
    id: ScahElementId,
) -> PyResult<Vec<ScahAttributeView>> {
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
            // SAFETY: fill initialized the first `written` entries.
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
    // SAFETY: MaybeUninit elements need no initialization before the ABI write.
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
        // SAFETY: fill initialized the first `written` entries.
        .map(|slot| unsafe { slot.assume_init() })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulates an ABI fill that writes only a prefix of reserved capacity.
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
