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
//!    [`Element`]s, their attributes, and (optionally) inner HTML / raw or normalized text.
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
use html::parser_no_text::NoTextParser;
pub use scah_macros::query;
pub use scah_query_ir::lazy;
pub use scah_query_ir::{
    Attribute, AttributeSelection, AttributeSelectionKind, AttributeSelections, ClassSelections,
    Combinator, ElementPredicate, IElement, Position, Query, QueryBuilder, QueryFactory,
    QuerySection, QuerySectionId, QuerySpec, Save, SelectionKind, SelectorParseError, StaticQuery,
    TextRequirements, Transition, TransitionId,
};
pub use scah_reader::Reader;
pub use store::{CapacityOptions, Element, ElementId, Store};

/// Internal APIs used by benchmarks.
///
/// Cursor instrumentation is available only with `bench-internals`.
#[doc(hidden)]
pub mod bench_internals {
    pub use crate::html::tag::{ClassifiedTag, ScopeKind, TagFlags, TextTagFlags};

    #[cfg(feature = "bench-internals")]
    pub use crate::engine::cursor::ScopedCursor;
    #[cfg(feature = "bench-internals")]
    pub use crate::engine::multiplexer::CursorStatsSnapshot;
    #[cfg(feature = "bench-internals")]
    use crate::engine::multiplexer::QueryMultiplexer;
    #[cfg(feature = "bench-internals")]
    pub use crate::html::TextPathStats;
    #[cfg(feature = "bench-internals")]
    use crate::store::Store;
    #[cfg(feature = "bench-internals")]
    use crate::{ParseError, QuerySpec, Reader, XHtmlParser};

    /// Parse HTML and return peak cursor counts with the result store.
    #[cfg(feature = "bench-internals")]
    pub fn parse_with_cursor_stats<'a: 'query, 'html: 'query, 'query: 'html, Q>(
        html: &'html str,
        queries: &'a [Q],
    ) -> Result<(Store<'html, 'query>, CursorStatsSnapshot), ParseError>
    where
        Q: QuerySpec<'query>,
    {
        if queries.is_empty() {
            return Err(ParseError::EmptyQueries);
        }

        let no_extra_allocations = queries.iter().all(|q| q.exit_at_section_end().is_some());

        let mut selectors = QueryMultiplexer::new_with_cursor_stats(queries);
        selectors.sample_cursor_stats();

        let requirements = selectors.text_requirements();
        let capture = requirements.raw_text || requirements.text;

        if capture {
            run_capturing_parser_with_cursor_stats(html, selectors, no_extra_allocations)
        } else {
            run_no_text_parser_with_cursor_stats(html, selectors, no_extra_allocations)
        }
    }

    #[cfg(feature = "bench-internals")]
    fn run_no_text_parser_with_cursor_stats<'html, 'query: 'html, Q>(
        html: &'html str,
        selectors: QueryMultiplexer<'query, Q>,
        no_extra_allocations: bool,
    ) -> Result<(Store<'html, 'query>, CursorStatsSnapshot), ParseError>
    where
        Q: QuerySpec<'query>,
    {
        let mut parser = if no_extra_allocations {
            crate::NoTextParser::new(selectors)
        } else {
            crate::NoTextParser::with_capacity(selectors, html.len())
        };

        let mut reader = Reader::new(html);
        while parser.next(&mut reader) {}

        if let Some(err) = parser.take_parse_error() {
            return Err(err);
        }

        let stats = parser.selectors.cursor_stats_snapshot();
        Ok((parser.finish(), stats))
    }

    #[cfg(feature = "bench-internals")]
    fn run_capturing_parser_with_cursor_stats<'html, 'query: 'html, Q>(
        html: &'html str,
        selectors: QueryMultiplexer<'query, Q>,
        no_extra_allocations: bool,
    ) -> Result<(Store<'html, 'query>, CursorStatsSnapshot), ParseError>
    where
        Q: QuerySpec<'query>,
    {
        let mut parser = if no_extra_allocations {
            XHtmlParser::new(selectors)
        } else {
            XHtmlParser::with_capacity(selectors, html.len())
        };

        let mut reader = Reader::new(html);
        while parser.next(&mut reader) {}

        if let Some(err) = parser.take_parse_error() {
            return Err(err);
        }

        let stats = parser.selectors.cursor_stats_snapshot();
        Ok((parser.finish(), stats))
    }
}

#[cfg(feature = "bench-internals")]
pub use engine::multiplexer::CursorStatsSnapshot;

/// Errors that can occur during parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The query slice passed to [`parse`] is empty.
    EmptyQueries,
    /// The open-element stack exceeded [`engine::MAX_ELEMENT_DEPTH`].
    MaximumDepthExceeded,
    /// [`parse_without_text_capture`] was called with queries that request
    /// raw or normalized text. Use [`parse`] instead.
    TextCaptureRequired,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::EmptyQueries => write!(f, "parse requires at least one query"),
            ParseError::MaximumDepthExceeded => {
                write!(f, "HTML nesting depth exceeds the maximum supported depth")
            }
            ParseError::TextCaptureRequired => write!(
                f,
                "parse_without_text_capture cannot run queries that capture raw or normalized text; use parse"
            ),
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
/// Returns [`ParseError::EmptyQueries`] for an empty query slice and
/// [`ParseError::MaximumDepthExceeded`] when nesting exceeds the supported depth.
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
    let requirements = selectors.text_requirements();
    let capture = requirements.raw_text || requirements.text;
    let query_count = queries.len();

    // Specialize the parser so no-text parses do not keep text-capture code in
    // the monomorphized hot path. Binaries that only call
    // [`parse_without_text_capture`] never reference the capturing parser, so
    // LTO can drop it entirely (matching main's no-text image size).
    if capture {
        parse_with_parser(html, selectors, no_extra_allocations, query_count)
    } else {
        run_no_text_parser(html, selectors, no_extra_allocations, query_count)
    }
}

/// Parse HTML for queries that do not capture raw or normalized text.
///
/// This entry point never references the text-capturing parser, so no-text-only
/// binaries (and their Criterion harnesses) can drop entity tables and text-path
/// code via LTO. Prefer it for inner-HTML / no-content workloads that must stay
/// competitive with main.
///
/// # Errors
///
/// Returns [`ParseError::TextCaptureRequired`] when any query requests raw or
/// normalized text. Use [`parse`] for those workloads.
pub fn parse_without_text_capture<'a: 'query, 'html: 'query, 'query: 'html, Q>(
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
    let requirements = selectors.text_requirements();
    if requirements.raw_text || requirements.text {
        return Err(ParseError::TextCaptureRequired);
    }

    run_no_text_parser(html, selectors, no_extra_allocations, queries.len())
}

fn run_no_text_parser<'html: 'query, 'query: 'html, Q>(
    html: &'html str,
    selectors: QueryMultiplexer<'query, Q>,
    no_extra_allocations: bool,
    query_count: usize,
) -> Result<Store<'html, 'query>, ParseError>
where
    Q: QuerySpec<'query>,
{
    let mut parser = if no_extra_allocations {
        NoTextParser::new(selectors)
    } else {
        NoTextParser::with_capacity(selectors, html.len())
    };

    let mut reader = Reader::new(html);
    parser.trace_parse_started(html.len(), query_count);
    while parser.next(&mut reader) {}

    if let Some(err) = parser.take_parse_error() {
        return Err(err);
    }

    Ok(parser.finish())
}

fn parse_with_parser<'html: 'query, 'query: 'html, Q>(
    html: &'html str,
    selectors: QueryMultiplexer<'query, Q>,
    no_extra_allocations: bool,
    query_count: usize,
) -> Result<Store<'html, 'query>, ParseError>
where
    Q: QuerySpec<'query>,
{
    let mut parser = if no_extra_allocations {
        XHtmlParser::new(selectors)
    } else {
        XHtmlParser::with_capacity(selectors, html.len())
    };

    let mut reader = Reader::new(html);
    parser.trace_parse_started(html.len(), query_count);
    while parser.next(&mut reader) {}

    if let Some(err) = parser.take_parse_error() {
        return Err(err);
    }

    Ok(parser.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_without_text_capture_rejects_text_queries() {
        let html = "<p>x</p>";
        let queries = &[Query::all("p", Save::only_text())
            .expect("valid selector")
            .build()];
        let err = parse_without_text_capture(html, queries).unwrap_err();
        assert!(matches!(err, ParseError::TextCaptureRequired));
    }

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

        // Text buffers should NOT be reserved when Save::none().
        assert_eq!(
            store.text.raw_text.capacity(),
            0,
            "capacity path with Save::none must skip raw-text buffer preallocation"
        );
        assert_eq!(
            store.text.text.capacity(),
            0,
            "capacity path with Save::none must skip normalized-text buffer preallocation"
        );
    }

    #[test]
    fn parse_with_save_text_reserves_text_buffer() {
        let html = "<div>text content here</div>".repeat(5_000);

        let query = Query::all("div", Save::only_text()).unwrap().build();
        let queries = &[query];
        let store = parse(&html, queries).unwrap();

        // Normalized text should be preallocated when saving text.
        assert!(
            store.text.text.capacity() > 0,
            "normalized-text buffer must be preallocated when queries need text"
        );
    }

    #[test]
    fn parse_first_early_exit_still_captures_text_when_needed() {
        let html = "<div id=\"hit\">important text</div>".to_string()
            + &"<span>filler</span>".repeat(1_000);

        let query = Query::first("#hit", Save::only_text()).unwrap().build();
        let queries = &[query];
        let store = parse(&html, queries).unwrap();

        let hits: Vec<_> = store.get("#hit").unwrap().collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text(&store), Some("important text"));

        // Early-exit with XHtmlParser::new uses Store::default() with no
        // preallocation, but matches are still recorded correctly.
    }

    #[test]
    fn element_attribute_lookup_is_case_insensitive_for_html_attributes() {
        let html = "<a HREF='x'></a>";
        let queries = &[Query::all("a", Save::all())
            .expect("valid selector")
            .build()];
        let store = parse(html, queries).expect("parse succeeds");
        let a = store.get("a").unwrap().next().unwrap();

        assert_eq!(a.attribute(&store, "href"), Some("x"));
        assert_eq!(a.attribute(&store, "HREF"), Some("x"));
        assert_eq!(a.attribute(&store, "Href"), Some("x"));
    }

    #[cfg(feature = "bench-internals")]
    #[test]
    fn cursor_stats_disabled_for_normal_multiplexer() {
        use crate::engine::multiplexer::QueryMultiplexer;

        let query = Query::all("div", Save::none()).unwrap().build();
        let queries = [query];
        let selectors = QueryMultiplexer::new(&queries);
        assert!(!selectors.cursor_stats_enabled());

        let html = "<div><span></span></div>";
        let mut reader = Reader::new(html);
        let mut parser = XHtmlParser::new(selectors);
        while parser.next(&mut reader) {}

        assert!(!parser.selectors.cursor_stats_enabled());
        assert_eq!(
            parser.selectors.cursor_stats_snapshot(),
            CursorStatsSnapshot {
                peak_resident_cursor_slots: 0,
                peak_active_obligations: 0,
            }
        );
    }

    #[cfg(feature = "bench-internals")]
    #[test]
    fn cursor_stats_enabled_for_instrumented_multiplexer() {
        use crate::bench_internals::parse_with_cursor_stats;

        let query = Query::all("div p", Save::none()).unwrap().build();
        let html = "<div><div><p>x</p></div></div>";
        let (_, stats) = parse_with_cursor_stats(html, std::slice::from_ref(&query)).unwrap();

        assert!(stats.peak_resident_cursor_slots > 0);
        assert!(stats.peak_active_obligations > 0);
    }

    #[cfg(feature = "bench-internals")]
    #[test]
    fn instrumented_parse_matches_production_results() {
        use crate::bench_internals::parse_with_cursor_stats;

        let html = "<article><h1 id=\"a\">A</h1><p>B</p></article>";
        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| {
                Ok([
                    article.first("> h1", Save::all())?,
                    article.all("> p", Save::all())?,
                ])
            })
            .unwrap()
            .build();
        let queries = [query];

        let production = parse(html, &queries).unwrap();
        let (instrumented, _) = parse_with_cursor_stats(html, &queries).unwrap();

        assert_eq!(production.elements.len(), instrumented.elements.len());
        for (left, right) in production.elements.iter().zip(instrumented.elements.iter()) {
            assert_eq!(left.name, right.name);
            assert_eq!(left.id, right.id);
            assert_eq!(left.inner_html, right.inner_html);
        }
    }

    #[cfg(feature = "bench-internals")]
    #[test]
    fn peak_resident_cursor_slots_adversarial_depths() {
        use crate::bench_internals::parse_with_cursor_stats;

        fn nested_div_p(depth: u16) -> String {
            format!(
                "{opens}<p>x</p>{closes}",
                opens = "<div>".repeat(depth as usize),
                closes = "</div>".repeat(depth as usize),
            )
        }

        let div_p = Query::all("div p", Save::none()).unwrap().build();
        let stats_at_8 = parse_with_cursor_stats(&nested_div_p(8), std::slice::from_ref(&div_p))
            .unwrap()
            .1;
        let stats_at_512 =
            parse_with_cursor_stats(&nested_div_p(512), std::slice::from_ref(&div_p))
                .unwrap()
                .1;
        assert_eq!(
            stats_at_8.peak_resident_cursor_slots, stats_at_512.peak_resident_cursor_slots,
            "div p peak resident cursor slots must not grow with nesting depth"
        );
        assert!(
            stats_at_512.peak_resident_cursor_slots <= 3,
            "div p peak resident cursor slots {} exceeds budget",
            stats_at_512.peak_resident_cursor_slots
        );

        let div_gt_div_p = Query::all("div > div p", Save::none()).unwrap().build();
        for depth in [8_u16, 512] {
            let html = nested_div_p(depth);
            let stats = parse_with_cursor_stats(&html, std::slice::from_ref(&div_gt_div_p))
                .unwrap()
                .1;
            assert!(
                stats.peak_resident_cursor_slots <= depth as usize + 3,
                "div > div p peak resident cursor slots {} at depth {depth} exceeds budget",
                stats.peak_resident_cursor_slots
            );
        }
    }

    mod parser_implementation_parity_tests {
        use super::*;

        fn parse_general_with_position<'html, 'query, Q>(
            html: &'html str,
            queries: &'query [Q],
        ) -> Result<(Store<'html, 'query>, usize), ParseError>
        where
            'query: 'html,
            Q: QuerySpec<'query>,
        {
            let no_extra_allocations =
                queries.iter().all(|query| query.exit_at_section_end().is_some());

            let selectors = QueryMultiplexer::new(queries);
            let mut parser = if no_extra_allocations {
                XHtmlParser::new(selectors)
            } else {
                XHtmlParser::with_capacity(selectors, html.len())
            };

            let mut reader = Reader::new(html);
            while parser.next(&mut reader) {}

            if let Some(error) = parser.take_parse_error() {
                return Err(error);
            }

            Ok((parser.finish(), reader.get_position()))
        }

        fn parse_no_text_with_position<'html, 'query, Q>(
            html: &'html str,
            queries: &'query [Q],
        ) -> Result<(Store<'html, 'query>, usize), ParseError>
        where
            'query: 'html,
            Q: QuerySpec<'query>,
        {
            let no_extra_allocations =
                queries.iter().all(|query| query.exit_at_section_end().is_some());

            let selectors = QueryMultiplexer::new(queries);

            let requirements = selectors.text_requirements();
            assert!(
                !requirements.raw_text && !requirements.text,
                "test helper requires no-text queries"
            );

            let mut parser = if no_extra_allocations {
                NoTextParser::new(selectors)
            } else {
                NoTextParser::with_capacity(selectors, html.len())
            };

            let mut reader = Reader::new(html);
            while parser.next(&mut reader) {}

            if let Some(error) = parser.take_parse_error() {
                return Err(error);
            }

            Ok((parser.finish(), reader.get_position()))
        }

        #[test]
        fn parity_early_exit_reader_position() {
            let html = format!(
                "<div id=\"hit\">x</div>{}",
                "<span>filler</span>".repeat(5_000)
            );

            let general_queries =
                [Query::first("#hit", Save::none()).unwrap().build()];
            let specialized_queries = [general_queries[0].clone()];

            let (general_store, general_position) =
                parse_general_with_position(&html, &general_queries).unwrap();

            let (specialized_store, specialized_position) =
                parse_no_text_with_position(&html, &specialized_queries).unwrap();

            assert!(general_position < html.len());
            assert!(specialized_position < html.len());
            assert_eq!(general_position, specialized_position);

            assert_eq!(general_store.get("#hit").unwrap().count(), 1);
            assert_eq!(specialized_store.get("#hit").unwrap().count(), 1);
        }
    }
}
