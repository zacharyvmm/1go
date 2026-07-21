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

/// Send/Sync stand-in for a store-owned [`ScahStringView`].
#[derive(Clone, Copy)]
pub struct CachedStr {
    data: usize,
    len: usize,
}

impl CachedStr {
    /// Sentinel: not yet fetched from the C ABI.
    pub const UNCACHED: Self = Self {
        data: 0,
        len: usize::MAX,
    };

    #[inline]
    pub fn from_view(view: ScahStringView) -> Self {
        Self {
            data: view.data as usize,
            len: view.len,
        }
    }

    #[inline]
    pub fn is_uncached(self) -> bool {
        self.len == usize::MAX
    }

    /// # Safety
    ///
    /// `self` must borrow live store-owned UTF-8 for `'a`, and must not be
    /// [`Self::UNCACHED`].
    #[inline]
    pub unsafe fn as_str<'a>(self) -> &'a str {
        debug_assert!(!self.is_uncached());
        let view = ScahStringView {
            data: self.data as *const u8,
            len: self.len,
        };
        unsafe { view_as_str(view) }
    }
}

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

/// Shared owner for one C ABI result list (one allocation, one ID vector).
pub struct ElementListOwner {
    handle: NonNull<ScahElementList>,
    /// Prefetched tag names parallel to the result order from `store.get`.
    /// Empty when this owner was created without name materialization (nested).
    names: Box<[CachedStr]>,
}

impl ElementListOwner {
    #[inline]
    pub fn new(handle: NonNull<ScahElementList>, names: Box<[CachedStr]>) -> Self {
        Self { handle, names }
    }

    #[inline]
    pub fn as_ptr(&self) -> *const ScahElementList {
        self.handle.as_ptr()
    }

    #[inline]
    pub fn name_at(&self, index: usize) -> Option<CachedStr> {
        self.names.get(index).copied().filter(|n| !n.is_uncached())
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

/// One-pass store lookup: fill IDs into a binding buffer and retain a span list owner.
///
/// `capacity_hint` avoids a `scah_store_len` round-trip when the caller already
/// knows the store size (typical after parse). Prefetches tag names in one C
/// call so subsequent `.name` access (including lookup consume loops) hits the
/// per-element cache.
pub fn take_store_get<T>(
    store: *const scah_ffi::ScahStore,
    query: &str,
    capacity_hint: Option<usize>,
    mut make: impl FnMut(Arc<ElementListOwner>, ScahElementId, u32) -> T,
) -> PyResult<Option<Vec<T>>> {
    use scah_ffi::{scah_store_get_ids_fill, scah_store_len};

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
    let mut names: Vec<ScahStringView> = Vec::with_capacity(hint);
    // SAFETY: get_ids_fill writes every slot up to `written` (<= hint on success).
    unsafe {
        ids.set_len(hint);
        names.set_len(hint);
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
            names.as_mut_ptr(),
            ids.len(),
            &mut written,
            &mut list,
            &mut found,
            &mut error,
        )
    };
    if status == ScahStatus::BufferTooSmall {
        // Exact retry with the reported count.
        if !error.is_null() {
            unsafe {
                scah_error_free(error);
            }
        }
        ids = Vec::with_capacity(written);
        names = Vec::with_capacity(written);
        unsafe {
            ids.set_len(written);
            names.set_len(written);
        }
        error = std::ptr::null_mut();
        let status = unsafe {
            scah_store_get_ids_fill(
                store,
                string_view(query),
                ids.as_mut_ptr(),
                names.as_mut_ptr(),
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

    let handle = NonNull::new(list)
        .ok_or_else(|| PyRuntimeError::new_err("successful lookup returned null element list"))?;
    ids.truncate(written);
    names.truncate(written);

    let owner = Arc::new(ElementListOwner::new(
        handle,
        names
            .into_iter()
            .map(CachedStr::from_view)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    ));
    Ok(Some(
        ids.into_iter()
            .enumerate()
            .map(|(index, id)| make(owner.clone(), id, index as u32))
            .collect(),
    ))
}

/// Nested element lookup with one-pass ID fill.
///
/// Child elements share `parent_owner` (same store lifetime) instead of
/// allocating a new C list per nested query.
pub fn take_element_get<T>(
    parent_owner: &Arc<ElementListOwner>,
    element: ScahElementId,
    query: &str,
    mut make: impl FnMut(Arc<ElementListOwner>, ScahElementId, u32) -> T,
) -> PyResult<Option<Vec<T>>> {
    use scah_ffi::scah_element_get_ids_fill;

    const NO_NAME_IDX: u32 = u32::MAX;

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
                .map(|id| make(parent_owner.clone(), id, NO_NAME_IDX))
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
            .map(|id| make(parent_owner.clone(), id, NO_NAME_IDX))
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
    // Most elements have few attributes; prefer a stack buffer to avoid a
    // separate attribute_count round-trip and a heap allocation.
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
