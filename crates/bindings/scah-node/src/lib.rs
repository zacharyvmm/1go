use napi::Result;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use scah_ffi::{BindingStore, ParseError, ScahQuery};

mod elements;
mod ffi;
mod query;
mod store;

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

    // SAFETY: query handles remain live for the duration of parse.
    let store = unsafe { BindingStore::parse(&html, &query_ptrs) }.map_err(|err| match err {
        ParseError::EmptyQueries => Error::new(
            Status::ArrayExpected,
            "parse requires at least one query".to_owned(),
        ),
        ParseError::MaximumDepthExceeded => Error::new(
            Status::GenericFailure,
            "HTML nesting depth exceeds the maximum supported depth".to_owned(),
        ),
    })?;

    Ok(JSStore { store })
}
