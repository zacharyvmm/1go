use napi::Result;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use scah_ffi::{ScahError, ScahQuery, ScahStore, scah_parse};

mod elements;
mod ffi;
mod query;
mod store;

use ffi::{map_status, string_view};
use query::JsQuery;
use store::JSStore;

#[napi]
#[allow(dead_code)]
fn parse(html: String, queries: Vec<Reference<JsQuery>>) -> Result<JSStore> {
    if queries.is_empty() {
        return Err(Error::new(
            Status::ArrayExpected,
            "parse requires at least one query".to_owned(),
        ));
    }

    let query_ptrs: Vec<*const ScahQuery> = queries
        .iter()
        .map(|q| q.handle.as_ptr() as *const ScahQuery)
        .collect();

    let mut out_store: *mut ScahStore = std::ptr::null_mut();
    let mut out_error: *mut ScahError = std::ptr::null_mut();
    // SAFETY: query handles remain live; html bytes borrow the local String.
    let status = unsafe {
        scah_parse(
            string_view(&html),
            query_ptrs.as_ptr(),
            query_ptrs.len(),
            &mut out_store,
            &mut out_error,
        )
    };
    map_status(status, out_error)?;
    JSStore::from_handle(out_store)
}
