use crate::ffi_util::status_err;
use pyo3::exceptions::PyValueError;
use pyo3::types::PyDict;
use pyo3::{Bound, IntoPyObjectExt, prelude::*};
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use scah_ffi::{BindingStore, ScahElementId};

#[gen_stub_pyclass]
#[pyclass(module = "scah", name = "Element")]
pub struct PyElement {
    store: BindingStore,
    id: ScahElementId,
}

unsafe impl Send for PyElement {}
unsafe impl Sync for PyElement {}

#[gen_stub_pymethods]
#[pymethods]
impl PyElement {
    #[getter]
    pub fn name(&self) -> PyResult<&str> {
        self.store.name(self.id).map_err(status_err)
    }

    #[getter]
    pub fn class_name(&self) -> PyResult<Option<&str>> {
        self.store.class_name(self.id).map_err(status_err)
    }

    #[getter]
    pub fn id(&self) -> PyResult<Option<&str>> {
        self.store.id_attr(self.id).map_err(status_err)
    }

    pub fn get_attribute(&self, key: String) -> PyResult<Option<&str>> {
        self.store.get_attribute(self.id, &key).map_err(status_err)
    }

    #[getter]
    pub fn attributes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let object = PyDict::new(py);
        self.store
            .for_each_attribute(self.id, |key, value| {
                let _ = object.set_item(key, value);
            })
            .map_err(status_err)?;
        Ok(object)
    }

    #[getter]
    pub fn inner_html(&self) -> PyResult<Option<&str>> {
        self.store.inner_html(self.id).map_err(status_err)
    }

    #[getter]
    pub fn text_content(&self) -> PyResult<Option<&str>> {
        self.store.text_content(self.id).map_err(status_err)
    }

    pub fn get(&self, query: String) -> PyResult<Vec<PyElement>> {
        match self
            .store
            .child_get_with(self.id, &query, |store, id| PyElement { store, id })
            .map_err(status_err)?
        {
            None => Err(PyValueError::new_err(format!(
                "This Element does not have children selected with `{query}`"
            ))),
            Some(children) => Ok(children),
        }
    }

    pub fn keys(&self) -> Vec<&'static str> {
        vec![
            "name",
            "id",
            "class",
            "attributes",
            "inner_html",
            "text_content",
        ]
    }

    pub fn __getitem__<'py>(&'py self, py: Python<'py>, key: &str) -> PyResult<Bound<'py, PyAny>> {
        match key {
            "name" => self.name()?.into_bound_py_any(py),
            "id" => self.id()?.into_bound_py_any(py),
            "class" => self.class_name()?.into_bound_py_any(py),
            "attributes" => self.attributes(py)?.into_bound_py_any(py),
            "inner_html" => self.inner_html()?.into_bound_py_any(py),
            "text_content" => self.text_content()?.into_bound_py_any(py),
            _ => Err(pyo3::exceptions::PyKeyError::new_err(key.to_string())),
        }
    }
}

#[gen_stub_pyclass]
#[pyclass(module = "scah", name = "Store")]
pub(crate) struct PyStore {
    pub(crate) store: BindingStore,
}

unsafe impl Send for PyStore {}
unsafe impl Sync for PyStore {}

#[gen_stub_pymethods]
#[pymethods]
impl PyStore {
    fn get(&self, query: String) -> Option<Vec<PyElement>> {
        self.store
            .get_with(&query, |store, id| PyElement { store, id })
    }

    fn __len__(&self) -> usize {
        self.store.len()
    }
}
