# Phase 2: Two-Stage Pipeline Implementation

## Overview

This module implements the tape-based DOM construction infrastructure for scah's
two-stage HTML parsing pipeline, inspired by simdjson's architecture.

## Architecture

```
┌─────────────────┐     ┌─────────────────┐     ┌──────────────────┐
│   HTML Input     │────▶│  Structural     │────▶│  Tape-Based      │
│   (bytes)        │     │  Index (SIMD)   │     │  DOM Builder     │
└─────────────────┘     └─────────────────┘     └──────────────────┘
                               │                         │
                               ▼                         ▼
                        Vec<u32> positions        QueryMultiplexer
                                                 + Store
```

> **Note:** The parallel path (rayon-based chunked processing) has been removed.
> It was 4–6× slower than the streaming parser on real hardware due to chunk
> splitting, thread spawning, boundary fixup, and tape merging overhead.
> The sequential fused path is retained as a development backend.

## Components

### 1. StructuralIndex (`structural_scanner.rs`)

Stage 1 of the pipeline: SIMD-accelerated scanning of the entire HTML input
to find all structural characters (`<`, `>`, `"`, `'`).

**Key features:**
- Processes 64 bytes per iteration (2 × 32-byte AVX2 chunks)
- Falls back to scalar processing for tail bytes
- Produces a dense `Vec<u32>` of positions for cache-friendly Stage 2
- Binary search for efficient position lookups

### 2. TapeEntry (`tape_entry.rs`)

Flat tape representation of parsed HTML elements.

**Entry kinds:**
- `OpenTag` - Opening tags like `<div`, `<a`
- `CloseTag` - Closing tags like `</div>`, `</a>`
- `SelfClosingTag` - Self-closing tags like `<br/>`, `<img/>`
- `Comment` - HTML comments `<!-- ... -->`
- `Doctype` - DOCTYPE declarations
- `Text` - Text content between tags

### 3. TapeParser (`tape_parser.rs`)

Stage 2 of the pipeline: consumes the structural index to build a flat tape,
then drives the existing `QueryMultiplexer` for DOM construction.

**Key features:**
- Separates structural indexing from DOM construction
- Cache-friendly sequential tape access
- Compatible with existing `QueryMultiplexer` and `Store` infrastructure
- Proper text content handling
- Support for malformed HTML

### 4. FusedTapeBuilder (`structural_scanner.rs`)

Single-pass SIMD-driven tape builder that combines structural scanning with
attribute tokenization, eliminating the redundant attribute re-scan of the
3-stage pipeline.

## Public API

### `parse_tape(html, queries)` — DEPRECATED

> **⚠️ Deprecated.** Use [`parse_fused`] or the streaming [`parse`] instead.
> The 3-stage tape pipeline is 79% slower than streaming due to double-tokenization
> (SIMD structural scan → tape construction → Reader-based DOM re-tokenization).
> The 2-stage fused path avoids this by combining scanning and tokenization in
> one pass. This function is retained for benchmark comparison only.

```rust
use scah::{Query, Save, parse_tape};

let html = "<div><a href='link'>Hello</a></div>";
let queries = &[Query::all("a", Save::all()).unwrap().build()];
let store = parse_tape(html, queries);
```

### `parse_fused(html, queries)`

Uses the fused single-pass tape builder for attribute-heavy HTML:

```rust
use scah::{Query, Save, parse_fused};

let html = "<div><a href='link' class='test'>Hello</a></div>";
let queries = &[Query::all("a", Save::all()).unwrap().build()];
let store = parse_fused(html, queries);
```

### `index_html(html)`

Exposes Stage 1 for testing and benchmarking:

```rust
use scah::index_html;

let html = "<div class='test'>Hello</div>";
let index = index_html(html);

println!("Found {} structural characters", index.len());
for pos in index.iter() {
    println!("  Position {}: '{}'", pos, html.as_bytes()[pos as usize] as char);
}
```

## When to Use

The **streaming parser** (`parse()`) is the recommended path — it's the fastest
on real hardware (55 ms vs 70 ms for fused, 98 ms for 3-stage tape).

The tape-based parser is retained for:
- Development and benchmarking
- Comparing parser architectures
- Future query-guided lazy parsing integration

## Testing

Run the tape module tests:
```bash
cargo test --lib html::tape
```

Run all tests:
```bash
cargo test
```

## Future Improvements

1. **Query-guided lazy parsing**: Only build tape entries for regions that match
   active queries, eliminating wasted work on unmatched subtrees.
2. **Tape Compression**: Encode common patterns (like `<div>`) more compactly
3. **AVX-512 Support**: Use 512-bit SIMD for even faster structural scanning
