//! # scah - Streaming CSS-selector-driven HTML extraction
//!
//! **scah** (*scan HTML*) is a high-performance parsing library that bridges the gap
//! between SAX/StAX streaming efficiency and DOM convenience. Instead of loading an
//! entire document into memory or manually tracking parser state, you declare what
//! you want with **CSS selectors**; the library handles the streaming complexity and
//! builds a targeted [`Store`] containing only your selections.
//!
//! ## Highlights
//!
//! | Feature | Detail |
//! |---------|--------|
//! | **Streaming core** | Built on StAX: constant memory regardless of document size |
//! | **Familiar API** | CSS selectors including `>` (child) and ` ` (descendant) combinators |
//! | **Composable queries** | Chain selections with [`QueryBuilder::then`] for hierarchical data extraction |
//! | **Zero-copy** | Element names, attributes, and inner HTML are `&str` slices into the source |
//! | **Multi-language** | Rust core with Python and TypeScript/JavaScript bindings |
//!
//! ## Quick Start
//!
//! ```rust
//! use scah::{Query, Save, parse};
//!
//! let html = r#"
//!     <main>
//!         <section>
//!             <a href="link1">Link 1</a>
//!             <a href="link2">Link 2</a>
//!         </section>
//!     </main>
//! "#;
//!
//! // Build a query: find all <a> tags with an href attribute
//! // that are direct children of a <section> inside <main>.
//! let queries = &[
//!     Query::all("main > section > a[href]", Save::all())
//!         .expect("valid selector")
//!         .build()
//! ];
//!
//! let store = parse(html, queries);
//!
//! // Iterate over matched elements
//! for element in store.get("main > section > a[href]").unwrap() {
//!     println!("{}: {}", element.name, element.attribute(&store, "href").unwrap());
//! }
//! ```
//!
//! ## Structured Querying with `.then()`
//!
//! Instead of flat filtering, you can nest queries using closures.
//! Child queries only run within the context of their parent match,
//! making extraction of hierarchical relationships both efficient and ergonomic:
//!
//! ```rust
//! use scah::{Query, Save, parse};
//!
//! # let html = "<main><section><a href='x'>Link</a></section></main>";
//! let queries = &[Query::all("main > section", Save::all())
//!     .expect("valid selector")
//!     .then(|section| {
//!         Ok([
//!             section.all("> a[href]", Save::all())?,
//!             section.all("div a", Save::all())?,
//!         ])
//!     })
//!     .expect("valid child selectors")
//!     .build()];
//!
//! let store = parse(html, queries);
//! ```
//!
//! ## Architecture
//!
//! Internally, scah is composed of the following layers:
//!
//! 1. **[`Reader`]**: A zero-copy byte-level cursor over the HTML source.
//! 2. **CSS selector compiler**: Parses selector strings into a compact
//!    automaton of [`Query`] transitions.
//! 3. **[`XHtmlParser`]**: A streaming StAX parser that emits open/close events.
//! 4. **[`QueryMultiplexer`]**: Drives one or more query executors against
//!    the token stream simultaneously.
//! 5. **[`Store`]**: An arena-based result set that collects matched
//!    [`Element`]s, their attributes, and (optionally) inner HTML / text content.
//!
//! ## Supported CSS Selector Syntax
//!
//! | Syntax | Example | Status |
//! |--------|---------|--------|
//! | **Tag name** | `a`, `div` | Working |
//! | **ID** | `#my-id` | Working |
//! | **Class** | `.my-class` | Working |
//! | **Descendant combinator** | `main section a` | Working |
//! | **Child combinator** | `main > section` | Working |
//! | **Attribute presence** | `a[href]` | Working |
//! | **Attribute exact match** | `a[href="url"]` | Working |
//! | **Attribute prefix** | `a[href^="https"]` | Working |
//! | **Attribute suffix** | `a[href$=".com"]` | Working |
//! | **Attribute substring** | `a[href*="example"]` | Working |
//! | **Adjacent sibling** | `h1 + p` | Coming soon |
//! | **General sibling** | `h1 ~ p` | Coming soon |

pub mod debug;
mod engine;
mod html;
mod store;
mod support;

#[cfg(all(any(debug_assertions, test), feature = "otel"))]
mod otel;

pub use engine::multiplexer::QueryMultiplexer;
pub use html::element::builder::XHtmlElement;
pub use html::parser::XHtmlParser;
pub use html::tape::{
    AttrFlags, ChunkEndState, ChunkResult, ChunkSplitter, CompactAttrEntry, DocumentProfile,
    FusedTapeBuilder, StructuralIndex, TapeEntry, TapeEntryKind, TapeMerger, TapeParser,
};
pub use scah_macros::query;
pub use scah_query_ir::lazy;
pub use scah_query_ir::{
    Attribute, AttributeSelection, AttributeSelectionKind, AttributeSelections, ClassSelections,
    Combinator, ElementPredicate, IElement, Position, Query, QueryBuilder, QueryFactory,
    QuerySection, QuerySectionId, QuerySpec, Save, SelectionKind, SelectorParseError, StaticQuery,
    Transition, TransitionId,
};
pub use scah_reader::Reader;
pub use store::{Element, ElementId, Store};

/// Parse an HTML string against one or more pre-built [`Query`] objects and
/// return a [`Store`] containing all matched elements.
///
/// This is the main entry point of scah. It wires together the streaming
/// [`XHtmlParser`], the [`QueryMultiplexer`], and the result [`Store`].
///
/// # Parameters
///
/// - `html`: The HTML source string. All returned string slices in the
///   resulting [`Store`] borrow directly from this string (zero-copy).
/// - `queries`: A slice of compiled [`Query`] objects. Each query is
///   executed concurrently against the same token stream in a single pass.
///
/// # Returns
///
/// A [`Store`] containing all matched elements. Use [`Store::get`] with the
/// original selector string to retrieve results for a specific query.
///
/// # Example
///
/// ```rust
/// use scah::{Query, Save, parse};
///
/// let html = "<div><a href='link'>Hello</a></div>";
/// let queries = &[Query::all("a", Save::all())
///     .expect("valid selector")
///     .build()];
/// let store = parse(html, queries);
///
/// let links: Vec<_> = store.get("a").unwrap().collect();
/// assert_eq!(links.len(), 1);
/// assert_eq!(links[0].name, "a");
/// ```
pub fn parse<'a: 'query, 'html: 'query, 'query: 'html, Q>(
    html: &'html str,
    queries: &'a [Q],
) -> Store<'html, 'query>
where
    Q: QuerySpec<'query>,
{
    let selectors = QueryMultiplexer::new(queries);

    let mut parser = if selectors.requires_text_content() {
        XHtmlParser::with_capacity(selectors, html.len())
    } else {
        XHtmlParser::new(selectors)
    };

    let mut reader = Reader::new(html);
    parser.trace_parse_started(html.len(), queries.len());
    while parser.next(&mut reader) {}

    parser.finish()
}

/// Parse HTML using the tape-based two-stage pipeline
///
/// This is an alternative to [`parse`] that uses the two-stage pipeline:
/// 1. **Stage 1 (SIMD):** Scans the entire input to build a structural index
/// 2. **Stage 2 (Sequential):** Builds a flat tape and drives DOM construction
///
/// # When to use this
///
/// The tape-based parser is optimized for:
/// - Large documents where SIMD scanning provides significant speedup
/// - Documents with many structural characters (tags, attributes)
/// - Scenarios where cache-friendly sequential access is beneficial
///
/// # Parameters
///
/// - `html`: The HTML source string
/// - `queries`: A slice of compiled [`Query`] objects
///
/// # Returns
///
/// A [`Store`] containing all matched elements
///
/// # Example
///
/// ```rust
/// use scah::{Query, Save, parse_tape};
///
/// let html = "<div><a href='link'>Hello</a></div>";
/// let queries = &[Query::all("a", Save::all())
///     .expect("valid selector")
///     .build()];
/// let store = parse_tape(html, queries);
///
/// let links: Vec<_> = store.get("a").unwrap().collect();
/// assert_eq!(links.len(), 1);
/// assert_eq!(links[0].name, "a");
/// ```
pub fn parse_tape<'a: 'query, 'html: 'query, 'query: 'html, Q>(
    html: &'html str,
    queries: &'a [Q],
) -> Store<'html, 'query>
where
    Q: QuerySpec<'query>,
{
    if let Some(store) = html::simple_tag_parser::parse_if_simple_tag(html, queries) {
        return store;
    }

    let selectors = QueryMultiplexer::new(queries);
    let parser = if selectors.requires_text_content() {
        TapeParser::with_capacity(selectors, html.as_bytes(), html.len())
    } else {
        TapeParser::new(selectors, html.as_bytes())
    };

    parser.parse()
}

/// Parse HTML using the fused single-pass tape pipeline
///
/// This is the most optimized parsing path that combines SIMD structural
/// scanning with attribute tokenization in a single pass, eliminating
/// the redundant attribute re-scan in the current 3-stage pipeline.
///
/// # When to use this
///
/// The fused parser is optimized for:
/// - Attribute-heavy HTML (forms, data-* attributes)
/// - Documents where attribute parsing is a bottleneck
/// - Maximum throughput with pre-tokenized attributes
///
/// # Parameters
///
/// - `html`: The HTML source string
/// - `queries`: A slice of compiled [`Query`] objects
///
/// # Returns
///
/// A [`Store`] containing all matched elements
///
/// # Example
///
/// ```rust
/// use scah::{Query, Save, parse_fused};
///
/// let html = "<div><a href='link' class='test'>Hello</a></div>";
/// let queries = &[Query::all("a", Save::all())
///     .expect("valid selector")
///     .build()];
/// let store = parse_fused(html, queries);
///
/// let links: Vec<_> = store.get("a").unwrap().collect();
/// assert_eq!(links.len(), 1);
/// assert_eq!(links[0].name, "a");
/// assert_eq!(links[0].attribute(&store, "href"), Some("link"));
/// ```
pub fn parse_fused<'a: 'query, 'html: 'query, 'query: 'html, Q>(
    html: &'html str,
    queries: &'a [Q],
) -> Store<'html, 'query>
where
    Q: QuerySpec<'query>,
{
    let selectors = QueryMultiplexer::new(queries);
    let parser = if selectors.requires_text_content() {
        TapeParser::with_capacity(selectors, html.as_bytes(), html.len())
    } else {
        TapeParser::new(selectors, html.as_bytes())
    };

    parser.parse_fused()
}

/// Parse HTML using the parallel fused tape pipeline.
///
/// This is the most optimized parsing path that splits the input into 64KB
/// chunks, processes each chunk in parallel using rayon, then merges the
/// results with boundary fixups.
///
/// For documents smaller than 128KB, falls back to sequential fused parsing.
///
/// # When to use this
///
/// The parallel parser is optimized for:
/// - Large documents (>100KB) where parallelism provides significant speedup
/// - Multi-core systems where rayon can distribute work across threads
/// - Attribute-heavy HTML where the fused tape builder already excels
///
/// # Parameters
///
/// - `html`: The HTML source string
/// - `queries`: A slice of compiled [`Query`] objects
///
/// # Returns
///
/// A [`Store`] containing all matched elements
///
/// # Example
///
/// ```rust
/// use scah::{Query, Save, parse_fused_parallel};
///
/// let html = "<div><a href='link' class='test'>Hello</a></div>";
/// let queries = &[Query::all("a", Save::all())
///     .expect("valid selector")
///     .build()];
/// let store = parse_fused_parallel(html, queries);
///
/// let links: Vec<_> = store.get("a").unwrap().collect();
/// assert_eq!(links.len(), 1);
/// assert_eq!(links[0].name, "a");
/// assert_eq!(links[0].attribute(&store, "href"), Some("link"));
/// ```
pub fn parse_fused_parallel<'a: 'query, 'html: 'query, 'query: 'html, Q>(
    html: &'html str,
    queries: &'a [Q],
) -> Store<'html, 'query>
where
    Q: QuerySpec<'query>,
{
    let selectors = QueryMultiplexer::new(queries);
    let parser = if selectors.requires_text_content() {
        TapeParser::with_capacity(selectors, html.as_bytes(), html.len())
    } else {
        TapeParser::new(selectors, html.as_bytes())
    };

    parser.parse_fused_parallel()
}

/// Parse HTML using optimized direct parallel writing.
///
/// This is the most optimized parsing path that avoids merge overhead by:
/// 1. First pass: Each chunk computes only counts (fast, no allocations)
/// 2. Prefix scan: Compute cumulative offsets for each chunk
/// 3. Second pass: Each chunk writes directly to the final buffer
///
/// For documents smaller than 1MB, falls back to sequential fused parsing.
///
/// # When to use this
///
/// This parser is optimized for:
/// - Very large documents (>1MB) where merge overhead is significant
/// - Multi-core systems where rayon can distribute work across threads
/// - Scenarios where minimizing intermediate allocations is critical
///
/// # Parameters
///
/// - `html`: The HTML source string
/// - `queries`: A slice of compiled [`Query`] objects
///
/// # Returns
///
/// A [`Store`] containing all matched elements
///
/// # Example
///
/// ```rust
/// use scah::{Query, Save, parse_fused_parallel_direct};
///
/// let html = "<div><a href='link' class='test'>Hello</a></div>";
/// let queries = &[Query::all("a", Save::all())
///     .expect("valid selector")
///     .build()];
/// let store = parse_fused_parallel_direct(html, queries);
///
/// let links: Vec<_> = store.get("a").unwrap().collect();
/// assert_eq!(links.len(), 1);
/// assert_eq!(links[0].name, "a");
/// assert_eq!(links[0].attribute(&store, "href"), Some("link"));
/// ```
pub fn parse_fused_parallel_direct<'a: 'query, 'html: 'query, 'query: 'html, Q>(
    html: &'html str,
    queries: &'a [Q],
) -> Store<'html, 'query>
where
    Q: QuerySpec<'query>,
{
    let selectors = QueryMultiplexer::new(queries);
    let parser = if selectors.requires_text_content() {
        TapeParser::with_capacity(selectors, html.as_bytes(), html.len())
    } else {
        TapeParser::new(selectors, html.as_bytes())
    };

    parser.parse_fused_parallel_direct()
}

/// Parse HTML using adaptive parallel fused tape pipeline.
///
/// This method analyzes the document to determine optimal chunk size based on:
/// - Document size
/// - Tag density (tags per KB)
/// - Attribute density (attrs per tag)
/// - Available CPU cores
///
/// For documents smaller than 1MB, falls back to sequential fused parsing.
///
/// # When to use this
///
/// This parser is optimized for:
/// - Documents where the optimal chunk size varies based on content
/// - Mixed HTML with varying tag/attribute densities
/// - Multi-core systems where load balancing is important
///
/// # Parameters
///
/// - `html`: The HTML source string
/// - `queries`: A slice of compiled [`Query`] objects
///
/// # Returns
///
/// A [`Store`] containing all matched elements
///
/// # Example
///
/// ```rust
/// use scah::{Query, Save, parse_fused_parallel_adaptive};
///
/// let html = "<div><a href='link' class='test'>Hello</a></div>";
/// let queries = &[Query::all("a", Save::all())
///     .expect("valid selector")
///     .build()];
/// let store = parse_fused_parallel_adaptive(html, queries);
///
/// let links: Vec<_> = store.get("a").unwrap().collect();
/// assert_eq!(links.len(), 1);
/// assert_eq!(links[0].name, "a");
/// assert_eq!(links[0].attribute(&store, "href"), Some("link"));
/// ```
pub fn parse_fused_parallel_adaptive<'a: 'query, 'html: 'query, 'query: 'html, Q>(
    html: &'html str,
    queries: &'a [Q],
) -> Store<'html, 'query>
where
    Q: QuerySpec<'query>,
{
    let selectors = QueryMultiplexer::new(queries);
    let parser = if selectors.requires_text_content() {
        TapeParser::with_capacity(selectors, html.as_bytes(), html.len())
    } else {
        TapeParser::new(selectors, html.as_bytes())
    };

    parser.parse_fused_parallel_adaptive()
}

/// Analyze document characteristics for adaptive chunk sizing.
///
/// This function returns a [`DocumentProfile`] containing information about
/// the document's tag density, attribute density, and other characteristics
/// that can be used to optimize parallel processing.
///
/// # Arguments
///
/// * `html` - The HTML source string
///
/// # Returns
///
/// A [`DocumentProfile`] with document characteristics
///
/// # Example
///
/// ```rust
/// use scah::analyze_document;
///
/// let html = "<div class='test' id='main'>Hello</div>";
/// let profile = analyze_document(html);
///
/// println!("Tag density: {:.1} tags/KB", profile.tag_density);
/// println!("Attribute density: {:.1} attrs/tag", profile.attr_density);
/// println!("Optimal chunk size: {} bytes", profile.optimal_chunk_size());
/// ```
pub fn analyze_document(html: &str) -> DocumentProfile {
    DocumentProfile::analyze(html.as_bytes())
}

/// Build a structural index from HTML input using SIMD acceleration
///
/// This function exposes Stage 1 of the two-stage pipeline for testing
/// and benchmarking purposes.
///
/// # Arguments
///
/// * `html` - The HTML source string
///
/// # Returns
///
/// A [`StructuralIndex`] containing positions of all structural characters
///
/// # Example
///
/// ```rust
/// use scah::index_html;
///
/// let html = "<div class='test'>Hello</div>";
/// let index = index_html(html);
///
/// println!("Found {} structural characters", index.len());
/// for pos in index.iter() {
///     println!("  Position {}: '{}'", pos, html.as_bytes()[pos as usize] as char);
/// }
/// ```
pub fn index_html(html: &str) -> StructuralIndex {
    StructuralIndex::build(html.as_bytes())
}
