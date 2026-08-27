use std::ptr::NonNull;
use std::sync::Arc;

use napi::Result;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use scah_ffi::{ScahStore, scah_store_len};

use crate::elements::JsElement;
use crate::ffi::{StoreOwner, map_status, take_store_get};

#[napi(js_name = "Store")]
pub struct JSStore {
    pub(crate) owner: Arc<StoreOwner>,
    pub(crate) len: usize,
}

impl JSStore {
    pub(crate) fn from_handle(handle: *mut ScahStore) -> Result<Self> {
        let handle = NonNull::new(handle)
            .ok_or_else(|| Error::from_reason("scah_parse returned null store".to_owned()))?;
        let mut len = 0usize;
        let mut error = std::ptr::null_mut();
        let status = unsafe { scah_store_len(handle.as_ptr(), &mut len, &mut error) };
        map_status(status, error)?;
        Ok(Self {
            owner: Arc::new(StoreOwner::new(handle)),
            len,
        })
    }
}

#[napi]
impl JSStore {
    #[napi]
    pub fn get(&self, query: String) -> Result<Option<Vec<JsElement>>> {
        take_store_get(&self.owner, &query, JsElement::new)
    }

    #[napi(getter)]
    pub fn length(&self) -> i64 {
        self.len as i64
    }
}
