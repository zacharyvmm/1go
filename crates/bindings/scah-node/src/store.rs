use std::ptr::NonNull;

use napi::Result;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use scah_ffi::{ScahStore, scah_store_free, scah_store_len};

use crate::elements::JsElement;
use crate::ffi::{map_status, take_store_get};

#[napi(js_name = "Store")]
pub struct JSStore {
    pub(crate) handle: NonNull<ScahStore>,
}

impl Drop for JSStore {
    fn drop(&mut self) {
        unsafe {
            scah_store_free(self.handle.as_ptr());
        }
    }
}

impl JSStore {
    pub(crate) fn from_handle(handle: *mut ScahStore) -> Result<Self> {
        NonNull::new(handle)
            .map(|handle| Self { handle })
            .ok_or_else(|| Error::from_reason("scah_parse returned null store".to_owned()))
    }
}

#[napi]
impl JSStore {
    #[napi]
    pub fn get(&self, query: String) -> Result<Option<Vec<JsElement>>> {
        take_store_get(self.handle.as_ptr(), &query, |owner, id| JsElement {
            owner,
            id,
        })
    }

    #[napi(getter)]
    pub fn length(&self) -> Result<i64> {
        let mut len = 0usize;
        let mut error = std::ptr::null_mut();
        let status = unsafe { scah_store_len(self.handle.as_ptr(), &mut len, &mut error) };
        map_status(status, error)?;
        Ok(len as i64)
    }
}
