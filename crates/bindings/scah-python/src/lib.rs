use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3_stub_gen::{define_stub_info_gatherer, derive::gen_stub_pyfunction};

mod element;
mod ffi_util;
mod query;
mod save;

use crate::element::{PyElement, PyStore};
use crate::query::{PyQuery, PyQueryBuilder, PyQueryFactory, PyQueryStatic};
use crate::save::PySave;
use scah_ffi::{BindingStore, ScahQuery};

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

    // SAFETY: query handles remain live for the duration of parse.
    let store = unsafe { BindingStore::parse(&html, &ptrs) }.map_err(|err| match err {
        scah_ffi::ParseError::EmptyQueries => {
            PyValueError::new_err("parse requires at least one query")
        }
        scah_ffi::ParseError::MaximumDepthExceeded => {
            PyValueError::new_err("HTML nesting depth exceeds the maximum supported depth")
        }
    })?;

    Ok(PyStore { store })
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
