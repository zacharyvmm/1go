pub mod element;
mod entities;
mod indexer;
mod open_elements;
pub mod parser;
mod simd_classifier;
pub mod tag;
mod text_edge;
mod text_state;

#[cfg(feature = "simd-bench-internals")]
pub(crate) use indexer::IndexingMode;
#[cfg(feature = "simd-bench-internals")]
pub(crate) use simd_classifier::BlockClassifier;
#[cfg(feature = "bench-internals")]
pub use text_state::TextPathStats;
