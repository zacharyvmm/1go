use napi::Result;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use scah_ffi::{
    ScahElementId, ScahOptionalStringView, ScahStatus, ScahStore, ScahStringView,
    scah_store_element_class_name, scah_store_element_get_attribute, scah_store_element_id,
    scah_store_element_inner_html, scah_store_element_name, scah_store_element_text_content,
    scah_store_element_view,
};
use std::sync::Arc;

use crate::ffi::{
    StoreOwner, map_status, optional_view_to_option, string_view, take_element_get, view_as_str,
    with_attributes,
};

#[napi(object)]
#[allow(dead_code)]
#[derive(Debug)]
pub struct JsonElement {
    pub name: String,
    pub id: Option<String>,
    pub class: Option<String>,
    #[napi(ts_type = "Record<string, string | null>")]
    pub attributes: std::collections::HashMap<String, Option<String>>,
    pub inner_html: Option<String>,
    pub text_content: Option<String>,
}

#[napi(js_name = "Element")]
pub struct JsElement {
    pub(crate) owner: Arc<StoreOwner>,
    pub(crate) id: ScahElementId,
    cached_name: Option<&'static str>,
}

impl JsElement {
    pub(crate) fn new(
        owner: Arc<StoreOwner>,
        id: ScahElementId,
        cached_name: Option<&'static str>,
    ) -> Self {
        Self {
            owner,
            id,
            cached_name,
        }
    }

    fn store_ptr(&self) -> *const ScahStore {
        self.owner.as_ptr()
    }
}

#[napi]
impl JsElement {
    #[napi(js_name = "toJson", ts_return_type = "JsonElement")]
    pub fn to_json(&self, env: Env) -> Result<Object<'_>> {
        let mut view = scah_ffi::ScahElementView {
            name: ScahStringView::empty(),
            id: ScahOptionalStringView::none(),
            class_name: ScahOptionalStringView::none(),
            inner_html: ScahOptionalStringView::none(),
            text_content: ScahOptionalStringView::none(),
            attribute_count: 0,
        };
        let mut error = std::ptr::null_mut();
        let status =
            unsafe { scah_store_element_view(self.store_ptr(), self.id, &mut view, &mut error) };
        map_status(status, error)?;

        let mut object = Object::new(&env)?;
        object.set("name", unsafe { view_as_str(view.name) })?;
        set_optional_undefined(&mut object, "id", optional_view_to_option(view.id))?;
        set_optional_undefined(
            &mut object,
            "class",
            optional_view_to_option(view.class_name),
        )?;
        set_optional_undefined(
            &mut object,
            "innerHtml",
            optional_view_to_option(view.inner_html),
        )?;
        set_optional_undefined(
            &mut object,
            "textContent",
            optional_view_to_option(view.text_content),
        )?;
        object.set("attributes", self.attributes(env)?)?;
        Ok(object)
    }

    #[napi(getter)]
    pub fn name(&self) -> Result<&str> {
        if let Some(name) = self.cached_name {
            return Ok(name);
        }
        let mut out = ScahStringView::empty();
        let status = unsafe {
            scah_store_element_name(self.store_ptr(), self.id, &mut out, std::ptr::null_mut())
        };
        if status != ScahStatus::Ok {
            map_status(status, std::ptr::null_mut())?;
        }
        Ok(unsafe { view_as_str(out) })
    }

    #[napi(getter)]
    pub fn class_name(&self) -> Result<Option<&str>> {
        let mut out = ScahOptionalStringView::none();
        let status = unsafe {
            scah_store_element_class_name(self.store_ptr(), self.id, &mut out, std::ptr::null_mut())
        };
        if status != ScahStatus::Ok {
            map_status(status, std::ptr::null_mut())?;
        }
        Ok(optional_view_to_option(out))
    }

    #[napi(getter)]
    pub fn id(&self) -> Result<Option<&str>> {
        let mut out = ScahOptionalStringView::none();
        let status = unsafe {
            scah_store_element_id(self.store_ptr(), self.id, &mut out, std::ptr::null_mut())
        };
        if status != ScahStatus::Ok {
            map_status(status, std::ptr::null_mut())?;
        }
        Ok(optional_view_to_option(out))
    }

    #[napi]
    pub fn get_attribute(&self, key: String) -> Result<Option<&str>> {
        let mut out = ScahOptionalStringView::none();
        let status = unsafe {
            scah_store_element_get_attribute(
                self.store_ptr(),
                self.id,
                string_view(&key),
                &mut out,
                std::ptr::null_mut(),
            )
        };
        if status != ScahStatus::Ok {
            map_status(status, std::ptr::null_mut())?;
        }
        Ok(optional_view_to_option(out))
    }

    #[napi(getter, ts_return_type = "Record<string, string | null>")]
    pub fn attributes(&self, env: Env) -> Result<Object<'_>> {
        with_attributes(self.store_ptr(), self.id, |attrs| {
            let mut object = Object::new(&env)?;
            for attr in attrs {
                let key = unsafe { view_as_str(attr.key) };
                let value = optional_view_to_option(attr.value);
                object.set(key, value)?;
            }
            Ok(object)
        })
    }

    #[napi(getter)]
    pub fn inner_html(&self) -> Result<Option<&str>> {
        let mut out = ScahOptionalStringView::none();
        let status = unsafe {
            scah_store_element_inner_html(self.store_ptr(), self.id, &mut out, std::ptr::null_mut())
        };
        if status != ScahStatus::Ok {
            map_status(status, std::ptr::null_mut())?;
        }
        Ok(optional_view_to_option(out))
    }

    #[napi(getter)]
    pub fn text_content(&self) -> Result<Option<&str>> {
        let mut out = ScahOptionalStringView::none();
        let status = unsafe {
            scah_store_element_text_content(
                self.store_ptr(),
                self.id,
                &mut out,
                std::ptr::null_mut(),
            )
        };
        if status != ScahStatus::Ok {
            map_status(status, std::ptr::null_mut())?;
        }
        Ok(optional_view_to_option(out))
    }

    #[napi]
    pub fn get(&self, query: String) -> Result<Vec<JsElement>> {
        match take_element_get(&self.owner, self.id, &query, JsElement::new)? {
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
