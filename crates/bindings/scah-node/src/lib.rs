use napi::Result;
use napi::bindgen_prelude::*;
use napi_derive::napi;

use ::scah::{Query, QueryMultiplexer, Reader, XHtmlParser};

use std::sync::Arc;

mod query;
use query::JsQuery;
mod store;
use store::JSStore;

mod elements;

#[napi]
#[allow(dead_code)]
fn parse(html: String, queries: Vec<Reference<JsQuery>>) -> Result<JSStore> {
    if queries.is_empty() {
        return Err(napi::Error::new(
            napi::Status::ArrayExpected,
            "parse requires at least one query".to_owned(),
        ));
    }

    let html = Arc::new(html);
    let html_bytes = html.as_ref().as_bytes();
    let html_bytes = unsafe { std::slice::from_raw_parts(html_bytes.as_ptr(), html_bytes.len()) };
    let queries_rs = queries
        .iter()
        .map(|q| q.query.clone())
        .collect::<Vec<Query>>();

    let slice = unsafe { std::slice::from_raw_parts(queries_rs.as_ptr(), queries_rs.len()) };
    let selectors = QueryMultiplexer::new(slice);
    let mut parser = XHtmlParser::with_capacity(selectors, html_bytes.len());
    let mut reader = Reader::from_bytes(html_bytes);
    while parser.next(&mut reader) {}

    let store = std::sync::Arc::new(parser.matches());

    Ok(JSStore { store, _html: html })
}
