use napi_derive::napi;
use scah_ffi::BindingStore;

use crate::elements::JsElement;

#[napi(js_name = "Store")]
pub struct JSStore {
    pub(crate) store: BindingStore,
}

#[napi]
impl JSStore {
    #[napi]
    pub fn get(&self, query: String) -> Option<Vec<JsElement>> {
        self.store
            .get_with(&query, |store, id| JsElement { store, id })
    }

    #[napi(getter)]
    pub fn length(&self) -> i64 {
        self.store.len() as i64
    }
}
