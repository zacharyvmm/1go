use std::ptr::{NonNull, null_mut};

use napi::Result;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use scah_ffi::{
    ScahElement, ScahElementList, ScahOptionalStringView, ScahStatus, ScahStringView,
    scah_element_attribute_at, scah_element_attribute_count, scah_element_class_name,
    scah_element_free, scah_element_get, scah_element_get_attribute, scah_element_id,
    scah_element_inner_html, scah_element_list_free, scah_element_list_get, scah_element_list_len,
    scah_element_name, scah_element_text_content,
};

use crate::ffi::{optional_to_option, status_to_error, string_view, view_to_string};

/// Convert an owned FFI element list into owned `JsElement` handles, then free the list.
pub(crate) fn take_element_list(list: *mut ScahElementList) -> Result<Vec<JsElement>> {
    if list.is_null() {
        return Ok(Vec::new());
    }

    let mut len = 0usize;
    let mut err = null_mut();
    let status = unsafe { scah_element_list_len(list, &mut len, &mut err) };
    if status != ScahStatus::Ok {
        unsafe {
            scah_element_list_free(list);
        }
        return Err(status_to_error(status, err));
    }

    let mut elements = Vec::with_capacity(len);
    for i in 0..len {
        let mut out: *mut ScahElement = null_mut();
        let mut err = null_mut();
        let status = unsafe { scah_element_list_get(list, i, &mut out, &mut err) };
        if status != ScahStatus::Ok {
            unsafe {
                scah_element_list_free(list);
            }
            return Err(status_to_error(status, err));
        }
        let handle = match NonNull::new(out) {
            Some(h) => h,
            None => {
                unsafe {
                    scah_element_list_free(list);
                }
                return Err(Error::from_reason(
                    "scah_element_list_get returned null element".to_owned(),
                ));
            }
        };
        elements.push(JsElement { handle });
    }

    unsafe {
        scah_element_list_free(list);
    }
    Ok(elements)
}

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
    pub(crate) handle: NonNull<ScahElement>,
}

impl Drop for JsElement {
    fn drop(&mut self) {
        unsafe {
            scah_element_free(self.handle.as_ptr());
        }
    }
}

#[napi]
impl JsElement {
    #[napi]
    pub fn to_json<'a>(&'a self, env: &'a Env) -> Result<JsonElement<'a>> {
        Ok(JsonElement {
            name: self.name()?.unwrap_or_default(),
            id: self.id()?,
            class: self.class_name()?,
            attributes: self.attributes(env)?,
            inner_html: self.inner_html()?,
            text_content: self.text_content()?,
        })
    }

    #[napi(getter)]
    pub fn name(&self) -> Result<Option<String>> {
        let mut view = ScahStringView {
            data: std::ptr::null(),
            len: 0,
        };
        let mut err = null_mut();
        let status = unsafe { scah_element_name(self.handle.as_ptr(), &mut view, &mut err) };
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }
        Ok(Some(view_to_string(view)))
    }

    #[napi(getter)]
    pub fn class_name(&self) -> Result<Option<String>> {
        let mut opt = ScahOptionalStringView {
            value: ScahStringView {
                data: std::ptr::null(),
                len: 0,
            },
            is_some: 0,
        };
        let mut err = null_mut();
        let status = unsafe { scah_element_class_name(self.handle.as_ptr(), &mut opt, &mut err) };
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }
        Ok(optional_to_option(opt))
    }

    #[napi(getter)]
    pub fn id(&self) -> Result<Option<String>> {
        let mut opt = ScahOptionalStringView {
            value: ScahStringView {
                data: std::ptr::null(),
                len: 0,
            },
            is_some: 0,
        };
        let mut err = null_mut();
        let status = unsafe { scah_element_id(self.handle.as_ptr(), &mut opt, &mut err) };
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }
        Ok(optional_to_option(opt))
    }

    #[napi]
    pub fn get_attribute(&self, key: String) -> Result<Option<String>> {
        let mut opt = ScahOptionalStringView {
            value: ScahStringView {
                data: std::ptr::null(),
                len: 0,
            },
            is_some: 0,
        };
        let mut err = null_mut();
        let status = unsafe {
            scah_element_get_attribute(self.handle.as_ptr(), string_view(&key), &mut opt, &mut err)
        };
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }
        Ok(optional_to_option(opt))
    }

    #[napi(getter)]
    pub fn attributes<'a>(&'a self, env: &'a Env) -> Result<Object<'a>> {
        let mut object = Object::new(env)?;

        let mut count = 0usize;
        let mut err = null_mut();
        let status =
            unsafe { scah_element_attribute_count(self.handle.as_ptr(), &mut count, &mut err) };
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }

        for i in 0..count {
            let mut key = ScahStringView {
                data: std::ptr::null(),
                len: 0,
            };
            let mut value = ScahOptionalStringView {
                value: ScahStringView {
                    data: std::ptr::null(),
                    len: 0,
                },
                is_some: 0,
            };
            let mut err = null_mut();
            let status = unsafe {
                scah_element_attribute_at(self.handle.as_ptr(), i, &mut key, &mut value, &mut err)
            };
            if status != ScahStatus::Ok {
                return Err(status_to_error(status, err));
            }
            object.set(view_to_string(key), optional_to_option(value))?;
        }

        Ok(object)
    }

    #[napi(getter)]
    pub fn inner_html(&self) -> Result<Option<String>> {
        let mut opt = ScahOptionalStringView {
            value: ScahStringView {
                data: std::ptr::null(),
                len: 0,
            },
            is_some: 0,
        };
        let mut err = null_mut();
        let status = unsafe { scah_element_inner_html(self.handle.as_ptr(), &mut opt, &mut err) };
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }
        Ok(optional_to_option(opt))
    }

    #[napi(getter)]
    pub fn text_content(&self) -> Result<Option<String>> {
        let mut opt = ScahOptionalStringView {
            value: ScahStringView {
                data: std::ptr::null(),
                len: 0,
            },
            is_some: 0,
        };
        let mut err = null_mut();
        let status = unsafe { scah_element_text_content(self.handle.as_ptr(), &mut opt, &mut err) };
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }
        Ok(optional_to_option(opt))
    }

    #[napi]
    pub fn get(&self, query: String) -> Result<Vec<JsElement>> {
        let mut list: *mut ScahElementList = null_mut();
        let mut found = 0u8;
        let mut err = null_mut();
        let status = unsafe {
            scah_element_get(
                self.handle.as_ptr(),
                string_view(&query),
                &mut list,
                &mut found,
                &mut err,
            )
        };
        if status != ScahStatus::Ok {
            return Err(status_to_error(status, err));
        }
        if found == 0 {
            return Err(Error::new(
                Status::GenericFailure,
                format!("This Element does not have children selected with `{query}`"),
            ));
        }
        take_element_list(list)
    }
}
