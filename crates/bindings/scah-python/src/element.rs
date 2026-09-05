use pyo3::exceptions::{PyDeprecationWarning, PyValueError};
use pyo3::ffi::c_str;
use pyo3::types::PyDict;
use pyo3::{Bound, IntoPyObjectExt, prelude::*};
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use scah_core::{Attribute, ElementId, Store};
use std::sync::Arc;

#[gen_stub_pyclass]
#[pyclass(module = "scah", name = "Element")]
pub struct PyElement {
    pub(crate) store: Arc<Store<'static, 'static>>,
    pub(crate) id: ElementId,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyElement {
    #[getter]
    pub fn name(&self) -> Option<&str> {
        self.store.elements.get(self.id.index()).map(|e| e.name)
    }

    #[getter]
    pub fn class_name(&self) -> Option<&str> {
        self.store
            .elements
            .get(self.id.index())
            .and_then(|e| e.class)
    }

    #[getter]
    pub fn id(&self) -> Option<&str> {
        self.store.elements.get(self.id.index()).and_then(|e| e.id)
    }

    pub fn get_attribute(&self, key: String) -> Option<&str> {
        self.store
            .elements
            .get(self.id.index())
            .and_then(|e| e.attribute(&self.store, &key))
    }

    #[getter]
    pub fn attributes<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyDict>> {
        let object = PyDict::new(py);
        let attributes = self
            .store
            .elements
            .get(self.id.index())
            .and_then(|e| e.attributes(&self.store));

        if let Some(attrs) = attributes {
            for Attribute { key, value } in attrs {
                object.set_item(*key, *value)?
            }
        }
        Ok(object)
    }

    #[getter]
    pub fn inner_html(&self) -> Option<&str> {
        self.store
            .elements
            .get(self.id.index())
            .and_then(|e| e.inner_html)
    }

    #[getter]
    pub fn raw_text(&self) -> Option<&str> {
        self.store
            .elements
            .get(self.id.index())
            .and_then(|e| e.raw_text(&self.store))
    }

    #[getter]
    pub fn text(&self) -> Option<&str> {
        self.store
            .elements
            .get(self.id.index())
            .and_then(|e| e.text(&self.store))
    }

    #[getter]
    pub fn text_content<'a>(&'a self, py: Python<'_>) -> PyResult<Option<&'a str>> {
        PyErr::warn(
            py,
            &py.get_type::<PyDeprecationWarning>(),
            c_str!("Element.text_content is deprecated; use Element.text"),
            1,
        )?;
        Ok(self.text())
    }

    pub fn get(&self, query: String) -> PyResult<Vec<PyElement>> {
        let element = self
            .store
            .elements
            .get(self.id.index())
            .expect("The Element ID should be valid");
        let children = element.get(&self.store, &query);
        match children {
            None => Err(PyValueError::new_err(format!(
                "This Element does not have children selected with `{query}`"
            ))),
            Some(children) => Ok(children
                .map(|e| PyElement {
                    store: self.store.clone(),
                    id: unsafe { self.store.elements.index_of(e) },
                })
                .collect()),
        }
    }

    pub fn keys(&self) -> Vec<&'static str> {
        vec![
            "name",
            "id",
            "class",
            "attributes",
            "inner_html",
            "raw_text",
            "text",
            "text_content",
        ]
    }

    pub fn __getitem__<'a>(&'a self, py: Python<'a>, key: &str) -> PyResult<Bound<'a, PyAny>> {
        match key {
            "name" => self.name().into_bound_py_any(py),
            "id" => self.id().into_bound_py_any(py),
            "class" => self.class_name().into_bound_py_any(py),
            "attributes" => self.attributes(py).and_then(|a| a.into_bound_py_any(py)),
            "inner_html" => self.inner_html().into_bound_py_any(py),
            "raw_text" => self.raw_text().into_bound_py_any(py),
            "text" => self.text().into_bound_py_any(py),
            "text_content" => self.text_content(py)?.into_bound_py_any(py),
            _ => Err(pyo3::exceptions::PyKeyError::new_err(key.to_string())),
        }
    }
}

#[gen_stub_pyclass]
#[pyclass(module = "scah", name = "Store")]
pub(crate) struct PyStore {
    pub(crate) store: Arc<Store<'static, 'static>>,
    pub(crate) _html: Arc<String>,
    pub(crate) _query_tapes: Vec<Arc<Vec<u8>>>,
}

#[gen_stub_pymethods]
#[pymethods]
impl PyStore {
    fn get(&self, query: String) -> Option<Vec<PyElement>> {
        self.store.get(&query).map(|iter| {
            iter.map(|e| unsafe { self.store.elements.index_of(e) })
                .map(|i| PyElement {
                    store: self.store.clone(),
                    id: i,
                })
                .collect()
        })
    }

    fn __len__(&self) -> usize {
        self.store.elements.len()
    }
}
