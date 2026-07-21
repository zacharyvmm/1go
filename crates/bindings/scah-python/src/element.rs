use crate::ffi_util::{
    ElementListOwner, fetch_attributes, map_status, optional_view_to_option, string_view,
    take_element_get, take_store_get, view_as_str,
};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::types::PyDict;
use pyo3::{Bound, IntoPyObjectExt, prelude::*};
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use scah_ffi::{
    ScahElementId, ScahElementList, ScahOptionalStringView, ScahStatus, ScahStore, ScahStringView,
    scah_element_class_name, scah_element_get_attribute, scah_element_id, scah_element_inner_html,
    scah_element_name, scah_element_text_content, scah_store_free, scah_store_len,
};
use std::ptr::NonNull;
use std::sync::Arc;

#[gen_stub_pyclass]
#[pyclass(module = "scah", name = "Element")]
pub struct PyElement {
    owner: Arc<ElementListOwner>,
    id: ScahElementId,
}

unsafe impl Send for PyElement {}
unsafe impl Sync for PyElement {}

impl PyElement {
    fn list_ptr(&self) -> *const ScahElementList {
        self.owner.as_ptr()
    }

    pub(crate) fn new(owner: Arc<ElementListOwner>, id: ScahElementId) -> Self {
        Self { owner, id }
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyElement {
    #[getter]
    pub fn name(&self) -> PyResult<&str> {
        let mut out = ScahStringView::empty();
        let status =
            unsafe { scah_element_name(self.list_ptr(), self.id, &mut out, std::ptr::null_mut()) };
        if status != ScahStatus::Ok {
            map_status(status, std::ptr::null_mut())?;
        }
        Ok(unsafe { view_as_str(out) })
    }

    #[getter]
    pub fn class_name(&self) -> PyResult<Option<&str>> {
        let mut out = ScahOptionalStringView::none();
        let status = unsafe {
            scah_element_class_name(self.list_ptr(), self.id, &mut out, std::ptr::null_mut())
        };
        if status != ScahStatus::Ok {
            map_status(status, std::ptr::null_mut())?;
        }
        Ok(optional_view_to_option(out))
    }

    #[getter]
    pub fn id(&self) -> PyResult<Option<&str>> {
        let mut out = ScahOptionalStringView::none();
        let status =
            unsafe { scah_element_id(self.list_ptr(), self.id, &mut out, std::ptr::null_mut()) };
        if status != ScahStatus::Ok {
            map_status(status, std::ptr::null_mut())?;
        }
        Ok(optional_view_to_option(out))
    }

    pub fn get_attribute(&self, key: String) -> PyResult<Option<&str>> {
        let mut out = ScahOptionalStringView::none();
        let status = unsafe {
            scah_element_get_attribute(
                self.list_ptr(),
                self.id,
                string_view(&key),
                &mut out,
                std::ptr::null_mut(),
            )
        };
        if status != ScahStatus::Ok {
            map_status(status, std::ptr::null_mut())?;
        }
        Ok(optional_view_to_option(out))
    }

    #[getter]
    pub fn attributes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let attrs = fetch_attributes(self.list_ptr(), self.id)?;
        let dict = PyDict::new(py);
        for attr in attrs {
            let key = unsafe { view_as_str(attr.key) };
            let value = optional_view_to_option(attr.value);
            dict.set_item(key, value)?;
        }
        Ok(dict)
    }

    #[getter]
    pub fn inner_html(&self) -> PyResult<Option<&str>> {
        let mut out = ScahOptionalStringView::none();
        let status = unsafe {
            scah_element_inner_html(self.list_ptr(), self.id, &mut out, std::ptr::null_mut())
        };
        if status != ScahStatus::Ok {
            map_status(status, std::ptr::null_mut())?;
        }
        Ok(optional_view_to_option(out))
    }

    #[getter]
    pub fn text_content(&self) -> PyResult<Option<&str>> {
        let mut out = ScahOptionalStringView::none();
        let status = unsafe {
            scah_element_text_content(self.list_ptr(), self.id, &mut out, std::ptr::null_mut())
        };
        if status != ScahStatus::Ok {
            map_status(status, std::ptr::null_mut())?;
        }
        Ok(optional_view_to_option(out))
    }

    pub fn get(&self, query: &str) -> PyResult<Vec<PyElement>> {
        match take_element_get(&self.owner, self.id, query, PyElement::new)? {
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
    pub(crate) handle: NonNull<ScahStore>,
    /// Cached at construction for `__len__` only — never used as lookup capacity.
    pub(crate) len: usize,
}

impl Drop for PyStore {
    fn drop(&mut self) {
        // SAFETY: handle was returned by scah_parse and is freed exactly once.
        unsafe {
            scah_store_free(self.handle.as_ptr());
        }
    }
}

unsafe impl Send for PyStore {}
unsafe impl Sync for PyStore {}

#[gen_stub_pymethods]
#[pymethods]
impl PyStore {
    fn get(&self, query: &str) -> PyResult<Option<Vec<PyElement>>> {
        take_store_get(self.handle.as_ptr(), query, PyElement::new)
    }

    fn __len__(&self) -> usize {
        self.len
    }
}

impl PyStore {
    pub(crate) fn from_handle(handle: *mut ScahStore) -> PyResult<Self> {
        let handle = NonNull::new(handle)
            .ok_or_else(|| PyRuntimeError::new_err("scah_parse returned null store"))?;
        let mut len = 0usize;
        let mut error = std::ptr::null_mut();
        let status = unsafe { scah_store_len(handle.as_ptr(), &mut len, &mut error) };
        map_status(status, error)?;
        Ok(Self { handle, len })
    }
}
