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
- `AttributeKey`, `AttributeValue`, `AttributeBool` - Attribute components

### 3. TapeParser (`tape_parser.rs`)

Stage 2 of the pipeline: consumes the structural index to build a flat tape,
then drives the existing `QueryMultiplexer` for DOM construction.

**Key features:**
- Separates structural indexing (parallel) from DOM construction (sequential)
- Cache-friendly sequential tape access
- Compatible with existing `QueryMultiplexer` and `Store` infrastructure
- Proper text content handling
- Support for malformed HTML

## Public API

### `parse_tape(html, queries)`

Alternative to `parse()` that uses the two-stage pipeline:

```rust
use scah::{Query, Save, parse_tape};

let html = "<div><a href='link'>Hello</a></div>";
let queries = &[Query::all("a", Save::all()).unwrap().build()];
let store = parse_tape(html, queries);
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

The tape-based parser is optimized for:
- Large documents where SIMD scanning provides significant speedup
- Documents with many structural characters (tags, attributes)
- Scenarios where cache-friendly sequential access is beneficial

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

1. **Parallel Stage 1**: Use rayon to parallelize structural indexing for very large documents
2. **Tape Compression**: Encode common patterns (like `<div>`) more compactly
3. **Prefetching**: Add software prefetch hints for Stage 2 tape consumption
4. **AVX-512 Support**: Use 512-bit SIMD for even faster structural scanning
