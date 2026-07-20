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
#[inline]
pub(crate) fn clear_out_error(out_error: *mut *mut ScahError) {
    if !out_error.is_null() {
        unsafe {
            *out_error = std::ptr::null_mut();
        }
    }
}

/// Allocate an error into `out_error` when the slot is non-null.
#[inline]
pub(crate) fn set_error(out_error: *mut *mut ScahError, message: impl Into<String>) {
    if out_error.is_null() {
        return;
    }
    let boxed = Box::new(ScahError::new(message));
    unsafe {
        *out_error = Box::into_raw(boxed);
    }
}

/// Run `f` behind `catch_unwind`, mapping panics to [`ScahStatus::InternalPanic`].
pub(crate) fn ffi_guard<F>(out_error: *mut *mut ScahError, f: F) -> ScahStatus
where
    F: FnOnce() -> Result<(), ScahStatus> + std::panic::UnwindSafe,
{
    clear_out_error(out_error);
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => ScahStatus::Ok,
        Ok(Err(status)) => status,
        Err(_) => {
            set_error(out_error, "internal panic in scah-ffi");
            ScahStatus::InternalPanic
        }
    }
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
#[unsafe(no_mangle)]
pub extern "C" fn scah_error_message(error: *const ScahError) -> ScahStringView {
    ffi_guard_value(ScahStringView::empty(), || {
        if error.is_null() {
            return ScahStringView::empty();
        }
        let error = unsafe { &*error };
        ScahStringView::borrow(error.message())
    })
}

/// Free an error handle. Null is a no-op.
#[unsafe(no_mangle)]
pub extern "C" fn scah_error_free(error: *mut ScahError) {
    ffi_guard_void(|| {
        if error.is_null() {
            return;
        }
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
        scah_error_free(std::ptr::null_mut());
    }

    #[test]
    fn set_and_read_error() {
        let mut err: *mut ScahError = std::ptr::null_mut();
        set_error(&mut err, "hello");
        assert!(!err.is_null());
        let view = scah_error_message(err);
        let s = unsafe { std::str::from_utf8(std::slice::from_raw_parts(view.data, view.len)) }
            .unwrap();
        assert_eq!(s, "hello");
        scah_error_free(err);
    }

    #[test]
    fn null_out_error_discards_message() {
        set_error(std::ptr::null_mut(), "ignored");
    }

    #[test]
    fn ffi_guard_maps_panic() {
        let mut err: *mut ScahError = std::ptr::null_mut();
        let status = ffi_guard(&mut err, || -> Result<(), ScahStatus> {
            panic!("boom");
        });
        assert_eq!(status, ScahStatus::InternalPanic);
        assert!(!err.is_null());
        scah_error_free(err);
    }
}
