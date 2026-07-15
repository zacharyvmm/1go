use crate::save::PySave;
use pyo3::prelude::*;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use scah_core::lazy::{LazyQuery, LazyQueryBuilder};
use scah_core::{Query, QuerySectionId};

#[gen_stub_pyclass]
#[pyclass]
pub struct PyQueryBuilder {
    builder: LazyQueryBuilder<String>,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyQueryBuilder {
    fn all(mut slf: PyRefMut<'_, Self>, selector: String, save: PySave) -> PyRefMut<'_, Self> {
        slf.builder.all_mut(selector, save.save);
        slf
    }
    fn first(mut slf: PyRefMut<'_, Self>, selector: String, save: PySave) -> PyRefMut<'_, Self> {
        slf.builder.first_mut(selector, save.save);
        slf
    }

    fn then<'a>(
        mut slf: PyRefMut<'a, Self>,
        callback: Bound<'a, PyAny>,
    ) -> PyResult<PyRefMut<'a, Self>> {
        let factory = PyQueryFactory {};
        let result = callback.call1((factory,))?;
        let builders: Vec<PyRef<PyQueryBuilder>> = result.extract()?;
        let children = builders.iter().map(|b| b.builder.clone());

        let current_index = QuerySectionId(slf.builder.len() - 1);
        for child in children {
            slf.builder.append(current_index, child);
        }

        Ok(slf)
    }

    fn build(&self) -> PyResult<PyQuery> {
        self.try_build()
    }

    fn try_build(&self) -> PyResult<PyQuery> {
        let (tape, query) = unsafe { self.builder.clone().try_to_query() }
            .map_err(|err| pyo3::exceptions::PyValueError::new_err(err.to_string()))?;
        Ok(PyQuery { tape, query })
    }
}

#[gen_stub_pyclass]
#[pyclass]
#[derive(Clone)]
pub struct PyQueryFactory {}

#[gen_stub_pymethods]
#[pymethods]
impl PyQueryFactory {
    fn all(&self, selector: String, save: PySave) -> PyQueryBuilder {
        PyQueryBuilder {
            builder: LazyQuery::all(selector, save.save),
        }
    }

    fn first(&self, selector: String, save: PySave) -> PyQueryBuilder {
        PyQueryBuilder {
            builder: LazyQuery::first(selector, save.save),
        }
    }
}

#[gen_stub_pyclass]
#[pyclass]
#[derive(Clone)]
pub struct PyQuery {
    pub(super) tape: std::sync::Arc<Vec<u8>>,
    pub(super) query: Query<'static>,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyQuery {
    fn __repr__(&self) -> String {
        format!("PyQuery(tape={:?}, query={:?})", self.tape, self.query)
    }
}

#[gen_stub_pyclass]
#[pyclass(name = "Query")]
pub struct PyQueryStatic;

#[gen_stub_pymethods]
#[pymethods]
impl PyQueryStatic {
    #[staticmethod]
    pub fn all(selector: String, save: PySave) -> PyQueryBuilder {
        PyQueryBuilder {
            builder: LazyQuery::all(selector, save.save),
        }
    }

    #[staticmethod]
    pub fn first(selector: String, save: PySave) -> PyQueryBuilder {
        PyQueryBuilder {
            builder: LazyQuery::first(selector, save.save),
        }
    }
}
