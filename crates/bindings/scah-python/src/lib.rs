use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3_stub_gen::{define_stub_info_gatherer, derive::gen_stub_pyfunction};

mod element;
mod ffi_util;
mod query;
mod save;

use crate::element::{PyElement, PyStore};
use crate::ffi_util::{map_status, string_view};
use crate::query::{PyQuery, PyQueryBuilder, PyQueryFactory, PyQueryStatic};
use crate::save::PySave;
use scah_ffi::{ScahError, ScahQuery, ScahStore, scah_parse};

#[gen_stub_pyfunction]
#[pyfunction]
fn parse(html: String, queries: Vec<PyRef<PyQuery>>) -> PyResult<PyStore> {
    if queries.is_empty() {
        return Err(PyValueError::new_err("parse requires at least one query"));
    }

    let ptrs: Vec<*const ScahQuery> = queries
        .iter()
        .map(|q| q.handle.as_ptr() as *const ScahQuery)
        .collect();

    let mut out_store: *mut ScahStore = std::ptr::null_mut();
    let mut out_error: *mut ScahError = std::ptr::null_mut();
    // SAFETY: query handles remain live; html bytes borrow the local String.
    let status = unsafe {
        scah_parse(
            string_view(&html),
            ptrs.as_ptr(),
            ptrs.len(),
            &mut out_store,
            &mut out_error,
        )
    };
    map_status(status, out_error)?;
    PyStore::from_handle(out_store)
}

#[pymodule]
fn scah(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(parse, m)?)?;
    m.add_class::<PySave>()?;
    m.add_class::<PyQuery>()?;
    m.add_class::<PyQueryBuilder>()?;
    m.add_class::<PyQueryStatic>()?;
    m.add_class::<PyQueryFactory>()?;
    m.add_class::<PyElement>()?;
    m.add_class::<PyStore>()?;
    Ok(())
}

define_stub_info_gatherer!(stub_info);
