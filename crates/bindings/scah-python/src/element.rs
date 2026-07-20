use crate::ffi_util::{map_status, optional_to_option, string_view, view_to_string};
use pyo3::exceptions::PyValueError;
use pyo3::types::PyDict;
use pyo3::{Bound, IntoPyObjectExt, prelude::*};
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use scah_ffi::{
    ScahElement, ScahElementList, ScahError, ScahOptionalStringView, ScahStatus, ScahStore,
    ScahStringView, scah_element_attribute_at, scah_element_attribute_count,
    scah_element_class_name, scah_element_free, scah_element_get, scah_element_get_attribute,
    scah_element_id, scah_element_inner_html, scah_element_list_free, scah_element_list_get,
    scah_element_list_len, scah_element_name, scah_element_text_content, scah_error_free,
    scah_store_free, scah_store_get, scah_store_len,
};
use std::ptr::NonNull;

#[gen_stub_pyclass]
#[pyclass(module = "scah", name = "Element")]
pub struct PyElement {
    pub(crate) handle: NonNull<ScahElement>,
}

impl Drop for PyElement {
    fn drop(&mut self) {
        unsafe {
            scah_element_free(self.handle.as_ptr());
        }
    }
}

unsafe impl Send for PyElement {}
unsafe impl Sync for PyElement {}

#[gen_stub_pymethods]
#[pymethods]
impl PyElement {
    #[getter]
    pub fn name(&self) -> Option<String> {
        let mut out = ScahStringView {
            data: std::ptr::null(),
            len: 0,
        };
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status = unsafe { scah_element_name(self.handle.as_ptr(), &mut out, &mut out_error) };
        unsafe {
            scah_error_free(out_error);
        }
        if status != ScahStatus::Ok {
            return None;
        }
        Some(view_to_string(out))
    }

    #[getter]
    pub fn class_name(&self) -> Option<String> {
        let mut out = ScahOptionalStringView {
            value: ScahStringView {
                data: std::ptr::null(),
                len: 0,
            },
            is_some: 0,
        };
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status =
            unsafe { scah_element_class_name(self.handle.as_ptr(), &mut out, &mut out_error) };
        unsafe {
            scah_error_free(out_error);
        }
        if status != ScahStatus::Ok {
            return None;
        }
        optional_to_option(out)
    }

    #[getter]
    pub fn id(&self) -> Option<String> {
        let mut out = ScahOptionalStringView {
            value: ScahStringView {
                data: std::ptr::null(),
                len: 0,
            },
            is_some: 0,
        };
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status = unsafe { scah_element_id(self.handle.as_ptr(), &mut out, &mut out_error) };
        unsafe {
            scah_error_free(out_error);
        }
        if status != ScahStatus::Ok {
            return None;
        }
        optional_to_option(out)
    }

    pub fn get_attribute(&self, key: String) -> Option<String> {
        let mut out = ScahOptionalStringView {
            value: ScahStringView {
                data: std::ptr::null(),
                len: 0,
            },
            is_some: 0,
        };
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status = unsafe {
            scah_element_get_attribute(
                self.handle.as_ptr(),
                string_view(&key),
                &mut out,
                &mut out_error,
            )
        };
        unsafe {
            scah_error_free(out_error);
        }
        if status != ScahStatus::Ok {
            return None;
        }
        optional_to_option(out)
    }

    #[getter]
    pub fn attributes<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyDict>> {
        let object = PyDict::new(py);
        let mut count = 0usize;
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status = unsafe {
            scah_element_attribute_count(self.handle.as_ptr(), &mut count, &mut out_error)
        };
        map_status(status, out_error)?;

        for i in 0..count {
            let mut key = ScahStringView {
                data: std::ptr::null(),
                len: 0,
            };
            let mut value = ScahOptionalStringView::none();
            let mut out_error: *mut ScahError = std::ptr::null_mut();
            let status = unsafe {
                scah_element_attribute_at(
                    self.handle.as_ptr(),
                    i,
                    &mut key,
                    &mut value,
                    &mut out_error,
                )
            };
            map_status(status, out_error)?;
            object.set_item(view_to_string(key), optional_to_option(value))?;
        }
        Ok(object)
    }

    #[getter]
    pub fn inner_html(&self) -> Option<String> {
        let mut out = ScahOptionalStringView {
            value: ScahStringView {
                data: std::ptr::null(),
                len: 0,
            },
            is_some: 0,
        };
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status =
            unsafe { scah_element_inner_html(self.handle.as_ptr(), &mut out, &mut out_error) };
        unsafe {
            scah_error_free(out_error);
        }
        if status != ScahStatus::Ok {
            return None;
        }
        optional_to_option(out)
    }

    #[getter]
    pub fn text_content(&self) -> Option<String> {
        let mut out = ScahOptionalStringView {
            value: ScahStringView {
                data: std::ptr::null(),
                len: 0,
            },
            is_some: 0,
        };
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status =
            unsafe { scah_element_text_content(self.handle.as_ptr(), &mut out, &mut out_error) };
        unsafe {
            scah_error_free(out_error);
        }
        if status != ScahStatus::Ok {
            return None;
        }
        optional_to_option(out)
    }

    pub fn get(&self, query: String) -> PyResult<Vec<PyElement>> {
        let mut out_elements: *mut ScahElementList = std::ptr::null_mut();
        let mut out_found = 0u8;
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status = unsafe {
            scah_element_get(
                self.handle.as_ptr(),
                string_view(&query),
                &mut out_elements,
                &mut out_found,
                &mut out_error,
            )
        };
        map_status(status, out_error)?;
        if out_found == 0 {
            return Err(PyValueError::new_err(format!(
                "This Element does not have children selected with `{query}`"
            )));
        }
        element_list_to_vec(out_elements)
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

    pub fn __getitem__<'a>(&'a self, py: Python<'a>, key: &str) -> PyResult<Bound<'a, PyAny>> {
        match key {
            "name" => self.name().into_bound_py_any(py),
            "id" => self.id().into_bound_py_any(py),
            "class" => self.class_name().into_bound_py_any(py),
            "attributes" => self.attributes(py).and_then(|a| a.into_bound_py_any(py)),
            "inner_html" => self.inner_html().into_bound_py_any(py),
            "text_content" => self.text_content().into_bound_py_any(py),
            _ => Err(pyo3::exceptions::PyKeyError::new_err(key.to_string())),
        }
    }
}

#[gen_stub_pyclass]
#[pyclass(module = "scah", name = "Store")]
pub(crate) struct PyStore {
    pub(crate) handle: NonNull<ScahStore>,
}

impl Drop for PyStore {
    fn drop(&mut self) {
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
    fn get(&self, query: String) -> PyResult<Option<Vec<PyElement>>> {
        let mut out_elements: *mut ScahElementList = std::ptr::null_mut();
        let mut out_found = 0u8;
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status = unsafe {
            scah_store_get(
                self.handle.as_ptr(),
                string_view(&query),
                &mut out_elements,
                &mut out_found,
                &mut out_error,
            )
        };
        map_status(status, out_error)?;
        if out_found == 0 {
            return Ok(None);
        }
        Ok(Some(element_list_to_vec(out_elements)?))
    }

    fn __len__(&self) -> usize {
        let mut len = 0usize;
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status = unsafe { scah_store_len(self.handle.as_ptr(), &mut len, &mut out_error) };
        unsafe {
            scah_error_free(out_error);
        }
        if status != ScahStatus::Ok {
            return 0;
        }
        len
    }
}

fn element_list_to_vec(list: *mut ScahElementList) -> PyResult<Vec<PyElement>> {
    let mut len = 0usize;
    let mut out_error: *mut ScahError = std::ptr::null_mut();
    let status = unsafe { scah_element_list_len(list, &mut len, &mut out_error) };
    if let Err(e) = map_status(status, out_error) {
        unsafe {
            scah_element_list_free(list);
        }
        return Err(e);
    }

    let mut result = Vec::with_capacity(len);
    for i in 0..len {
        let mut out_element: *mut ScahElement = std::ptr::null_mut();
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status = unsafe { scah_element_list_get(list, i, &mut out_element, &mut out_error) };
        if let Err(e) = map_status(status, out_error) {
            unsafe {
                scah_element_list_free(list);
            }
            return Err(e);
        }
        result.push(PyElement {
            handle: NonNull::new(out_element).expect("Ok status with null element"),
        });
    }
    unsafe {
        scah_element_list_free(list);
    }
    Ok(result)
}
