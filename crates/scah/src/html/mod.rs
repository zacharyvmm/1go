pub mod element;
mod entities;
mod open_elements;
pub mod parser;
pub mod parser_no_text;
pub mod tag;
mod text_edge;
mod text_state;

#[cfg(feature = "bench-internals")]
pub use text_state::TextPathStats;
