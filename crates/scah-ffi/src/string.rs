//! C-compatible string views and save options.

use crate::error::{ScahStatus, ffi_guard_value};
use scah::Save;
use std::slice;

/// A non-owning UTF-8 string view (`pointer` + `length`).
///
/// Never requires a NUL terminator. A null `data` pointer is only valid when
/// `len == 0`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ScahStringView {
    pub data: *const u8,
    pub len: usize,
}

/// Optional string view. Distinguish missing values from empty strings with
/// `is_some` rather than overloading null pointers.
///
/// - `is_some == 0`: value is absent (`None`)
/// - `is_some != 0` with `value.len == 0`: explicitly empty string (`Some("")`)
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ScahOptionalStringView {
    pub value: ScahStringView,
    pub is_some: u8,
}

/// Which content to capture for matched elements.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScahSave {
    pub inner_html: u8,
    pub text_content: u8,
}

impl ScahStringView {
    /// Empty view (`null`, length 0).
    #[inline]
    pub const fn empty() -> Self {
        Self {
            data: std::ptr::null(),
            len: 0,
        }
    }

    /// Borrow a view into an existing UTF-8 string.
    ///
    /// The returned view is valid only while `s` remains alive and unmodified.
    #[inline]
    pub fn borrow(s: &str) -> Self {
        Self {
            data: s.as_ptr(),
            len: s.len(),
        }
    }

    /// Interpret this view as UTF-8.
    ///
    /// # Safety
    ///
    /// When `len > 0`, `data` must:
    ///
    /// - be non-null;
    /// - be valid for reads of `len` bytes;
    /// - point to initialized memory;
    /// - remain alive and unmodified for the returned reference's lifetime.
    ///
    /// A null `data` with `len == 0` is accepted as an empty string.
    pub(crate) unsafe fn as_str<'a>(self) -> Result<&'a str, ScahStatus> {
        if self.data.is_null() {
            if self.len == 0 {
                return Ok("");
            }
            return Err(ScahStatus::NullPointer);
        }

        // SAFETY: guaranteed by the caller of this unsafe helper.
        let bytes = unsafe { slice::from_raw_parts(self.data, self.len) };
        std::str::from_utf8(bytes).map_err(|_| ScahStatus::InvalidUtf8)
    }
}

impl ScahOptionalStringView {
    #[inline]
    pub const fn none() -> Self {
        Self {
            value: ScahStringView::empty(),
            is_some: 0,
        }
    }

    #[inline]
    pub fn some(s: &str) -> Self {
        Self {
            value: ScahStringView::borrow(s),
            is_some: 1,
        }
    }

    #[inline]
    pub fn from_option(value: Option<&str>) -> Self {
        match value {
            Some(s) => Self::some(s),
            None => Self::none(),
        }
    }
}

impl ScahSave {
    #[inline]
    pub fn to_save(self) -> Save {
        Save {
            inner_html: self.inner_html != 0,
            text_content: self.text_content != 0,
        }
    }

    #[inline]
    pub fn from_save(save: Save) -> Self {
        Self {
            inner_html: u8::from(save.inner_html),
            text_content: u8::from(save.text_content),
        }
    }
}

/// Capture neither inner HTML nor text content.
#[unsafe(no_mangle)]
pub extern "C" fn scah_save_none() -> ScahSave {
    ffi_guard_value(ScahSave::from_save(Save::none()), || {
        ScahSave::from_save(Save::none())
    })
}

/// Capture both inner HTML and text content.
#[unsafe(no_mangle)]
pub extern "C" fn scah_save_all() -> ScahSave {
    ffi_guard_value(ScahSave::from_save(Save::all()), || {
        ScahSave::from_save(Save::all())
    })
}

/// Capture only inner HTML.
#[unsafe(no_mangle)]
pub extern "C" fn scah_save_only_inner_html() -> ScahSave {
    ffi_guard_value(ScahSave::from_save(Save::only_inner_html()), || {
        ScahSave::from_save(Save::only_inner_html())
    })
}

/// Capture only text content.
#[unsafe(no_mangle)]
pub extern "C" fn scah_save_only_text_content() -> ScahSave {
    ffi_guard_value(ScahSave::from_save(Save::only_text_content()), || {
        ScahSave::from_save(Save::only_text_content())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_null_view_is_ok() {
        assert_eq!(unsafe { ScahStringView::empty().as_str() }.unwrap(), "");
    }

    #[test]
    fn null_with_len_is_error() {
        let view = ScahStringView {
            data: std::ptr::null(),
            len: 3,
        };
        assert_eq!(
            unsafe { view.as_str() }.unwrap_err(),
            ScahStatus::NullPointer
        );
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        let bytes = [0xff, 0xfe];
        let view = ScahStringView {
            data: bytes.as_ptr(),
            len: bytes.len(),
        };
        assert_eq!(
            unsafe { view.as_str() }.unwrap_err(),
            ScahStatus::InvalidUtf8
        );
    }

    #[test]
    fn optional_distinguishes_empty_and_none() {
        let empty = ScahOptionalStringView::some("");
        assert_eq!(empty.is_some, 1);
        assert_eq!(empty.value.len, 0);

        let none = ScahOptionalStringView::none();
        assert_eq!(none.is_some, 0);
    }

    #[test]
    fn save_roundtrip() {
        let save = scah_save_all();
        assert_eq!(save.to_save(), Save::all());
        assert_eq!(scah_save_none().to_save(), Save::none());
        assert_eq!(
            scah_save_only_inner_html().to_save(),
            Save::only_inner_html()
        );
        assert_eq!(
            scah_save_only_text_content().to_save(),
            Save::only_text_content()
        );
    }
}
