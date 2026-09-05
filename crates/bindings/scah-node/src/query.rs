use ::scah::lazy::{LazyQuery, LazyQueryBuilder};
use ::scah::{Query, QuerySectionId, Save};

use napi::Result;
use napi::bindgen_prelude::*;
use napi_derive::napi;

#[napi(object, js_name = "Save")]
#[derive(Clone, Copy, Debug)]
pub struct JsSave {
    pub inner_html: Option<bool>,
    /// Deprecated compatibility alias for `text`.
    pub text_content: Option<bool>,
    pub attributes: Option<bool>,
    pub raw_text: Option<bool>,
    pub text: Option<bool>,
}

impl JsSave {
    fn none() -> Self {
        Self {
            inner_html: Some(false),
            text_content: Some(false),
            attributes: Some(true),
            raw_text: Some(false),
            text: Some(false),
        }
    }

    fn to_save(self) -> Save {
        Save {
            inner_html: self.inner_html.unwrap_or(false),
            attributes: self.attributes.unwrap_or(true),
            raw_text: self.raw_text.unwrap_or(false),
            text: self.text.or(self.text_content).unwrap_or(false),
        }
    }
}

#[napi(js_name = "QueryBuilder")]
pub struct JsQueryBuilder {
    builder: LazyQueryBuilder<String>,
}

#[napi]
impl JsQueryBuilder {
    #[napi]
    pub fn all(&mut self, selector: String, save: Option<JsSave>) -> JsQueryBuilder {
        self.builder
            .all_mut(selector, save.unwrap_or_else(JsSave::none).to_save());

        JsQueryBuilder {
            builder: self.builder.clone(),
        }
    }
    #[napi]
    pub fn first(&mut self, selector: String, save: Option<JsSave>) -> JsQueryBuilder {
        self.builder
            .first_mut(selector, save.unwrap_or_else(JsSave::none).to_save());

        JsQueryBuilder {
            builder: self.builder.clone(),
        }
    }

    #[napi]
    pub fn then(
        &mut self,
        callback: Function<JsQueryFactory, Vec<Reference<JsQueryBuilder>>>,
    ) -> Result<JsQueryBuilder> {
        let factory = JsQueryFactory { _data: true };
        let builders = callback.call(factory)?;
        let children = builders.iter().map(|b| b.builder.clone());

        let current_index = QuerySectionId(self.builder.len() - 1);
        for child in children {
            self.builder.append(current_index, child);
        }

        Ok(JsQueryBuilder {
            builder: self.builder.clone(),
        })
    }

    #[napi]
    pub fn build(&self) -> Result<JsQuery> {
        let (_tape, query) = unsafe { self.builder.clone().try_to_query() }
            .map_err(|err| Error::from_reason(err.to_string()))?;
        Ok(JsQuery { _tape, query })
    }
}

#[napi(js_name = "QueryFactory")]
pub struct JsQueryFactory {
    // if their isn't any data in the struct, no object is created, thus the `.then` doesn't work
    _data: bool,
}

#[napi]
impl JsQueryFactory {
    #[napi]
    pub fn all(&self, selector: String, save: Option<JsSave>) -> JsQueryBuilder {
        JsQueryBuilder {
            builder: LazyQuery::all(selector, save.unwrap_or_else(JsSave::none).to_save()),
        }
    }

    #[napi]
    pub fn first(&self, selector: String, save: Option<JsSave>) -> JsQueryBuilder {
        JsQueryBuilder {
            builder: LazyQuery::first(selector, save.unwrap_or_else(JsSave::none).to_save()),
        }
    }
}

#[napi]
#[derive(Clone)]
pub struct JsQuery {
    pub(crate) _tape: std::sync::Arc<Vec<u8>>,
    pub(crate) query: Query<'static>,
}

#[napi(js_name = "Query")]
pub struct JsQueryStatic;

#[napi]
impl JsQueryStatic {
    #[napi]
    pub fn all(selector: String, save: Option<JsSave>) -> JsQueryBuilder {
        JsQueryBuilder {
            builder: LazyQuery::all(selector, save.unwrap_or_else(JsSave::none).to_save()),
        }
    }

    #[napi]
    pub fn first(selector: String, save: Option<JsSave>) -> JsQueryBuilder {
        JsQueryBuilder {
            builder: LazyQuery::first(selector, save.unwrap_or_else(JsSave::none).to_save()),
        }
    }
}
