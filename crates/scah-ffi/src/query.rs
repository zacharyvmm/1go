//! Opaque query handles and C ABI entry points.

use crate::error::{ScahError, ScahStatus, ffi_guard, ffi_guard_void, set_error};
use crate::owned_query::{OwnedQuery, OwnedQueryBuilder};
use crate::string::{ScahSave, ScahStringView};
use scah::QuerySectionId;
use std::sync::Arc;

/// Opaque pending query builder handle.
pub struct ScahQueryBuilder {
    pub(crate) inner: OwnedQueryBuilder,
}

/// Opaque compiled query handle.
pub struct ScahQuery {
    pub(crate) inner: Arc<OwnedQuery>,
}

/// Section identifier within a query builder tree.
pub type ScahQuerySectionId = usize;

/// # Safety
///
/// `ptr` must be non-null and point to a live `T` for the returned lifetime.
unsafe fn require_mut<'a, T>(ptr: *mut T) -> Result<&'a mut T, ScahStatus> {
    if ptr.is_null() {
        Err(ScahStatus::NullPointer)
    } else {
        // SAFETY: caller guarantees a live, uniquely borrowed `T`.
        Ok(unsafe { &mut *ptr })
    }
}

/// # Safety
///
/// `ptr` must be non-null and point to a live `T` for the returned lifetime.
unsafe fn require_ref<'a, T>(ptr: *const T) -> Result<&'a T, ScahStatus> {
    if ptr.is_null() {
        Err(ScahStatus::NullPointer)
    } else {
        // SAFETY: caller guarantees a live `T`.
        Ok(unsafe { &*ptr })
    }
}

/// # Safety
///
/// When `out` is non-null, it must be valid for writing one `*mut T`.
unsafe fn write_ptr<T>(out: *mut *mut T, value: Box<T>) -> Result<(), ScahStatus> {
    if out.is_null() {
        return Err(ScahStatus::NullPointer);
    }
    unsafe {
        *out = Box::into_raw(value);
    }
    Ok(())
}

/// # Safety
///
/// When `out` is non-null, it must be valid for writing one `*mut T`.
unsafe fn clear_out_ptr<T>(out: *mut *mut T) {
    if !out.is_null() {
        unsafe {
            *out = std::ptr::null_mut();
        }
    }
}

/// # Safety
///
/// `selector` must satisfy [`ScahStringView::as_str`]. When `out_error` is
/// non-null, it must be valid for writing one `*mut ScahError`.
unsafe fn parse_selector(
    selector: ScahStringView,
    out_error: *mut *mut ScahError,
) -> Result<String, ScahStatus> {
    // SAFETY: caller guarantees the string-view contract.
    match unsafe { selector.as_str() } {
        Ok(s) => Ok(s.to_owned()),
        Err(ScahStatus::NullPointer) => {
            unsafe {
                set_error(out_error, "null string pointer with nonzero length");
            }
            Err(ScahStatus::NullPointer)
        }
        Err(ScahStatus::InvalidUtf8) => {
            unsafe {
                set_error(out_error, "string view is not valid UTF-8");
            }
            Err(ScahStatus::InvalidUtf8)
        }
        Err(other) => Err(other),
    }
}

/// Create a root builder that matches all occurrences of `selector`.
///
/// The selector is not validated until [`scah_query_builder_build`].
///
/// # Safety
///
/// `selector` must satisfy [`ScahStringView::as_str`]. `out_builder` must be
/// non-null and valid for writing one `*mut ScahQueryBuilder`. When
/// `out_error` is non-null, it must be valid for writing one `*mut ScahError`.
/// On success the caller owns the returned builder and must free it with
/// [`scah_query_builder_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_query_all(
    selector: ScahStringView,
    save: ScahSave,
    out_builder: *mut *mut ScahQueryBuilder,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_ptr(out_builder);
        ffi_guard(out_error, || {
            let selector = parse_selector(selector, out_error)?;
            let builder = Box::new(ScahQueryBuilder {
                inner: OwnedQueryBuilder::new_all(selector, save.to_save()),
            });
            write_ptr(out_builder, builder)?;
            Ok(())
        })
    }
}

/// Create a root builder that matches the first occurrence of `selector`.
///
/// # Safety
///
/// Same requirements as [`scah_query_all`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_query_first(
    selector: ScahStringView,
    save: ScahSave,
    out_builder: *mut *mut ScahQueryBuilder,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_ptr(out_builder);
        ffi_guard(out_error, || {
            let selector = parse_selector(selector, out_error)?;
            let builder = Box::new(ScahQueryBuilder {
                inner: OwnedQueryBuilder::new_first(selector, save.to_save()),
            });
            write_ptr(out_builder, builder)?;
            Ok(())
        })
    }
}

/// Append a linear `all` child under the builder's current last section.
///
/// # Safety
///
/// `builder` must point to a live [`ScahQueryBuilder`]. `selector` must satisfy
/// [`ScahStringView::as_str`]. When `out_error` is non-null, it must be valid
/// for writing one `*mut ScahError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_query_builder_all(
    builder: *mut ScahQueryBuilder,
    selector: ScahStringView,
    save: ScahSave,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        ffi_guard(out_error, || {
            let builder = require_mut(builder)?;
            let selector = parse_selector(selector, out_error)?;
            builder.inner.all_mut(selector, save.to_save());
            Ok(())
        })
    }
}

/// Append a linear `first` child under the builder's current last section.
///
/// # Safety
///
/// Same requirements as [`scah_query_builder_all`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_query_builder_first(
    builder: *mut ScahQueryBuilder,
    selector: ScahStringView,
    save: ScahSave,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        ffi_guard(out_error, || {
            let builder = require_mut(builder)?;
            let selector = parse_selector(selector, out_error)?;
            builder.inner.first_mut(selector, save.to_save());
            Ok(())
        })
    }
}

/// Return the current (last) section id for subsequent [`scah_query_builder_append`] calls.
///
/// # Safety
///
/// `builder` must either be null (returns [`ScahStatus::NullPointer`]) or point
/// to a live [`ScahQueryBuilder`]. `out_section` must be non-null and valid for
/// writing one [`ScahQuerySectionId`]. When `out_error` is non-null, it must be
/// valid for writing one `*mut ScahError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_query_builder_current_section(
    builder: *const ScahQueryBuilder,
    out_section: *mut ScahQuerySectionId,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        ffi_guard(out_error, || {
            let builder = require_ref(builder)?;
            if out_section.is_null() {
                return Err(ScahStatus::NullPointer);
            }
            match builder.inner.current_section() {
                Some(section) => {
                    *out_section = section.index();
                    Ok(())
                }
                None => {
                    set_error(out_error, "query builder has no sections");
                    Err(ScahStatus::InvalidSection)
                }
            }
        })
    }
}

/// Clone `child` under `parent` without consuming either builder.
///
/// Self-append (`builder == child`) clones the current tree into an owned
/// temporary before taking a mutable borrow of `builder`, so overlapping
/// Rust references are never created.
///
/// # Safety
///
/// `builder` must point to a live mutable [`ScahQueryBuilder`]. `child` must
/// point to a live [`ScahQueryBuilder`]. When `out_error` is non-null, it must
/// be valid for writing one `*mut ScahError`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_query_builder_append(
    builder: *mut ScahQueryBuilder,
    parent: ScahQuerySectionId,
    child: *const ScahQueryBuilder,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        ffi_guard(out_error, || {
            if builder.is_null() || child.is_null() {
                return Err(ScahStatus::NullPointer);
            }

            // Complete the shared access and create an owned clone before
            // constructing a mutable reference to `builder`.
            let child_inner = (*child).inner.clone();

            let builder = &mut *builder;

            match builder.inner.append(QuerySectionId(parent), &child_inner) {
                Ok(()) => Ok(()),
                Err(()) => {
                    set_error(out_error, "invalid parent query section");
                    Err(ScahStatus::InvalidSection)
                }
            }
        })
    }
}

/// Compile the builder into a query. Does not consume the builder.
///
/// # Safety
///
/// `builder` must point to a live [`ScahQueryBuilder`]. `out_query` must be
/// non-null and valid for writing one `*mut ScahQuery`. When `out_error` is
/// non-null, it must be valid for writing one `*mut ScahError`. On success the
/// caller owns the returned query and must free it with [`scah_query_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_query_builder_build(
    builder: *const ScahQueryBuilder,
    out_query: *mut *mut ScahQuery,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_ptr(out_query);
        ffi_guard(out_error, || {
            let builder = require_ref(builder)?;
            match builder.inner.build() {
                Ok(owned) => {
                    let query = Box::new(ScahQuery {
                        inner: Arc::new(owned),
                    });
                    write_ptr(out_query, query)?;
                    Ok(())
                }
                Err(err) => {
                    set_error(out_error, err.to_string());
                    Err(ScahStatus::InvalidSelector)
                }
            }
        })
    }
}

/// Deep-clone a builder handle.
///
/// # Safety
///
/// `builder` must point to a live [`ScahQueryBuilder`]. `out_builder` must be
/// non-null and valid for writing one `*mut ScahQueryBuilder`. When `out_error`
/// is non-null, it must be valid for writing one `*mut ScahError`. On success
/// the caller owns the clone and must free it with [`scah_query_builder_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_query_builder_clone(
    builder: *const ScahQueryBuilder,
    out_builder: *mut *mut ScahQueryBuilder,
    out_error: *mut *mut ScahError,
) -> ScahStatus {
    unsafe {
        clear_out_ptr(out_builder);
        ffi_guard(out_error, || {
            let builder = require_ref(builder)?;
            let cloned = Box::new(ScahQueryBuilder {
                inner: builder.inner.clone(),
            });
            write_ptr(out_builder, cloned)?;
            Ok(())
        })
    }
}

/// Free a builder handle. Null is a no-op.
///
/// # Safety
///
/// A non-null `builder` must have been returned by scah-ffi, must not already
/// have been freed, and must not be used again after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_query_builder_free(builder: *mut ScahQueryBuilder) {
    ffi_guard_void(|| {
        if builder.is_null() {
            return;
        }
        // SAFETY: caller guarantees ownership of a live builder.
        unsafe {
            drop(Box::from_raw(builder));
        }
    });
}

/// Free a query handle. Null is a no-op.
///
/// # Safety
///
/// A non-null `query` must have been returned by scah-ffi, must not already
/// have been freed, and must not be used again after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scah_query_free(query: *mut ScahQuery) {
    ffi_guard_void(|| {
        if query.is_null() {
            return;
        }
        // SAFETY: caller guarantees ownership of a live query.
        unsafe {
            drop(Box::from_raw(query));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::scah_error_free;
    use crate::string::{scah_save_all, scah_save_none};

    fn view(s: &str) -> ScahStringView {
        ScahStringView::borrow(s)
    }

    #[test]
    fn null_builder_returns_null_pointer() {
        let mut err: *mut ScahError = std::ptr::null_mut();
        let status = unsafe {
            scah_query_builder_all(std::ptr::null_mut(), view("a"), scah_save_all(), &mut err)
        };
        assert_eq!(status, ScahStatus::NullPointer);
        unsafe {
            scah_error_free(err);
        }
    }

    #[test]
    fn build_invalid_selector() {
        let mut builder: *mut ScahQueryBuilder = std::ptr::null_mut();
        let mut err: *mut ScahError = std::ptr::null_mut();
        assert_eq!(
            unsafe { scah_query_all(view(""), scah_save_all(), &mut builder, &mut err) },
            ScahStatus::Ok
        );
        let mut query: *mut ScahQuery = std::ptr::null_mut();
        let status = unsafe { scah_query_builder_build(builder, &mut query, &mut err) };
        assert_eq!(status, ScahStatus::InvalidSelector);
        assert!(!err.is_null());
        unsafe {
            scah_error_free(err);
            scah_query_builder_free(builder);
            scah_query_free(query);
        }
    }

    #[test]
    fn append_invalid_section() {
        let mut builder: *mut ScahQueryBuilder = std::ptr::null_mut();
        let mut child: *mut ScahQueryBuilder = std::ptr::null_mut();
        let mut err: *mut ScahError = std::ptr::null_mut();
        unsafe {
            scah_query_all(view("div"), scah_save_all(), &mut builder, &mut err);
            scah_query_all(view("span"), scah_save_none(), &mut child, &mut err);
        }
        let status = unsafe { scah_query_builder_append(builder, 99, child, &mut err) };
        assert_eq!(status, ScahStatus::InvalidSection);
        unsafe {
            scah_error_free(err);
            scah_query_builder_free(builder);
            scah_query_builder_free(child);
        }
    }

    #[test]
    fn free_null_ok() {
        unsafe {
            scah_query_builder_free(std::ptr::null_mut());
            scah_query_free(std::ptr::null_mut());
        }
    }

    #[test]
    fn build_reuse_and_child_survive() {
        let mut root: *mut ScahQueryBuilder = std::ptr::null_mut();
        let mut child: *mut ScahQueryBuilder = std::ptr::null_mut();
        let mut err: *mut ScahError = std::ptr::null_mut();
        unsafe {
            scah_query_all(view("main"), scah_save_all(), &mut root, &mut err);
            scah_query_all(view("a"), scah_save_all(), &mut child, &mut err);
        }
        let mut parent = 0usize;
        unsafe {
            scah_query_builder_current_section(root, &mut parent, &mut err);
            scah_query_builder_append(root, parent, child, &mut err);
        }

        let mut q1: *mut ScahQuery = std::ptr::null_mut();
        let mut q2: *mut ScahQuery = std::ptr::null_mut();
        assert_eq!(
            unsafe { scah_query_builder_build(root, &mut q1, &mut err) },
            ScahStatus::Ok
        );
        assert_eq!(
            unsafe { scah_query_builder_build(root, &mut q2, &mut err) },
            ScahStatus::Ok
        );
        assert!(!q1.is_null() && !q2.is_null());

        // Child still usable.
        let mut q_child: *mut ScahQuery = std::ptr::null_mut();
        assert_eq!(
            unsafe { scah_query_builder_build(child, &mut q_child, &mut err) },
            ScahStatus::Ok
        );

        unsafe {
            scah_query_free(q1);
            scah_query_free(q2);
            scah_query_free(q_child);
            scah_query_builder_free(root);
            scah_query_builder_free(child);
        }
    }

    #[test]
    fn builder_can_append_itself_without_aliasing() {
        let mut root: *mut ScahQueryBuilder = std::ptr::null_mut();
        let mut err: *mut ScahError = std::ptr::null_mut();
        assert_eq!(
            unsafe { scah_query_all(view("div"), scah_save_all(), &mut root, &mut err) },
            ScahStatus::Ok
        );

        let mut parent = 0usize;
        assert_eq!(
            unsafe { scah_query_builder_current_section(root, &mut parent, &mut err) },
            ScahStatus::Ok
        );

        // Self-append must clone before mutably borrowing the same handle.
        assert_eq!(
            unsafe { scah_query_builder_append(root, parent, root, &mut err) },
            ScahStatus::Ok
        );

        let mut query: *mut ScahQuery = std::ptr::null_mut();
        assert_eq!(
            unsafe { scah_query_builder_build(root, &mut query, &mut err) },
            ScahStatus::Ok
        );
        assert!(!query.is_null());

        // Parse nested HTML that exercises the cloned child tree under div.
        let html = "<div><div><span>ok</span></div></div>";
        let mut store: *mut crate::store::ScahStore = std::ptr::null_mut();
        let queries = [query as *const ScahQuery];
        assert_eq!(
            unsafe {
                crate::store::scah_parse(view(html), queries.as_ptr(), 1, &mut store, &mut err)
            },
            ScahStatus::Ok
        );

        let mut list: *mut crate::store::ScahElementList = std::ptr::null_mut();
        let mut found = 0u8;
        assert_eq!(
            unsafe {
                crate::store::scah_store_get(store, view("div"), &mut list, &mut found, &mut err)
            },
            ScahStatus::Ok
        );
        assert_eq!(found, 1);

        let mut len = 0usize;
        unsafe {
            crate::store::scah_element_list_len(list, &mut len, &mut err);
        }
        assert!(len >= 1);

        unsafe {
            crate::store::scah_element_list_free(list);
            crate::store::scah_store_free(store);
            scah_query_free(query);
            scah_query_builder_free(root);
        }
    }
}
