use napi_derive::napi;

use super::elements::JsElement;
use scah::Store;

use std::sync::Arc;

#[napi(js_name = "Store")]
pub struct JSStore {
    pub(crate) store: Arc<Store<'static, 'static>>,
    pub(crate) _html: Arc<String>,
    pub(crate) _query_tapes: Vec<Arc<Vec<u8>>>,
}

#[napi]
impl JSStore {
    #[napi]
    pub fn get(&self, query: String) -> Option<Vec<JsElement>> {
        self.store.get(&query).map(|iter| {
            iter.map(|e| unsafe { self.store.elements.index_of(e) })
                .map(|i| JsElement {
                    store: self.store.clone(),
                    id: i,
                })
                .collect()
        })
    }

    #[napi(getter)]
    pub fn length(&self) -> i64 {
        self.store.elements.len() as i64
    }
}
