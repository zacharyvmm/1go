pub mod element;
mod indexer;
mod open_elements;
pub mod parser;
mod simd_classifier;
pub mod tag;

#[cfg(feature = "bench-internals")]
pub(crate) use simd_classifier::BlockClassifier;
