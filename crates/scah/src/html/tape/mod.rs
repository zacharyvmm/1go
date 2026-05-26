//! Tape-based HTML parsing infrastructure (Phase 2: Two-Stage Pipeline)
//!
//! This module implements the simdjson-inspired two-stage pipeline:
//! - **Stage 1 (Parallel):** SIMD-accelerated structural indexing that scans the
//!   entire input and produces a flat "tape" of tagged positions.
//! - **Stage 2 (Sequential):** Cache-friendly DOM construction that consumes the
//!   tape entries and drives the existing `QueryMultiplexer`.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────┐     ┌─────────────────┐     ┌──────────────────┐
//! │   HTML Input     │────▶│  Structural     │────▶│  Tape-Based      │
//! │   (bytes)        │     │  Index (SIMD)   │     │  DOM Builder     │
//! └─────────────────┘     └─────────────────┘     └──────────────────┘
//!                                │                         │
//!                                ▼                         ▼
//!                         Vec<u32> positions        QueryMultiplexer
//!                                                  + Store
//! ```
//!
//! ## Benefits
//! - Sequential memory writes during indexing (no pointer chasing)
//! - Cache-friendly reads during DOM construction
//! - Reduced allocator pressure (flat arrays vs tree nodes)
//! - Clear separation of concerns (parallel scan vs sequential build)

mod tape_entry;
mod tape_parser;
mod structural_scanner;

pub use tape_entry::{TapeEntry, TapeEntryKind};
pub use tape_parser::TapeParser;
pub use structural_scanner::StructuralIndex;
