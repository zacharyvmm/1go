//! Helpers for calling the scah-ffi C ABI from napi bindings.

use napi::bindgen_prelude::*;
use scah_ffi::{
    ScahError, ScahOptionalStringView, ScahStatus, ScahStringView, scah_error_free,
    scah_error_message,
};

pub fn string_view(s: &str) -> ScahStringView {
    ScahStringView {
        data: s.as_ptr(),
        len: s.len(),
    }
}

pub fn view_to_string(view: ScahStringView) -> String {
    if view.data.is_null() {
        return String::new();
    }
    // SAFETY: views returned by scah-ffi borrow live handle-owned UTF-8.
    let bytes = unsafe { std::slice::from_raw_parts(view.data, view.len) };
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

pub fn optional_to_option(opt: ScahOptionalStringView) -> Option<String> {
    if opt.is_some == 0 {
        None
    } else {
        Some(view_to_string(opt.value))
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
        let msg = view_to_string(view);
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

/// Free an error handle if present (e.g. after manually mapping a known status).
pub fn free_error(err: *mut ScahError) {
    if !err.is_null() {
        unsafe {
            scah_error_free(err);
        }
    }
}
