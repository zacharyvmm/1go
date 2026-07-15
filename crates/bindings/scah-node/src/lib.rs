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
    let html = Arc::new(html);
    let html_str: &str = unsafe { std::mem::transmute(html.as_str()) };

    let mut query_tapes: Vec<Arc<Vec<u8>>> = Vec::with_capacity(queries.len());
    let mut queries_rs: Vec<scah::Query<'static>> = Vec::with_capacity(queries.len());
    for q in &queries {
        query_tapes.push(q._tape.clone());
        queries_rs.push(q.query.clone());
    }

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
    };

    Ok(JSStore {
        store: Arc::new(store),
        _html: html,
        _query_tapes: query_tapes,
    })
}
