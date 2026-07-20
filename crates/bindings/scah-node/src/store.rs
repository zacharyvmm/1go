use std::ptr::{NonNull, null_mut};

use napi::Result;
use napi_derive::napi;
use scah_ffi::{
    ScahElementList, ScahStatus, ScahStore, scah_store_free, scah_store_get, scah_store_len,
};

use crate::elements::{JsElement, take_element_list};
use crate::ffi::{status_to_error, string_view};

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

#[napi]
impl JSStore {
    #[napi]
    pub fn get(&self, query: String) -> Result<Option<Vec<JsElement>>> {
        let mut list: *mut ScahElementList = null_mut();
        let mut found = 0u8;
        let mut err = null_mut();
        let status = unsafe {
            scah_store_get(
                self.handle.as_ptr(),
                string_view(&query),
                &mut list,
                &mut found,
                &mut err,
            )
        };
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }
        if found == 0 {
            return Ok(None);
        }
        Ok(Some(take_element_list(list)?))
    }

    #[napi(getter)]
    pub fn length(&self) -> Result<i64> {
        let mut len = 0usize;
        let mut err = null_mut();
        let status = unsafe { scah_store_len(self.handle.as_ptr(), &mut len, &mut err) };
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }
        Ok(len as i64)
    }
}
