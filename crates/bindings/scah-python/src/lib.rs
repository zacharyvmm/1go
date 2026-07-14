use pyo3::prelude::*;
use pyo3_stub_gen::{define_stub_info_gatherer, derive::gen_stub_pyfunction};
use scah_core::{Query, QueryMultiplexer, QuerySpec, Reader, XHtmlParser};

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
    if queries.is_empty() {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "parse requires at least one query",
        ));
    }
    let html = Arc::new(html);
    let html_bytes = html.as_ref().as_bytes();
    let html_bytes = unsafe { std::slice::from_raw_parts(html_bytes.as_ptr(), html_bytes.len()) };

    let queries_rs = queries
        .iter()
        .map(|q| q.query.clone())
        .collect::<Box<[Query]>>();

    let slice = unsafe {
        std::slice::from_raw_parts(queries_rs.as_ref().as_ptr(), queries_rs.as_ref().len())
    };
    // Mirror scah::parse() allocation policy: avoid full-document store
    // preallocation when all queries can exit early (Query::first-style).
    let no_extra_allocations = queries_rs.iter().all(|q| q.exit_at_section_end().is_some());
    let selectors = QueryMultiplexer::new(slice);
    let mut parser = if no_extra_allocations {
        XHtmlParser::new(selectors)
    } else {
        XHtmlParser::with_capacity(selectors, html_bytes.len())
    };

    let mut reader = Reader::from_bytes(html_bytes);
    while parser.next(&mut reader) {}

    let store = std::sync::Arc::new(parser.matches());

    Ok(PyStore { store, _html: html })
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
