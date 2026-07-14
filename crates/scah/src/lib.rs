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
    let reserve_text_content = selectors.requires_text_content();

    let mut parser =
        XHtmlParser::with_capacity_options(selectors, html.len(), reserve_text_content);

    let mut reader = Reader::new(html);
    parser.trace_parse_started(html.len(), queries.len());
    while parser.next(&mut reader) {}

    parser.finish()
}
