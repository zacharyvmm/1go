pub mod element;
#[allow(dead_code)] // consumed by the text parser in the next stack layer
mod entities;
mod indexer;
mod open_elements;
pub mod parser;
mod simd_classifier;
pub mod tag;

#[cfg(feature = "simd-bench-internals")]
pub(crate) use indexer::IndexingMode;
#[cfg(feature = "simd-bench-internals")]
pub(crate) use simd_classifier::BlockClassifier;
