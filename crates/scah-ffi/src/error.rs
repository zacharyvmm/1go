//! Status codes and owned error handles for the C ABI.

use crate::string::ScahStringView;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Result status for every fallible C ABI function.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScahStatus {
    Ok = 0,
    NullPointer = 1,
    InvalidUtf8 = 2,
    InvalidSelector = 3,
    EmptyQueries = 4,
    MaximumDepthExceeded = 5,
    InvalidSection = 6,
    IndexOutOfBounds = 7,
    InternalPanic = 8,
    /// Caller-provided buffer capacity is insufficient.
    BufferTooSmall = 9,
}

/// Owned diagnostic message allocated by the FFI layer.
pub struct ScahError {
    message: String,
}

impl ScahError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

/// Set `*out_error` to null when the pointer is provided.
///
/// # Safety
///
/// When `out_error` is non-null, it must be valid for writing one
/// `*mut ScahError`.
#[inline]
pub(crate) unsafe fn clear_out_error(out_error: *mut *mut ScahError) {
    if !out_error.is_null() {
        unsafe {
            *out_error = std::ptr::null_mut();
        }
    }
}

/// Allocate an error into `out_error` when the slot is non-null.
///
/// # Safety
///
/// When `out_error` is non-null, it must be valid for writing one
/// `*mut ScahError`. Any previous non-null value in the slot is overwritten
/// without freeing it; callers must free prior errors themselves.
#[inline]
pub(crate) unsafe fn set_error(out_error: *mut *mut ScahError, message: impl Into<String>) {
    if out_error.is_null() {
        return;
    }
    let boxed = Box::new(ScahError::new(message));
    unsafe {
        *out_error = Box::into_raw(boxed);
    }
}

/// Run `f` behind `catch_unwind`, mapping panics to [`ScahStatus::InternalPanic`].
///
/// # Safety
///
/// When `out_error` is non-null, it must be valid for writing one
/// `*mut ScahError` for the duration of this call.
pub(crate) unsafe fn ffi_guard<F>(out_error: *mut *mut ScahError, f: F) -> ScahStatus
where
    F: FnOnce() -> Result<(), ScahStatus> + std::panic::UnwindSafe,
{
    unsafe {
        clear_out_error(out_error);
    }
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => ScahStatus::Ok,
        Ok(Err(status)) => status,
        Err(_) => {
            unsafe {
                set_error(out_error, "internal panic in scah-ffi");
            }
            ScahStatus::InternalPanic
        }
    }
}

/// Panic-containing guard for hot ABI entry points.
///
/// Always uses `catch_unwind` in every build profile so a Rust panic cannot
/// escape an `extern "C"` boundary. Prefer a separately audited panic-free
/// leaf implementation (no closure) only when absolute nanosecond cost has
/// been measured and documented.
///
/// # Safety
///
/// Same as [`ffi_guard`].
#[inline(always)]
pub(crate) unsafe fn ffi_guard_leaf<F>(out_error: *mut *mut ScahError, f: F) -> ScahStatus
where
    F: FnOnce() -> Result<(), ScahStatus> + std::panic::UnwindSafe,
{
    // Identical containment model to [`ffi_guard`]; kept as a named entry so
    // call sites remain readable for hot-path getters.
    unsafe { ffi_guard(out_error, f) }
}

/// Null-safe free / simple side-effect wrappers that must not unwind.
pub(crate) fn ffi_guard_void<F>(f: F)
where
    F: FnOnce() + std::panic::UnwindSafe,
{
    let _ = catch_unwind(AssertUnwindSafe(f));
}

/// Value-returning constructors that must not unwind.
pub(crate) fn ffi_guard_value<T, F>(default: T, f: F) -> T
where
    F: FnOnce() -> T + std::panic::UnwindSafe,
{
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(default)
}

/// Borrow the error message. Valid while `error` remains alive.
///
/// # Safety
///
/// `error` must either be null or point to a live [`ScahError`] returned by
/// scah-ffi that has not yet been freed. The returned view borrows the error's
/// message and is only valid until the error is freed or mutated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_error_message(error: *const ScahError) -> ScahStringView {
    ffi_guard_value(ScahStringView::empty(), || {
        if error.is_null() {
            return ScahStringView::empty();
        }
        // SAFETY: caller guarantees `error` is a live ScahError.
        let error = unsafe { &*error };
        ScahStringView::borrow(error.message())
    })
}

/// Free an error handle. Null is a no-op.
///
/// # Safety
///
/// A non-null `error` must have been returned by scah-ffi, must not already
/// have been freed, and must not be used again after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_error_free(error: *mut ScahError) {
    ffi_guard_void(|| {
        if error.is_null() {
            return;
        }
        // SAFETY: caller guarantees ownership of a live ScahError.
        unsafe {
            drop(Box::from_raw(error));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_null_is_ok() {
        unsafe {
            scah_error_free(std::ptr::null_mut());
        }
    }

    #[test]
    fn set_and_read_error() {
        let mut err: *mut ScahError = std::ptr::null_mut();
        unsafe {
            set_error(&mut err, "hello");
        }
        assert!(!err.is_null());
        let view = unsafe { scah_error_message(err) };
        let s = unsafe { std::str::from_utf8(std::slice::from_raw_parts(view.data, view.len)) }
            .unwrap();
        assert_eq!(s, "hello");
        unsafe {
            scah_error_free(err);
        }
    }

    #[test]
    fn null_out_error_discards_message() {
        unsafe {
            set_error(std::ptr::null_mut(), "ignored");
        }
    }

    #[test]
    fn ffi_guard_maps_panic() {
        let mut err: *mut ScahError = std::ptr::null_mut();
        let status = unsafe {
            ffi_guard(&mut err, || -> Result<(), ScahStatus> {
                panic!("boom");
            })
        };
        assert_eq!(status, ScahStatus::InternalPanic);
        assert!(!err.is_null());
        let view = unsafe { scah_error_message(err) };
        assert!(!view.data.is_null() && view.len > 0);
        unsafe {
            scah_error_free(err);
        }
    }

    #[test]
    fn ffi_guard_leaf_maps_panic_in_all_profiles() {
        let mut err: *mut ScahError = std::ptr::null_mut();
        let status = unsafe {
            ffi_guard_leaf(&mut err, || -> Result<(), ScahStatus> {
                panic!("leaf boom");
            })
        };
        assert_eq!(status, ScahStatus::InternalPanic);
        assert!(!err.is_null());
        let view = unsafe { scah_error_message(err) };
        assert!(view.len > 0);
        unsafe {
            scah_error_free(err);
        }
    }
}
