use std::ptr::{NonNull, null_mut};

use napi::Result;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use scah_ffi::{
    ScahQuery, ScahQueryBuilder, ScahQuerySectionId, ScahSave, ScahStatus, scah_query_all,
    scah_query_builder_all, scah_query_builder_append, scah_query_builder_build,
    scah_query_builder_clone, scah_query_builder_current_section, scah_query_builder_first,
    scah_query_builder_free, scah_query_first, scah_query_free,
};

use crate::ffi::{status_to_error, string_view};

#[napi(object, js_name = "Save")]
#[derive(Clone, Copy, Debug)]
pub struct JsSave {
    pub inner_html: Option<bool>,
    pub text_content: Option<bool>,
}

#[napi]
impl JsSave {
    #[napi]
    pub fn only_inner_html() -> Self {
        Self {
            inner_html: Some(true),
            text_content: Some(false),
        }
    }

    #[napi]
    pub fn only_text_content() -> Self {
        Self {
            inner_html: Some(false),
            text_content: Some(true),
        }
    }

    #[napi]
    pub fn all() -> Self {
        Self {
            inner_html: Some(true),
            text_content: Some(true),
        }
    }

    #[napi]
    pub fn none() -> Self {
        Self {
            inner_html: Some(false),
            text_content: Some(false),
        }
    }

    #[napi]
    pub fn new(inner_html: Option<bool>, text_content: Option<bool>) -> Self {
        Self {
            inner_html,
            text_content,
        }
    }

    fn to_scah_save(self) -> ScahSave {
        ScahSave {
            inner_html: u8::from(self.inner_html.unwrap_or(false)),
            text_content: u8::from(self.text_content.unwrap_or(false)),
        }
    }
}

fn save_or_none(save: Option<JsSave>) -> ScahSave {
    save.unwrap_or_else(JsSave::none).to_scah_save()
}

fn new_root_all(selector: &str, save: ScahSave) -> Result<JsQueryBuilder> {
    let mut out: *mut ScahQueryBuilder = null_mut();
    let mut err = null_mut();
    let status = scah_query_all(string_view(selector), save, &mut out, &mut err);
    if status != ScahStatus::Ok {
        return Err(status_to_error(status, err));
    }
    Ok(JsQueryBuilder {
        handle: NonNull::new(out)
            .ok_or_else(|| Error::from_reason("scah_query_all returned null builder".to_owned()))?,
    })
}

fn new_root_first(selector: &str, save: ScahSave) -> Result<JsQueryBuilder> {
    let mut out: *mut ScahQueryBuilder = null_mut();
    let mut err = null_mut();
    let status = scah_query_first(string_view(selector), save, &mut out, &mut err);
    if status != ScahStatus::Ok {
        return Err(status_to_error(status, err));
    }
    Ok(JsQueryBuilder {
        handle: NonNull::new(out).ok_or_else(|| {
            Error::from_reason("scah_query_first returned null builder".to_owned())
        })?,
    })
}

fn clone_builder(handle: NonNull<ScahQueryBuilder>) -> Result<JsQueryBuilder> {
    let mut out: *mut ScahQueryBuilder = null_mut();
    let mut err = null_mut();
    let status = scah_query_builder_clone(handle.as_ptr(), &mut out, &mut err);
    if status != ScahStatus::Ok {
        return Err(status_to_error(status, err));
    }
    Ok(JsQueryBuilder {
        handle: NonNull::new(out).ok_or_else(|| {
            Error::from_reason("scah_query_builder_clone returned null builder".to_owned())
        })?,
    })
}

#[napi(js_name = "QueryBuilder")]
pub struct JsQueryBuilder {
    pub(crate) handle: NonNull<ScahQueryBuilder>,
}

impl Drop for JsQueryBuilder {
    fn drop(&mut self) {
        scah_query_builder_free(self.handle.as_ptr());
    }
}

#[napi]
impl JsQueryBuilder {
    #[napi]
    pub fn all(&mut self, selector: String, save: Option<JsSave>) -> Result<JsQueryBuilder> {
        let mut err = null_mut();
        let status = scah_query_builder_all(
            self.handle.as_ptr(),
            string_view(&selector),
            save_or_none(save),
            &mut err,
        );
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }
        clone_builder(self.handle)
    }

    #[napi]
    pub fn first(&mut self, selector: String, save: Option<JsSave>) -> Result<JsQueryBuilder> {
        let mut err = null_mut();
        let status = scah_query_builder_first(
            self.handle.as_ptr(),
            string_view(&selector),
            save_or_none(save),
            &mut err,
        );
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }
        clone_builder(self.handle)
    }

    #[napi]
    pub fn then(
        &mut self,
        callback: Function<JsQueryFactory, Vec<Reference<JsQueryBuilder>>>,
    ) -> Result<JsQueryBuilder> {
        let mut parent: ScahQuerySectionId = 0;
        let mut err = null_mut();
        let status =
            scah_query_builder_current_section(self.handle.as_ptr(), &mut parent, &mut err);
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }

        let factory = JsQueryFactory { _data: true };
        let builders = callback.call(factory)?;

        for child in &builders {
            let mut err = null_mut();
            let status = scah_query_builder_append(
                self.handle.as_ptr(),
                parent,
                child.handle.as_ptr(),
                &mut err,
            );
            if status != ScahStatus::Ok {
                return Err(status_to_error(status, err));
            }
        }

        clone_builder(self.handle)
    }

    #[napi]
    pub fn build(&self) -> Result<JsQuery> {
        let mut out: *mut ScahQuery = null_mut();
        let mut err = null_mut();
        let status = scah_query_builder_build(self.handle.as_ptr(), &mut out, &mut err);
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }
        Ok(JsQuery {
            handle: NonNull::new(out).ok_or_else(|| {
                Error::from_reason("scah_query_builder_build returned null query".to_owned())
            })?,
        })
    }
}

#[napi(js_name = "QueryFactory")]
pub struct JsQueryFactory {
    // if there isn't any data in the struct, no object is created, thus `.then` doesn't work
    _data: bool,
}

#[napi]
impl JsQueryFactory {
    #[napi]
    pub fn all(&self, selector: String, save: Option<JsSave>) -> Result<JsQueryBuilder> {
        new_root_all(&selector, save_or_none(save))
    }

    #[napi]
    pub fn first(&self, selector: String, save: Option<JsSave>) -> Result<JsQueryBuilder> {
        new_root_first(&selector, save_or_none(save))
    }
}

#[napi]
pub struct JsQuery {
    pub(crate) handle: NonNull<ScahQuery>,
}

impl Drop for JsQuery {
    fn drop(&mut self) {
        scah_query_free(self.handle.as_ptr());
    }
}

#[napi(js_name = "Query")]
pub struct JsQueryStatic;

#[napi]
impl JsQueryStatic {
    #[napi]
    pub fn all(selector: String, save: Option<JsSave>) -> Result<JsQueryBuilder> {
        new_root_all(&selector, save_or_none(save))
    }

    #[napi]
    pub fn first(selector: String, save: Option<JsSave>) -> Result<JsQueryBuilder> {
        new_root_first(&selector, save_or_none(save))
    }
}
