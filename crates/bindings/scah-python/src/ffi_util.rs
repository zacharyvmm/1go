use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use scah_ffi::{ScahError, ScahStatus, ScahStringView, scah_error_free, scah_error_message};

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
    let fallback = match status {
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
        ScahStatus::Ok => unreachable!(),
    };
    let message = if msg.is_empty() {
        fallback.to_string()
    } else {
        msg
    };

    Err(status_err_message(status, message))
}

pub fn status_err(status: ScahStatus) -> PyErr {
    status_err_message(status, status_fallback(status).to_string())
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
