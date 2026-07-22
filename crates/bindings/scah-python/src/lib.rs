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
    // SAFETY:
    // The returned PyStore stores `_html: Arc<String>` alongside the parsed
    // Store. All string slices inside Store borrow from this String
    // allocation. Extending the &str lifetime to 'static is sound because
    // PyStore owns the Arc<String> for at least as long as the Store is
    // accessible.
    let html = Arc::new(html);
    let html_str: &'static str = unsafe { std::mem::transmute(html.as_str()) };

    let mut query_tapes: Vec<Arc<Vec<u8>>> = Vec::with_capacity(queries.len());
    let mut queries_rs: Vec<scah_core::Query<'static>> = Vec::with_capacity(queries.len());
    for q in &queries {
        query_tapes.push(q.tape.clone());
        queries_rs.push(q.query.clone());
    }

    // SAFETY:
    // The `'a: 'query` bound on scah_core::parse requires the query-slice
    // reference to outlive `'query` (= `'static` here). `queries_rs` is a
    // local Vec, but the actual query data (`QuerySection::source` strings)
    // lives in `_query_tapes` (Arc-owned). The slice itself is only read
    // during parsing; no reference into the Vec's allocation is stored in
    // the returned Store. The raw-parts coercion satisfies the lifetime
    // bound without leaking memory.
    let queries_slice =
        unsafe { std::slice::from_raw_parts(queries_rs.as_ptr(), queries_rs.len()) };
    let store = match scah_core::parse(html_str, queries_slice) {
        Ok(store) => store,
        Err(scah_core::ParseError::EmptyQueries) => {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "parse requires at least one query",
            ));
        }
        Err(scah_core::ParseError::MaximumDepthExceeded) => {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "HTML nesting depth exceeds the maximum supported depth",
            ));
        }
        Err(scah_core::ParseError::TextCaptureRequired) => {
            // `parse` never returns this; only `parse_without_text_capture` does.
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "internal error: unexpected TextCaptureRequired from parse",
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
