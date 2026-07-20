//! C ABI for scah: owned opaque handles over the core Rust library.
//!
//! Architecture:
//!
//! ```text
//! scah-query-ir / scah
//!         │
//!         ▼
//!     scah-ffi
//! owned handles + C ABI
//! ```
//!
//! Foreign callers never see Rust lifetimes. Query builders own selector
//! strings until build; compiled queries own a selector tape; stores own HTML
//! and query backing storage; elements retain their store.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod error;
mod owned_query;
mod owned_store;
mod query;
mod store;
mod string;

pub use error::{ScahError, ScahStatus, scah_error_free, scah_error_message};
pub use query::{
    ScahQuery, ScahQueryBuilder, ScahQuerySectionId, scah_query_all, scah_query_builder_all,
    scah_query_builder_append, scah_query_builder_build, scah_query_builder_clone,
    scah_query_builder_current_section, scah_query_builder_first, scah_query_builder_free,
    scah_query_first, scah_query_free,
};
pub use store::{
    ScahElement, ScahElementList, ScahStore, scah_abi_version, scah_element_attribute_at,
    scah_element_attribute_count, scah_element_class_name, scah_element_free, scah_element_get,
    scah_element_get_attribute, scah_element_id, scah_element_inner_html, scah_element_list_free,
    scah_element_list_get, scah_element_list_len, scah_element_name, scah_element_text_content,
    scah_parse, scah_store_free, scah_store_get, scah_store_len,
};
pub use string::{
    ScahOptionalStringView, ScahSave, ScahStringView, scah_save_all, scah_save_none,
    scah_save_only_inner_html, scah_save_only_text_content,
};
