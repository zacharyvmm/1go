use pyo3::prelude::*;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use scah_ffi::{
    ScahSave, scah_save_all, scah_save_none, scah_save_only_inner_html, scah_save_only_text_content,
};

#[gen_stub_pyclass]
#[pyclass(module = "scah", name = "Save")]
#[derive(Clone, Copy, Debug)]
pub struct PySave {
    pub save: ScahSave,
}

#[gen_stub_pymethods]
#[pymethods]
impl PySave {
    #[staticmethod]
    pub fn only_inner_html() -> Self {
        Self {
            save: scah_save_only_inner_html(),
        }
    }

    #[staticmethod]
    pub fn only_text_content() -> Self {
        Self {
            save: scah_save_only_text_content(),
        }
    }

    #[staticmethod]
    pub fn all() -> Self {
        Self {
            save: scah_save_all(),
        }
    }

    #[staticmethod]
    pub fn none() -> Self {
        Self {
            save: scah_save_none(),
        }
    }

    #[new]
    #[pyo3(signature = (inner_html=false, text_content=false))]
    pub fn new(inner_html: bool, text_content: bool) -> Self {
        Self {
            save: ScahSave {
                inner_html: u8::from(inner_html),
                text_content: u8::from(text_content),
            },
        }
    }
}
