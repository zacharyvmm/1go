use std::ptr::{NonNull, null_mut};

use napi::Result;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use scah_ffi::{ScahQuery, ScahStatus, ScahStore, scah_parse};

mod elements;
mod ffi;
mod query;
mod store;

use ffi::{free_error, status_to_error, string_view};
use query::JsQuery;
use store::JSStore;

#[napi]
#[allow(dead_code)]
fn parse(html: String, queries: Vec<Reference<JsQuery>>) -> Result<JSStore> {
    let query_ptrs: Vec<*const ScahQuery> = queries
        .iter()
        .map(|q| q.handle.as_ptr() as *const ScahQuery)
        .collect();

    let mut out: *mut ScahStore = null_mut();
    let mut err = null_mut();
    let status = scah_parse(
        string_view(&html),
        query_ptrs.as_ptr(),
        query_ptrs.len(),
        &mut out,
        &mut err,
    );

    match status {
        ScahStatus::Ok => {
            let handle = NonNull::new(out)
                .ok_or_else(|| Error::from_reason("scah_parse returned null store".to_owned()))?;
            Ok(JSStore { handle })
        }
        ScahStatus::EmptyQueries => {
            free_error(err);
            Err(Error::new(
                Status::ArrayExpected,
                "parse requires at least one query".to_owned(),
            ))
        }
        ScahStatus::MaximumDepthExceeded => {
            free_error(err);
            Err(Error::new(
                Status::GenericFailure,
                "HTML nesting depth exceeds the maximum supported depth".to_owned(),
            ))
        }
        other => Err(status_to_error(other, err)),
    }
}
