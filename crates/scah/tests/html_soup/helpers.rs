#![allow(dead_code)]

use scah::{Element, Query, Save, Store, parse};

pub fn parse_all<'a>(html: &'a str, selectors: &[&'a str]) -> Store<'a, 'a> {
    let queries = selectors
        .iter()
        .map(|selector| Query::all(selector, Save::all()).unwrap().build())
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let queries = Box::leak(queries);

    parse(html, queries).expect("parse succeeds")
}

pub fn parse_with_saves<'a>(html: &'a str, queries: &[(&'a str, Save)]) -> Store<'a, 'a> {
    let queries = queries
        .iter()
        .map(|(selector, save)| Query::all(selector, *save).unwrap().build())
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let queries = Box::leak(queries);

    parse(html, queries).expect("parse succeeds")
}

pub fn elements<'a>(store: &'a Store<'a, 'a>, selector: &str) -> Vec<&'a Element<'a>> {
    store
        .get(selector)
        .map(|items| items.collect())
        .unwrap_or_default()
}

pub fn texts<'a>(store: &'a Store<'a, 'a>, selector: &str) -> Vec<Option<&'a str>> {
    elements(store, selector)
        .into_iter()
        .map(|element| element.text_content(store))
        .collect()
}

pub fn inner_htmls<'a>(store: &'a Store<'a, 'a>, selector: &str) -> Vec<Option<&'a str>> {
    elements(store, selector)
        .into_iter()
        .map(|element| element.inner_html)
        .collect()
}

pub fn attr<'a>(store: &'a Store<'a, 'a>, selector: &str, key: &str) -> Vec<Option<&'a str>> {
    elements(store, selector)
        .into_iter()
        .map(|element| element.attribute(store, key))
        .collect()
}
