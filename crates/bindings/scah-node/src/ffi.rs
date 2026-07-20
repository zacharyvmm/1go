//! Helpers for calling the scah-ffi C ABI from napi bindings.

use napi::bindgen_prelude::*;
use scah_ffi::{ScahError, ScahStatus, ScahStringView, scah_error_free, scah_error_message};

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
