use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use scah_ffi::{
    ScahError, ScahOptionalStringView, ScahStatus, ScahStringView, scah_error_free,
    scah_error_message,
};

#[inline]
pub fn string_view(s: &str) -> ScahStringView {
    ScahStringView {
        data: s.as_ptr(),
        len: s.len(),
    }
}

#[inline]
pub fn view_to_string(view: ScahStringView) -> String {
    if view.data.is_null() || view.len == 0 {
        return String::new();
    }
    unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(view.data, view.len)).into_owned() }
}

/// Distinguish missing attribute values (null data) from empty strings.
#[inline]
pub fn view_to_option_string(view: ScahStringView) -> Option<String> {
    if view.data.is_null() {
        None
    } else {
        Some(view_to_string(view))
    }
}

#[inline]
pub fn optional_to_option(opt: ScahOptionalStringView) -> Option<String> {
    if opt.is_some == 0 {
        None
    } else {
        Some(view_to_string(opt.value))
    }
}

pub fn take_error_message(err: *mut ScahError) -> String {
    if err.is_null() {
        return String::new();
    }
    let msg = view_to_string(scah_error_message(err));
    scah_error_free(err);
    msg
}

pub fn map_status(status: ScahStatus, err: *mut ScahError) -> PyResult<()> {
    if status == ScahStatus::Ok {
        // Clear any leftover error slot just in case.
        if !err.is_null() {
            scah_error_free(err);
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

    match status {
        ScahStatus::InvalidSelector
        | ScahStatus::EmptyQueries
        | ScahStatus::MaximumDepthExceeded
        | ScahStatus::InvalidSection
        | ScahStatus::IndexOutOfBounds => Err(PyValueError::new_err(message)),
        _ => Err(PyRuntimeError::new_err(message)),
    }
}
