pub mod element;
mod entities;
mod open_elements;
pub mod parser;
pub mod tag;
mod text_state;

#[cfg(feature = "bench-internals")]
pub use text_state::TextPathStats;
