use napi::Result;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use scah_ffi::{BindingStore, ScahElementId, ScahStatus};

use crate::ffi::status_to_error;

fn map_status<T>(result: std::result::Result<T, ScahStatus>) -> Result<T> {
    result.map_err(|status| status_to_error(status, std::ptr::null_mut()))
}

#[napi(js_name = "Element")]
pub struct JsElement {
    pub(crate) store: BindingStore,
    pub(crate) id: ScahElementId,
}

#[napi]
impl JsElement {
    #[napi(js_name = "toJson")]
    pub fn to_json(&self, env: Env) -> Result<Object<'_>> {
        let mut object = Object::new(&env)?;
        object.set("name", map_status(self.store.name(self.id))?)?;
        set_optional_undefined(&mut object, "id", map_status(self.store.id_attr(self.id))?)?;
        set_optional_undefined(
            &mut object,
            "class",
            map_status(self.store.class_name(self.id))?,
        )?;
        set_optional_undefined(
            &mut object,
            "innerHtml",
            map_status(self.store.inner_html(self.id))?,
        )?;
        set_optional_undefined(
            &mut object,
            "textContent",
            map_status(self.store.text_content(self.id))?,
        )?;
        object.set("attributes", self.attributes(env)?)?;
        Ok(object)
    }

    #[napi(getter)]
    pub fn name(&self) -> Result<&str> {
        map_status(self.store.name(self.id))
    }

    #[napi(getter)]
    pub fn class_name(&self) -> Result<Option<&str>> {
        map_status(self.store.class_name(self.id))
    }

    #[napi(getter)]
    pub fn id(&self) -> Result<Option<&str>> {
        map_status(self.store.id_attr(self.id))
    }

    #[napi]
    pub fn get_attribute(&self, key: String) -> Result<Option<&str>> {
        map_status(self.store.get_attribute(self.id, &key))
    }

    #[napi(getter)]
    pub fn attributes(&self, env: Env) -> Result<Object<'_>> {
        let mut object = Object::new(&env)?;
        map_status(self.store.for_each_attribute(self.id, |key, value| {
            let _ = object.set(key, value);
        }))?;
        Ok(object)
    }

    #[napi(getter)]
    pub fn inner_html(&self) -> Result<Option<&str>> {
        map_status(self.store.inner_html(self.id))
    }

    #[napi(getter)]
    pub fn text_content(&self) -> Result<Option<&str>> {
        map_status(self.store.text_content(self.id))
    }

    #[napi]
    pub fn get(&self, query: String) -> Result<Vec<JsElement>> {
        match map_status(
            self.store
                .child_get_with(self.id, &query, |store, id| JsElement { store, id }),
        )? {
            None => Err(Error::new(
                Status::GenericFailure,
                format!("This Element does not have children selected with `{query}`"),
            )),
            Some(children) => Ok(children),
        }
    }
}

fn set_optional_undefined(object: &mut Object, key: &str, value: Option<&str>) -> Result<()> {
    match value {
        None => object.set(key, ()),
        Some(s) => object.set(key, s),
    }
}
