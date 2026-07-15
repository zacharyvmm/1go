use pyo3::prelude::*;
use pyo3_stub_gen::{define_stub_info_gatherer, derive::gen_stub_pyfunction};

use std::sync::Arc;

mod element;
mod query;
mod save;

use crate::query::{PyQuery, PyQueryBuilder, PyQueryFactory, PyQueryStatic};
use crate::save::PySave;
use element::{PyElement, PyStore};

#[gen_stub_pyfunction]
#[pyfunction]
fn parse(html: String, queries: Vec<PyRef<PyQuery>>) -> PyResult<PyStore> {
    let html = Arc::new(html);
    let html_str: &str = unsafe { std::mem::transmute(html.as_str()) };

    let mut query_tapes: Vec<Arc<Vec<u8>>> = Vec::with_capacity(queries.len());
    let mut queries_rs: Vec<scah_core::Query<'static>> = Vec::with_capacity(queries.len());
    for q in &queries {
        query_tapes.push(q.tape.clone());
        queries_rs.push(q.query.clone());
    }

    let queries_slice =
        unsafe { std::slice::from_raw_parts(queries_rs.as_ptr(), queries_rs.len()) };

    let store = match scah_core::parse(html_str, queries_slice) {
        Ok(store) => store,
        Err(scah_core::ParseError::EmptyQueries) => {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "parse requires at least one query",
            ));
        }
    };

    Ok(PyStore {
        store: Arc::new(store),
        _html: html,
        _query_tapes: query_tapes,
    })
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
