use crate::ffi_util::{map_status, string_view};
use crate::save::PySave;
use pyo3::prelude::*;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use scah_ffi::{
    ScahError, ScahQuery, ScahQueryBuilder, ScahQuerySectionId, scah_query_all,
    scah_query_builder_all, scah_query_builder_append, scah_query_builder_build,
    scah_query_builder_current_section, scah_query_builder_first, scah_query_builder_free,
    scah_query_first, scah_query_free,
};
use std::ptr::NonNull;

#[gen_stub_pyclass]
#[pyclass]
pub struct PyQueryBuilder {
    pub(crate) handle: NonNull<ScahQueryBuilder>,
}

impl Drop for PyQueryBuilder {
    fn drop(&mut self) {
        // SAFETY: handle was returned by scah-ffi and is freed exactly once.
        unsafe {
            scah_query_builder_free(self.handle.as_ptr());
        }
    }
}

unsafe impl Send for PyQueryBuilder {}
unsafe impl Sync for PyQueryBuilder {}

#[gen_stub_pymethods]
#[pymethods]
impl PyQueryBuilder {
    fn all(
        slf: PyRefMut<'_, Self>,
        selector: String,
        save: PySave,
    ) -> PyResult<PyRefMut<'_, Self>> {
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        // SAFETY: handle is live; selector bytes borrow the local String.
        let status = unsafe {
            scah_query_builder_all(
                slf.handle.as_ptr(),
                string_view(&selector),
                save.save,
                &mut out_error,
            )
        };
        map_status(status, out_error)?;
        Ok(slf)
    }

    fn first(
        slf: PyRefMut<'_, Self>,
        selector: String,
        save: PySave,
    ) -> PyResult<PyRefMut<'_, Self>> {
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status = unsafe {
            scah_query_builder_first(
                slf.handle.as_ptr(),
                string_view(&selector),
                save.save,
                &mut out_error,
            )
        };
        map_status(status, out_error)?;
        Ok(slf)
    }

    fn then<'a>(
        slf: PyRefMut<'a, Self>,
        callback: Bound<'a, PyAny>,
    ) -> PyResult<PyRefMut<'a, Self>> {
        let mut parent: ScahQuerySectionId = 0;
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status = unsafe {
            scah_query_builder_current_section(slf.handle.as_ptr(), &mut parent, &mut out_error)
        };
        map_status(status, out_error)?;

        let factory = PyQueryFactory {};
        let result = callback.call1((factory,))?;
        let builders: Vec<PyRef<PyQueryBuilder>> = result.extract()?;

        for child in &builders {
            let mut out_error: *mut ScahError = std::ptr::null_mut();
            let status = unsafe {
                scah_query_builder_append(
                    slf.handle.as_ptr(),
                    parent,
                    child.handle.as_ptr(),
                    &mut out_error,
                )
            };
            map_status(status, out_error)?;
        }

        Ok(slf)
    }

    fn build(&self) -> PyResult<PyQuery> {
        let mut out_query: *mut ScahQuery = std::ptr::null_mut();
        let mut out_error: *mut ScahError = std::ptr::null_mut();
        let status = unsafe {
            scah_query_builder_build(self.handle.as_ptr(), &mut out_query, &mut out_error)
        };
        map_status(status, out_error)?;
        Ok(PyQuery {
            handle: NonNull::new(out_query).expect("Ok status with null query"),
        })
    }
}

#[gen_stub_pyclass]
#[pyclass]
#[derive(Clone)]
pub struct PyQueryFactory {}

#[gen_stub_pymethods]
#[pymethods]
impl PyQueryFactory {
    fn all(&self, selector: String, save: PySave) -> PyResult<PyQueryBuilder> {
        new_root_builder(true, selector, save)
    }

    fn first(&self, selector: String, save: PySave) -> PyResult<PyQueryBuilder> {
        new_root_builder(false, selector, save)
    }
}

#[gen_stub_pyclass]
#[pyclass]
pub struct PyQuery {
    pub(crate) handle: NonNull<ScahQuery>,
}

impl Drop for PyQuery {
    fn drop(&mut self) {
        unsafe {
            scah_query_free(self.handle.as_ptr());
        }
    }
}

unsafe impl Send for PyQuery {}
unsafe impl Sync for PyQuery {}

#[gen_stub_pymethods]
#[pymethods]
impl PyQuery {
    fn __repr__(&self) -> String {
        format!("PyQuery({:?})", self.handle.as_ptr())
    }
}

#[gen_stub_pyclass]
#[pyclass(name = "Query")]
pub struct PyQueryStatic;

#[gen_stub_pymethods]
#[pymethods]
impl PyQueryStatic {
    #[staticmethod]
    pub fn all(selector: String, save: PySave) -> PyResult<PyQueryBuilder> {
        new_root_builder(true, selector, save)
    }

    #[staticmethod]
    pub fn first(selector: String, save: PySave) -> PyResult<PyQueryBuilder> {
        new_root_builder(false, selector, save)
    }
}

fn new_root_builder(all: bool, selector: String, save: PySave) -> PyResult<PyQueryBuilder> {
    let mut out_builder: *mut ScahQueryBuilder = std::ptr::null_mut();
    let mut out_error: *mut ScahError = std::ptr::null_mut();
    let status = if all {
        unsafe {
            scah_query_all(
                string_view(&selector),
                save.save,
                &mut out_builder,
                &mut out_error,
            )
        }
    } else {
        unsafe {
            scah_query_first(
                string_view(&selector),
                save.save,
                &mut out_builder,
                &mut out_error,
            )
        }
    };
    map_status(status, out_error)?;
    Ok(PyQueryBuilder {
        handle: NonNull::new(out_builder).expect("Ok status with null builder"),
    })
}
