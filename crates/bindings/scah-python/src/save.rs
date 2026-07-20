use pyo3::prelude::*;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use scah_core::Save;

#[gen_stub_pyclass]
#[pyclass(module = "scah", name = "Save")]
#[derive(Clone, Copy, Debug)]
pub struct PySave {
    pub save: Save,
}

#[gen_stub_pymethods]
#[pymethods]
impl PySave {
    #[staticmethod]
    pub fn only_inner_html() -> Self {
        Self {
            save: Save::only_inner_html(),
        }
    }

    #[staticmethod]
    pub fn only_raw_text() -> Self {
        Self {
            save: Save::only_raw_text(),
        }
    }

    #[staticmethod]
    pub fn only_text() -> Self {
        Self {
            save: Save::only_text(),
        }
    }

    #[staticmethod]
    pub fn all() -> Self {
        Self { save: Save::all() }
    }

    #[staticmethod]
    pub fn none() -> Self {
        Self { save: Save::none() }
    }

    #[new]
    #[pyo3(signature = (inner_html=false, raw_text=false, text=false))]
    pub fn new(inner_html: bool, raw_text: bool, text: bool) -> Self {
        Self {
            save: Save {
                inner_html,
                raw_text,
                text,
            },
        }
    }
}
