use napi::Result;
use napi::bindgen_prelude::*;
use napi_derive::napi;

use ::scah::ParseError;

use std::sync::Arc;

mod query;
use query::JsQuery;
mod store;
use store::JSStore;

mod elements;

#[napi]
#[allow(dead_code)]
fn parse(html: String, queries: Vec<Reference<JsQuery>>) -> Result<JSStore> {
    // SAFETY:
    // The returned JSStore stores `_html: Arc<String>` alongside the parsed
    // Store. All string slices inside Store borrow from this String
    // allocation. Extending the &str lifetime to 'static is sound because
    // JSStore owns the Arc<String> for at least as long as the Store is
    // accessible.
    let html = Arc::new(html);
    let html_str: &'static str = unsafe { std::mem::transmute(html.as_str()) };

    let mut query_tapes: Vec<Arc<Vec<u8>>> = Vec::with_capacity(queries.len());
    let mut queries_rs: Vec<scah::Query<'static>> = Vec::with_capacity(queries.len());
    for q in &queries {
        query_tapes.push(q._tape.clone());
        queries_rs.push(q.query.clone());
    }

    // SAFETY:
    // The `'a: 'query` bound on scah::parse requires the query-slice
    // reference to outlive `'query` (= `'static` here). `queries_rs` is a
    // local Vec, but the actual query data (`QuerySection::source` strings)
    // lives in `_query_tapes` (Arc-owned). The slice itself is only read
    // during parsing; no reference into the Vec's allocation is stored in the
    // returned Store. The raw-parts coercion satisfies the lifetime bound
    // without leaking memory.
    let queries_slice =
        unsafe { std::slice::from_raw_parts(queries_rs.as_ptr(), queries_rs.len()) };
    let store = match ::scah::parse(html_str, queries_slice) {
        Ok(store) => store,
        Err(ParseError::EmptyQueries) => {
            return Err(napi::Error::new(
                napi::Status::ArrayExpected,
                "parse requires at least one query".to_owned(),
            ));
        }
        Err(ParseError::MaximumDepthExceeded) => {
            return Err(napi::Error::new(
                napi::Status::GenericFailure,
                "HTML nesting depth exceeds the maximum supported depth".to_owned(),
            ));
        }
        Err(ParseError::TextCaptureRequired) => {
            return Err(napi::Error::new(
                napi::Status::GenericFailure,
                "internal error: unexpected TextCaptureRequired from parse".to_owned(),
            ));
        }
    };

    Ok(JSStore {
        store: Arc::new(store),
        _html: html,
        _query_tapes: query_tapes,
    })
}
