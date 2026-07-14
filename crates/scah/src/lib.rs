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
//! let store = parse(html, queries).expect("parse succeeds");
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
//! let store = parse(html, queries).expect("parse succeeds");
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

/// Errors that can occur during parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The query slice passed to [`parse`] is empty.
    /// At least one query is required.
    EmptyQueries,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::EmptyQueries => write!(f, "parse requires at least one query"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse an HTML string against one or more pre-built [`Query`] objects and
/// return a [`Result`] containing a [`Store`] with all matched elements.
///
/// This is the main entry point of scah. It wires together the streaming
/// [`XHtmlParser`], the [`QueryMultiplexer`], and the result [`Store`].
///
/// # Errors
///
/// Returns [`ParseError::EmptyQueries`] if the query slice is empty.
/// At least one query is required.
///
/// # Parameters
///
/// - `html`: The HTML source string. All returned string slices in the
///   resulting [`Store`] borrow directly from this string (zero-copy).
/// - `queries`: A slice of compiled [`Query`] objects. Each query is
///   executed concurrently against the same token stream in a single pass.
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
/// let store = parse(html, queries).expect("parse succeeds");
///
/// let links: Vec<_> = store.get("a").unwrap().collect();
/// assert_eq!(links.len(), 1);
/// assert_eq!(links[0].name, "a");
/// ```
pub fn parse<'a: 'query, 'html: 'query, 'query: 'html, Q>(
    html: &'html str,
    queries: &'a [Q],
) -> Result<Store<'html, 'query>, ParseError>
where
    Q: QuerySpec<'query>,
{
    if queries.is_empty() {
        return Err(ParseError::EmptyQueries);
    }

    let no_extra_allocations = queries.iter().all(|q| q.exit_at_section_end().is_some());

    let selectors = QueryMultiplexer::new(queries);

    let mut parser = if no_extra_allocations {
        XHtmlParser::new(selectors)
    } else {
        XHtmlParser::with_capacity(selectors, html.len())
    };

    let mut reader = Reader::new(html);
    parser.trace_parse_started(html.len(), queries.len());
    while parser.next(&mut reader) {}

    Ok(parser.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_slice_returns_error() {
        let html = "<main><a href='x'>x</a></main>";
        let queries: &[Query] = &[];

        let result = parse(html, queries);

        assert!(matches!(result, Err(ParseError::EmptyQueries)));
    }

    #[test]
    fn non_empty_query_slice_succeeds() {
        let html = "<main><a href='x'>x</a></main>";
        let queries = &[Query::all("a", Save::all())
            .expect("valid selector")
            .build()];

        let store = parse(html, queries).expect("parse succeeds");

        assert_eq!(store.get("a").unwrap().count(), 1);
    }

    #[test]
    fn parse_first_query_skips_full_document_preallocation() {
        let filler = "<span class=\"filler\"></span>".repeat(10_000);
        let html_len = filler.len();
        let html = format!("<div id=\"hit\"></div>{}", filler);

        let query = Query::first("#hit", Save::none()).unwrap().build();
        let queries = &[query];
        let store = parse(&html, queries).unwrap();

        // Early-exit path uses XHtmlParser::new → Store::default()
        // which creates empty arenas. After parsing one match, capacity
        // should be driven by Vec growth (~4-8), not the full document
        // length (capacity path would reserve html_len / 48 ≈ 5k+).
        let capacity_path_reservation = html_len / 48;
        assert!(
            store.elements.capacity() < capacity_path_reservation,
            "early-exit parse capacity ({}) must be far below full-document reservation ({})",
            store.elements.capacity(),
            capacity_path_reservation,
        );
        assert!(
            store.attributes.capacity() < html_len / 24,
            "early-exit parse must not preallocate attribute arena"
        );

        // Results must still be correct.
        let hits: Vec<_> = store.get("#hit").unwrap().collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "div");
    }

    #[test]
    fn parse_all_query_uses_capacity_preallocation_path() {
        let html = format!(
            "<div id=\"hit\"></div>{}",
            "<span class=\"filler\"></span>".repeat(5_000)
        );

        let query = Query::all("#hit", Save::none()).unwrap().build();
        let queries = &[query];
        let store = parse(&html, queries).unwrap();

        // .all() queries don't have exit_at_section_end → uses capacity path.
        assert!(
            store.elements.capacity() > 0,
            "non-early-exit parse must preallocate element arena"
        );

        // Text content should NOT be reserved when Save::none().
        assert_eq!(
            store.text_content.content.capacity(),
            0,
            "capacity path with Save::none must skip text buffer preallocation"
        );
    }

    #[test]
    fn parse_with_save_text_content_reserves_text_buffer() {
        let html = "<div>text content here</div>".repeat(5_000);

        let query = Query::all("div", Save::only_text_content())
            .unwrap()
            .build();
        let queries = &[query];
        let store = parse(&html, queries).unwrap();

        // Text content should be preallocated when saving text content.
        assert!(
            store.text_content.content.capacity() > 0,
            "text buffer must be preallocated when queries need text content"
        );
    }

    #[test]
    fn parse_first_early_exit_still_captures_text_when_needed() {
        let html = "<div id=\"hit\">important text</div>".to_string()
            + &"<span>filler</span>".repeat(1_000);

        let query = Query::first("#hit", Save::only_text_content())
            .unwrap()
            .build();
        let queries = &[query];
        let store = parse(&html, queries).unwrap();

        let hits: Vec<_> = store.get("#hit").unwrap().collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text_content(&store), Some("important text"));

        // Early-exit with XHtmlParser::new uses Store::default() with no
        // preallocation, but matches are still recorded correctly.
    }
}
