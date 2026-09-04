use ::scah::{Attribute, ElementId, Store};

use napi::bindgen_prelude::*;
use napi::{Env, Error, Result, Status};
use napi_derive::napi;

#[napi(object)]
pub struct JsonElement<'a> {
    pub name: String,
    pub id: Option<String>,
    pub class: Option<String>,
    pub attributes: Object<'a>,
    pub inner_html: Option<String>,
    pub text_content: Option<String>,
}

#[napi(js_name = "Element")]
pub struct JsElement {
    pub(super) store: std::sync::Arc<Store<'static, 'static>>,
    pub(super) id: ElementId,
}

#[napi]
impl JsElement {
    #[napi]
    pub fn to_json<'a>(&'a self, env: Env) -> Result<JsonElement<'a>> {
        let element = self
            .store
            .elements
            .get(self.id.index())
            .expect("The Element ID should be valid");

        let json = JsonElement {
            name: element.name.to_string(),
            id: element.id.map(|s| s.to_string()),
            class: element.class.map(|s| s.to_string()),
            attributes: self.attributes(env)?,
            inner_html: element.inner_html.map(|s| s.to_string()),
            text_content: element.text(&self.store).map(|s| s.to_string()),
        };

        Ok(json)
    }

    #[napi(getter)]
    pub fn name(&self) -> Option<&str> {
        self.store.elements.get(self.id.index()).map(|e| e.name)
    }

    #[napi(getter)]
    pub fn class_name(&self) -> Option<&str> {
        self.store
            .elements
            .get(self.id.index())
            .and_then(|e| e.class)
    }

    #[napi(getter)]
    pub fn id(&self) -> Option<&str> {
        self.store.elements.get(self.id.index()).and_then(|e| e.id)
    }

    #[napi]
    pub fn get_attribute(&self, key: String) -> Option<&str> {
        self.store
            .elements
            .get(self.id.index())
            .and_then(|e| e.attribute(&self.store, &key))
    }

    #[napi(getter)]
    pub fn attributes<'a>(&'a self, env: Env) -> Result<Object<'a>> {
        let mut object = Object::new(&env)?;
        let attributes = self
            .store
            .elements
            .get(self.id.index())
            .and_then(|e| e.attributes(&self.store));

        if let Some(attrs) = attributes {
            for Attribute { key, value } in attrs {
                object.set(*key, *value)?
            }
        }
        Ok(object)
    }

    #[napi(getter)]
    pub fn inner_html(&self) -> Option<&str> {
        self.store
            .elements
            .get(self.id.index())
            .and_then(|e| e.inner_html)
    }

    #[napi(getter)]
    pub fn text_content(&self) -> Option<&str> {
        self.store
            .elements
            .get(self.id.index())
            .and_then(|e| e.text(&self.store))
    }

    #[napi]
    pub fn get(&self, query: String) -> Result<Vec<JsElement>> {
        let element = self
            .store
            .elements
            .get(self.id.index())
            .expect("The Element ID should be valid");
        let children = element.get(&self.store, &query);
        match children {
            None => Err(Error::new(
                Status::GenericFailure,
                format!("This Element does not have children selected with `{query}`"),
            )),
            Some(children) => Ok(children
                .map(|e| JsElement {
                    store: self.store.clone(),
                    id: unsafe { self.store.elements.index_of(e) },
                })
                .collect()),
        }
    }
}
