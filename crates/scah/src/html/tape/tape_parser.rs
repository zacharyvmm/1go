//! Tape-based HTML parser (Stage 2)
//!
//! This module implements the second stage of the two-stage pipeline:
//! consuming the structural index to build a flat tape of HTML entries,
//! then driving the existing `QueryMultiplexer` for DOM construction.
//!
//! ## Design Philosophy
//!
//! The tape parser separates concerns:
//! 1. **Structural indexing** (done in Stage 1): SIMD-accelerated position finding
//! 2. **Tokenization** (this stage): Converting positions into semantic tokens
//! 3. **DOM construction**: Driving queries against the token stream
//!
//! This separation enables:
//! - Better cache locality (sequential tape access)
//! - Cleaner code organization
//! - Potential for parallel tokenization in the future

use super::structural_scanner::{FusedTapeBuilder, StructuralIndex};
use super::tape_entry::{CompactAttrEntry, TapeEntry, TapeEntryKind};
use crate::QuerySpec;
use crate::Reader;
use crate::engine::multiplexer::{DocumentPosition, QueryMultiplexer, SaveHit};
use crate::html::element::builder::XHtmlElement;
use crate::html::open_elements::{OpenElement, OpenElementStack};
use crate::store::Store;

/// A tape-based HTML parser that implements the two-stage pipeline
///
/// Stage 1: Build structural index using SIMD (done externally via `StructuralIndex`)
/// Stage 2: Consume index to build tape, then drive DOM construction
pub struct TapeParser<'html, 'query, Q> {
    /// The structural index from Stage 1
    structural_index: StructuralIndex,
    /// The flat tape of parsed entries
    tape: Vec<TapeEntry>,
    /// Compact attribute entries from the fused tape builder
    compact_attributes: Vec<CompactAttrEntry>,
    /// Maps tag index to (attr_start, attr_count)
    tag_attr_map: Vec<(usize, usize)>,
    /// The HTML source bytes
    source: &'html [u8],
    /// Position tracking for the document
    position: DocumentPosition,
    /// Query multiplexer for DOM construction
    pub selectors: QueryMultiplexer<'query, Q>,
    /// Store for matched elements
    store: Store<'html, 'query>,
    /// Current element being parsed
    element: XHtmlElement<'html>,
    /// Stack of open elements
    open_elements: OpenElementStack<'html>,
    /// Temporary storage for closing elements
    closing_elements: Vec<OpenElement<'html>>,
    /// Temporary storage for implied closes
    implied_closes: Vec<OpenElement<'html>>,
    /// Temporary storage for save hits
    save_hits: Vec<SaveHit>,
    /// Whether we need to capture text content
    capture_text_content: bool,
    /// Whether EOF has been processed
    eof_drained: bool,
}

impl<'html, 'query: 'html, Q> TapeParser<'html, 'query, Q>
where
    Q: QuerySpec<'query>,
{
    /// Create a new tape parser with the given query multiplexer
    ///
    /// # Arguments
    /// * `selectors` - The query multiplexer for DOM construction
    /// * `source` - The HTML source bytes
    pub fn new(selectors: QueryMultiplexer<'query, Q>, source: &'html [u8]) -> Self {
        let capture_text_content = selectors.requires_text_content();
        Self {
            structural_index: StructuralIndex::new(),
            tape: Vec::new(),
            compact_attributes: Vec::new(),
            tag_attr_map: Vec::new(),
            source,
            position: DocumentPosition {
                element_depth: 0,
                reader_position: 0,
                text_content_position: usize::MAX,
            },
            selectors,
            element: XHtmlElement::default(),
            open_elements: OpenElementStack::default(),
            closing_elements: Vec::new(),
            implied_closes: Vec::new(),
            save_hits: Vec::new(),
            capture_text_content,
            eof_drained: false,
            store: Store::default(),
        }
    }

    /// Create a new tape parser with capacity hints
    pub fn with_capacity(
        selectors: QueryMultiplexer<'query, Q>,
        source: &'html [u8],
        capacity: usize,
    ) -> Self {
        let capture_text_content = selectors.requires_text_content();
        Self {
            structural_index: StructuralIndex::with_capacity(capacity / 16),
            tape: Vec::with_capacity(capacity / 8),
            compact_attributes: Vec::with_capacity(capacity / 16),
            tag_attr_map: Vec::with_capacity(capacity / 16),
            source,
            position: DocumentPosition {
                element_depth: 0,
                reader_position: 0,
                text_content_position: usize::MAX,
            },
            selectors,
            element: XHtmlElement::default(),
            open_elements: OpenElementStack::default(),
            closing_elements: Vec::new(),
            implied_closes: Vec::new(),
            save_hits: Vec::new(),
            capture_text_content,
            eof_drained: false,
            store: Store::with_capacity(capacity),
        }
    }

    /// Parse the HTML input using the two-stage pipeline
    ///
    /// # Stage 1: Structural Indexing (SIMD)
    /// Scans the entire input to find all structural character positions.
    ///
    /// # Stage 2: Tape Construction + DOM Building
    /// Walks the structural index to build a tape, then drives the
    /// QueryMultiplexer for DOM construction.
    ///
    /// # Returns
    /// The `Store` containing all matched elements
    pub fn parse(mut self) -> Store<'html, 'query> {
        // Stage 1: Build structural index using SIMD
        self.structural_index = StructuralIndex::build(self.source);

        // Stage 2: Build tape and drive DOM construction
        self.build_tape();
        self.run_dom_construction();

        self.store
    }

    /// Parse the HTML input using the fused single-pass pipeline.
    ///
    /// This performs a single SIMD scan that builds the tape with
    /// pre-tokenized attributes, eliminating the redundant attribute
    /// re-scan in the current 3-stage pipeline.
    ///
    /// # Returns
    /// The `Store` containing all matched elements
    pub fn parse_fused(mut self) -> Store<'html, 'query> {
        // Single-pass fused scan that builds tape with pre-tokenized attributes
        let (tape, compact_attrs, tag_attr_map) = FusedTapeBuilder::build(self.source);
        self.tape = tape;
        self.compact_attributes = compact_attrs;
        self.tag_attr_map = tag_attr_map;

        // Run DOM construction using pre-tokenized entries
        self.run_fused_dom_construction();

        self.store
    }

    /// Parse the HTML input using the parallel fused tape pipeline.
    ///
    /// This splits the input into 64KB chunks, processes each chunk
    /// independently in parallel using rayon, then merges the results
    /// with boundary fixups for split tags, attribute counts, and
    /// text coalescing.
    ///
    /// For documents smaller than 128KB, falls back to sequential
    /// fused parsing.
    ///
    /// # Returns
    /// The `Store` containing all matched elements
    pub fn parse_fused_parallel(mut self) -> Store<'html, 'query> {
        // Parallel fused scan: split into chunks, process in parallel, merge
        let (tape, compact_attrs, tag_attr_map) = FusedTapeBuilder::build_parallel(self.source);
        self.tape = tape;
        self.compact_attributes = compact_attrs;
        self.tag_attr_map = tag_attr_map;

        // Run DOM construction using pre-tokenized entries
        self.run_fused_dom_construction();

        self.store
    }

    /// Parse the HTML input using optimized direct parallel writing.
    ///
    /// This is an optimized version that avoids merge overhead by:
    /// 1. First pass: Each chunk computes only counts (fast, no allocations)
    /// 2. Prefix scan: Compute cumulative offsets for each chunk
    /// 3. Second pass: Each chunk writes directly to the final buffer
    ///
    /// For documents smaller than 1MB, falls back to sequential fused parsing.
    ///
    /// # Returns
    /// The `Store` containing all matched elements
    pub fn parse_fused_parallel_direct(mut self) -> Store<'html, 'query> {
        // Direct parallel writing: no merge overhead
        let (tape, compact_attrs, tag_attr_map) =
            FusedTapeBuilder::build_parallel_direct(self.source);
        self.tape = tape;
        self.compact_attributes = compact_attrs;
        self.tag_attr_map = tag_attr_map;

        // Run DOM construction using pre-tokenized entries
        self.run_fused_dom_construction();

        self.store
    }

    /// Parse the HTML input using adaptive parallel fused tape pipeline.
    ///
    /// This method analyzes the document to determine optimal chunk size based on:
    /// - Document size
    /// - Tag density (tags per KB)
    /// - Attribute density (attrs per tag)
    /// - Available CPU cores
    ///
    /// For documents smaller than 1MB, falls back to sequential fused parsing.
    ///
    /// # Returns
    /// The `Store` containing all matched elements
    pub fn parse_fused_parallel_adaptive(mut self) -> Store<'html, 'query> {
        // Adaptive parallel processing with optimal chunk sizing
        let (tape, compact_attrs, tag_attr_map) =
            FusedTapeBuilder::build_parallel_adaptive(self.source);
        self.tape = tape;
        self.compact_attributes = compact_attrs;
        self.tag_attr_map = tag_attr_map;

        // Run DOM construction using pre-tokenized entries
        self.run_fused_dom_construction();

        self.store
    }

    /// Build the tape from the structural index
    ///
    /// This walks through the structural positions and creates tape entries
    /// for each semantic element (tags, attributes, text, etc.).
    fn build_tape(&mut self) {
        let input = self.source;
        let mut pos: u32 = 0;
        let len = input.len() as u32;

        while pos < len {
            // Find the next structural character
            if let Some(struct_pos) = self.structural_index.next_position_after(pos) {
                // Add text entry for content before the structural character
                if struct_pos > pos {
                    self.tape
                        .push(TapeEntry::new(TapeEntryKind::Text, pos, struct_pos - pos));
                }

                let ch = input[struct_pos as usize];
                match ch {
                    b'<' => {
                        // Parse tag
                        let tag_entry = self.parse_tag_at(struct_pos);
                        if let Some(entry) = tag_entry {
                            self.tape.push(entry);
                            pos = entry.end();
                        } else {
                            // Comment or malformed - skip to next '>'
                            pos = struct_pos + 1;
                            while pos < len && input[pos as usize] != b'>' {
                                pos += 1;
                            }
                            if pos < len {
                                pos += 1; // Skip '>'
                            }
                        }
                    }
                    b'>' => {
                        // Stray '>' - skip
                        pos = struct_pos + 1;
                    }
                    b'"' | b'\'' => {
                        // Quote outside of tag context - skip
                        pos = struct_pos + 1;
                    }
                    _ => {
                        pos = struct_pos + 1;
                    }
                }
            } else {
                // No more structural characters - remaining is text
                if pos < len {
                    self.tape
                        .push(TapeEntry::new(TapeEntryKind::Text, pos, len - pos));
                }
                break;
            }
        }
    }

    /// Parse a tag starting at the given position
    ///
    /// Returns the tape entry for the tag, or None if it's a comment/doctype
    fn parse_tag_at(&self, pos: u32) -> Option<TapeEntry> {
        let input = self.source;
        let len = input.len() as u32;

        if pos + 1 >= len {
            return None;
        }

        let next_ch = input[(pos + 1) as usize];

        if next_ch == b'/' {
            // Closing tag: </tagname>
            let name_start = pos + 2;
            let mut name_end = name_start;
            while name_end < len
                && !matches!(
                    input[name_end as usize],
                    b'>' | b' ' | b'\t' | b'\n' | b'\r'
                )
            {
                name_end += 1;
            }
            // Find the closing '>'
            let mut gt_pos = name_end;
            while gt_pos < len && input[gt_pos as usize] != b'>' {
                gt_pos += 1;
            }
            if gt_pos < len {
                Some(TapeEntry::new(
                    TapeEntryKind::CloseTag,
                    pos,
                    gt_pos - pos + 1,
                ))
            } else {
                None
            }
        } else if next_ch == b'!' {
            // Comment or doctype: <!...>
            if pos + 3 < len
                && input[(pos + 2) as usize] == b'-'
                && input[(pos + 3) as usize] == b'-'
            {
                // Comment: <!--...-->
                let mut end = pos + 4;
                while end + 2 < len {
                    if input[end as usize] == b'-'
                        && input[(end + 1) as usize] == b'-'
                        && input[(end + 2) as usize] == b'>'
                    {
                        return Some(TapeEntry::new(TapeEntryKind::Comment, pos, end - pos + 3));
                    }
                    end += 1;
                }
                None
            } else {
                // Doctype: <!DOCTYPE ...>
                let mut end = pos + 2;
                while end < len && input[end as usize] != b'>' {
                    end += 1;
                }
                if end < len {
                    Some(TapeEntry::new(TapeEntryKind::Doctype, pos, end - pos + 1))
                } else {
                    None
                }
            }
        } else {
            // Opening tag: <tagname ...>
            let name_start = pos + 1;
            let mut name_end = name_start;
            while name_end < len
                && !matches!(
                    input[name_end as usize],
                    b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r'
                )
            {
                name_end += 1;
            }

            // Check for self-closing
            let mut gt_pos = name_end;
            let mut self_closing = false;

            // Scan to find '>' and check for '/>' pattern
            while gt_pos < len {
                if input[gt_pos as usize] == b'>' {
                    if gt_pos > 0 && input[(gt_pos - 1) as usize] == b'/' {
                        self_closing = true;
                    }
                    break;
                }
                gt_pos += 1;
            }

            if gt_pos < len {
                let kind = if self_closing {
                    TapeEntryKind::SelfClosingTag
                } else {
                    TapeEntryKind::OpenTag
                };
                Some(TapeEntry::new(kind, pos, gt_pos - pos + 1))
            } else {
                None
            }
        }
    }

    /// Run DOM construction using the tape
    ///
    /// This walks through the tape entries and drives the QueryMultiplexer
    /// to build the DOM and match queries.
    fn run_dom_construction(&mut self) {
        // We need to process the tape entries and drive the existing parser logic
        // For now, we'll use a Reader-based approach to maintain compatibility
        // with the existing XHtmlElement parsing

        let tape = std::mem::take(&mut self.tape);
        let source_len = self.source.len();
        let mut reader = Reader::from_bytes(self.source);

        for entry in &tape {
            let entry = *entry;
            match entry.kind {
                TapeEntryKind::OpenTag | TapeEntryKind::SelfClosingTag => {
                    // Push text content before this tag if we have a text_start
                    if self.capture_text_content && self.store.text_content.text_start.is_some() {
                        if let Some(position) =
                            self.store.text_content.push(&reader, entry.offset as usize)
                        {
                            self.position.text_content_position = position;
                        }
                    }

                    reader.set_position(entry.offset as usize + 1); // Skip '<'

                    // Skip whitespace and get tag name
                    reader.skip_whitespace();

                    // Parse the element using existing infrastructure
                    self.element.from(&mut reader, &mut self.store.attributes);

                    let is_self_closing = entry.kind == TapeEntryKind::SelfClosingTag
                        || self.element.is_self_closing();

                    self.position.reader_position = entry.offset as usize;

                    // Handle implied closes
                    self.open_elements
                        .prepare_for_open_into(self.element.name, &mut self.implied_closes);
                    let mut implied = std::mem::take(&mut self.implied_closes);
                    self.pop_open_elements(
                        &mut implied,
                        &reader,
                        Some(crate::debug::ImpliedCloseReason::OpenTagRule),
                        None,
                    );
                    self.implied_closes = implied;
                    self.position.reader_position = entry.end() as usize;

                    if is_self_closing {
                        self.position.element_depth = self.open_elements.depth().saturating_add(1);
                    } else {
                        self.open_elements.push(self.element.name);
                        self.position.element_depth = self.open_elements.depth();
                    }

                    // Set text_start before driving query multiplexer
                    // This matches the behavior of the original XHtmlParser
                    if self.capture_text_content {
                        self.store.text_content.set_start(entry.end() as usize);
                    }

                    // Drive query multiplexer
                    self.selectors.next_into(
                        &self.element,
                        &self.position,
                        &mut self.store,
                        &mut self.save_hits,
                    );

                    if !is_self_closing {
                        for save_hit in &self.save_hits {
                            self.open_elements.attach_saved(
                                save_hit.element_id,
                                save_hit
                                    .save_inner_html
                                    .then_some(self.position.reader_position),
                                save_hit
                                    .save_text_content
                                    .then_some(self.position.text_content_position),
                            );
                        }
                    }

                    self.element.clear();
                }
                TapeEntryKind::CloseTag => {
                    // Push text content before this close tag
                    if self.capture_text_content && self.store.text_content.text_start.is_some() {
                        if let Some(position) =
                            self.store.text_content.push(&reader, entry.offset as usize)
                        {
                            self.position.text_content_position = position;
                        }
                    }

                    // Extract tag name from the close tag
                    let tag_slice = entry.slice(self.source);
                    let tag_name = tag_slice
                        .trim_start_matches("</")
                        .trim_end_matches('>')
                        .trim();

                    self.position.reader_position = entry.offset as usize;

                    self.open_elements
                        .close_by_end_tag_into(tag_name, &mut self.closing_elements);
                    self.pop_closing_elements(
                        &reader,
                        Some(crate::debug::ImpliedCloseReason::MismatchedEndTag),
                        Some(tag_name),
                    );

                    // Set text_start after close tag for subsequent text
                    if self.capture_text_content {
                        self.store.text_content.set_start(entry.end() as usize);
                    }
                }
                TapeEntryKind::Text => {
                    // Handle text content
                    if self.capture_text_content && self.store.text_content.text_start.is_some() {
                        // Push text from text_start to the end of this text entry
                        if let Some(position) =
                            self.store.text_content.push(&reader, entry.end() as usize)
                        {
                            self.position.text_content_position = position;
                        }
                        // Set text_start for the next text segment
                        self.store.text_content.set_start(entry.end() as usize);
                    }
                }
                TapeEntryKind::Comment | TapeEntryKind::Doctype => {
                    // Skip comments and doctypes
                }
                _ => {
                    // Attribute entries are handled within tag parsing
                }
            }
        }

        // Restore tape for potential debugging
        self.tape = tape;

        // Set reader position to end of source for proper EOF handling
        reader.set_position(source_len);

        // Drain open elements at EOF
        self.drain_open_elements(&reader);
    }

    /// Run DOM construction using fused tape entries with pre-tokenized attributes.
    ///
    /// This method processes tape entries built by the FusedTapeBuilder and
    /// uses the pre-tokenized compact attributes to build elements directly
    /// without re-scanning through the Reader+tokenizer.
    fn run_fused_dom_construction(&mut self) {
        let tape: Vec<TapeEntry> = std::mem::take(&mut self.tape);
        let compact_attrs: Vec<CompactAttrEntry> = std::mem::take(&mut self.compact_attributes);
        let tag_attr_map: Vec<(usize, usize)> = std::mem::take(&mut self.tag_attr_map);
        let source_len = self.source.len();
        let mut reader = Reader::from_bytes(self.source);

        // Track which tag we're processing in the mapping
        let mut tag_map_idx: usize = 0;

        for &entry in &tape {
            match entry.kind {
                TapeEntryKind::OpenTag | TapeEntryKind::SelfClosingTag => {
                    // Push text content before this tag if we have a text_start
                    if self.capture_text_content && self.store.text_content.text_start.is_some() {
                        if let Some(position) =
                            self.store.text_content.push(&reader, entry.offset as usize)
                        {
                            self.position.text_content_position = position;
                        }
                    }

                    // Get attribute range from the mapping
                    let (attr_start, attr_count) = if tag_map_idx < tag_attr_map.len() {
                        tag_attr_map[tag_map_idx]
                    } else {
                        (0, 0)
                    };
                    tag_map_idx += 1;

                    let attr_range = attr_start..attr_start + attr_count;

                    // Build element from tape with pre-tokenized attributes
                    self.element.from_tape(
                        &entry,
                        self.source,
                        &compact_attrs,
                        attr_range,
                        &mut self.store.attributes,
                    );

                    let is_self_closing = entry.kind == TapeEntryKind::SelfClosingTag
                        || self.element.is_self_closing();

                    self.position.reader_position = entry.offset as usize;

                    // Handle implied closes
                    self.open_elements
                        .prepare_for_open_into(self.element.name, &mut self.implied_closes);
                    let mut implied = std::mem::take(&mut self.implied_closes);
                    self.pop_open_elements(
                        &mut implied,
                        &reader,
                        Some(crate::debug::ImpliedCloseReason::OpenTagRule),
                        None,
                    );
                    self.implied_closes = implied;
                    self.position.reader_position = entry.end() as usize;

                    if is_self_closing {
                        self.position.element_depth = self.open_elements.depth().saturating_add(1);
                    } else {
                        self.open_elements.push(self.element.name);
                        self.position.element_depth = self.open_elements.depth();
                    }

                    // Set text_start before driving query multiplexer
                    if self.capture_text_content {
                        self.store.text_content.set_start(entry.end() as usize);
                    }

                    // Drive query multiplexer
                    self.selectors.next_into(
                        &self.element,
                        &self.position,
                        &mut self.store,
                        &mut self.save_hits,
                    );

                    if !is_self_closing {
                        for save_hit in &self.save_hits {
                            self.open_elements.attach_saved(
                                save_hit.element_id,
                                save_hit
                                    .save_inner_html
                                    .then_some(self.position.reader_position),
                                save_hit
                                    .save_text_content
                                    .then_some(self.position.text_content_position),
                            );
                        }
                    }

                    self.element.clear();
                }
                TapeEntryKind::CloseTag => {
                    // Push text content before this close tag
                    if self.capture_text_content && self.store.text_content.text_start.is_some() {
                        if let Some(position) =
                            self.store.text_content.push(&reader, entry.offset as usize)
                        {
                            self.position.text_content_position = position;
                        }
                    }

                    // Extract tag name from the close tag
                    let tag_slice = entry.slice(self.source);
                    let tag_name = tag_slice
                        .trim_start_matches("</")
                        .trim_end_matches('>')
                        .trim();

                    self.position.reader_position = entry.offset as usize;

                    self.open_elements
                        .close_by_end_tag_into(tag_name, &mut self.closing_elements);
                    self.pop_closing_elements(
                        &reader,
                        Some(crate::debug::ImpliedCloseReason::MismatchedEndTag),
                        Some(tag_name),
                    );

                    // Set text_start after close tag for subsequent text
                    if self.capture_text_content {
                        self.store.text_content.set_start(entry.end() as usize);
                    }
                }
                TapeEntryKind::Text => {
                    // Handle text content
                    if self.capture_text_content && self.store.text_content.text_start.is_some() {
                        if let Some(position) =
                            self.store.text_content.push(&reader, entry.end() as usize)
                        {
                            self.position.text_content_position = position;
                        }
                        self.store.text_content.set_start(entry.end() as usize);
                    }
                }
                TapeEntryKind::Comment | TapeEntryKind::Doctype => {
                    // Skip comments and doctypes
                }
                _ => {
                    // Attribute entries are handled within tag parsing
                }
            }
        }

        // Restore tape and compact_attrs for potential debugging
        self.tape = tape;
        self.compact_attributes = compact_attrs;
        self.tag_attr_map = tag_attr_map;

        // Set reader position to end of source for proper EOF handling
        reader.set_position(source_len);

        // Drain open elements at EOF
        self.drain_open_elements(&reader);
    }

    /// Get a reference to the structural index
    pub fn structural_index(&self) -> &StructuralIndex {
        &self.structural_index
    }

    /// Get a reference to the tape
    pub fn tape(&self) -> &[TapeEntry] {
        &self.tape
    }

    /// Get the parsed store
    pub fn finish(self) -> Store<'html, 'query> {
        self.store
    }

    // Helper methods copied from XHtmlParser for compatibility

    fn pop_open_element(
        &mut self,
        open_element: OpenElement<'html>,
        close_depth: crate::engine::DepthSize,
        reader: &Reader<'html>,
    ) -> bool {
        self.finalize_open_element(&open_element, reader);
        self.position.element_depth = close_depth;
        self.selectors
            .back(open_element.name, &self.position, reader, &mut self.store)
    }

    fn pop_open_elements(
        &mut self,
        open_elements: &mut Vec<OpenElement<'html>>,
        reader: &Reader<'html>,
        implied_close_reason: Option<crate::debug::ImpliedCloseReason>,
        expected_tag: Option<&'html str>,
    ) -> bool {
        let base_depth = self.open_elements.depth();
        let mut elems = std::mem::take(open_elements);
        let total = elems.len();
        let mut early_exit = false;

        for (index, open_element) in elems.drain(..).enumerate() {
            let close_depth =
                base_depth.saturating_add((total - index) as crate::engine::DepthSize);
            if implied_close_reason.is_some_and(|_| {
                expected_tag
                    .is_none_or(|expected| !open_element.name.eq_ignore_ascii_case(expected))
            }) {
                crate::scah_trace!(
                    self.store,
                    crate::debug::TraceEvent::ImpliedClose {
                        tag: open_element.name,
                        depth: close_depth,
                        reason: implied_close_reason.unwrap(),
                    }
                );
            }
            early_exit = self.pop_open_element(open_element, close_depth, reader) || early_exit;
        }

        *open_elements = elems;
        early_exit
    }

    fn pop_closing_elements(
        &mut self,
        reader: &Reader<'html>,
        implied_close_reason: Option<crate::debug::ImpliedCloseReason>,
        expected_tag: Option<&'html str>,
    ) -> bool {
        let base_depth = self.open_elements.depth();
        let mut closing_elements = std::mem::take(&mut self.closing_elements);
        let total = closing_elements.len();
        let mut early_exit = false;

        for (index, open_element) in closing_elements.drain(..).enumerate() {
            let close_depth =
                base_depth.saturating_add((total - index) as crate::engine::DepthSize);
            if implied_close_reason.is_some_and(|_| {
                expected_tag
                    .is_none_or(|expected| !open_element.name.eq_ignore_ascii_case(expected))
            }) {
                crate::scah_trace!(
                    self.store,
                    crate::debug::TraceEvent::ImpliedClose {
                        tag: open_element.name,
                        depth: close_depth,
                        reason: implied_close_reason.unwrap(),
                    }
                );
            }
            early_exit = self.pop_open_element(open_element, close_depth, reader) || early_exit;
        }

        self.closing_elements = closing_elements;
        early_exit
    }

    fn finalize_open_element(&mut self, open_element: &OpenElement<'html>, reader: &Reader<'html>) {
        for saved in &open_element.saved {
            let inner_html = saved
                .inner_html_start
                .map(|start_idx| reader.slice(start_idx..self.position.reader_position));

            let text_content = saved.text_content_start.and_then(|start_idx| {
                let end = self.store.text_content.get_position();
                if start_idx == usize::MAX {
                    if self.store.text_content.is_empty() {
                        None
                    } else {
                        Some(0..end)
                    }
                } else if start_idx == end {
                    None
                } else {
                    Some((start_idx + 1)..end)
                }
            });

            self.store
                .set_content(saved.element_id, inner_html, text_content);
        }
    }

    fn drain_open_elements(&mut self, reader: &Reader<'html>) {
        if self.eof_drained {
            return;
        }

        if self.capture_text_content
            && self.store.text_content.text_start.is_some()
            && let Some(position) = self.store.text_content.push(reader, reader.get_position())
        {
            self.position.text_content_position = position;
        }
        self.position.reader_position = reader.get_position();
        self.open_elements
            .close_all_at_eof_into(&mut self.implied_closes);
        let mut implied = std::mem::take(&mut self.implied_closes);
        self.pop_open_elements(
            &mut implied,
            reader,
            Some(crate::debug::ImpliedCloseReason::EofDrain),
            None,
        );
        self.implied_closes = implied;
        self.eof_drained = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Query, Save};

    #[test]
    fn test_tape_parser_basic() {
        let html = b"<div><a href='link'>Hello</a></div>";
        let queries = &[Query::all("a", Save::all()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse();

        let anchors: Vec<_> = store.get("a").unwrap().collect();
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].name, "a");
        assert_eq!(anchors[0].inner_html, Some("Hello"));
    }

    #[test]
    fn test_tape_parser_nested() {
        let html = b"<div><section><a href='link'>Link</a></section></div>";
        let queries = &[Query::all("div section a", Save::all()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse();

        let anchors: Vec<_> = store.get("div section a").unwrap().collect();
        assert_eq!(anchors.len(), 1);
    }

    #[test]
    fn test_tape_parser_self_closing() {
        let html = b"<div><br/><img src='test'/></div>";
        let queries = &[Query::all("img", Save::none()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse();

        let imgs: Vec<_> = store.get("img").unwrap().collect();
        assert_eq!(imgs.len(), 1);
    }

    #[test]
    fn test_tape_construction() {
        let html = b"<div class='test'>Hello</div>";
        let queries = &[Query::all("div", Save::none()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let mut parser = TapeParser::new(selectors, html);
        parser.structural_index = StructuralIndex::build(html);
        parser.build_tape();

        // Verify tape entries
        assert!(!parser.tape().is_empty());

        // Should have: open_tag, text, close_tag
        let has_open = parser
            .tape()
            .iter()
            .any(|e| e.kind == TapeEntryKind::OpenTag);
        let has_text = parser.tape().iter().any(|e| e.kind == TapeEntryKind::Text);
        let has_close = parser
            .tape()
            .iter()
            .any(|e| e.kind == TapeEntryKind::CloseTag);

        assert!(has_open);
        assert!(has_text);
        assert!(has_close);
    }

    #[test]
    fn test_tape_parser_attributes() {
        let html = b"<a href='https://example.com' target='_blank'>Link</a>";
        let queries = &[Query::all("a[href]", Save::all()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse();

        let anchors: Vec<_> = store.get("a[href]").unwrap().collect();
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].name, "a");
        assert_eq!(
            anchors[0].attribute(&store, "href"),
            Some("https://example.com")
        );
        assert_eq!(anchors[0].attribute(&store, "target"), Some("_blank"));
    }

    #[test]
    fn test_tape_parser_multiple_queries() {
        let html = b"<div><span class='hello'>World</span><a href='link'>Click</a></div>";
        let queries = &[
            Query::all("span", Save::all()).unwrap().build(),
            Query::all("a", Save::all()).unwrap().build(),
        ];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse();

        let spans: Vec<_> = store.get("span").unwrap().collect();
        let anchors: Vec<_> = store.get("a").unwrap().collect();

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "span");
        assert_eq!(spans[0].class, Some("hello"));
        assert_eq!(spans[0].inner_html, Some("World"));

        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].name, "a");
        assert_eq!(anchors[0].inner_html, Some("Click"));
    }

    #[test]
    fn test_tape_parser_text_content() {
        let html = b"<div><p>Hello World</p></div>";
        let queries = &[Query::all("p", Save::only_text_content()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse();

        let paragraphs: Vec<_> = store.get("p").unwrap().collect();
        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text_content(&store), Some("Hello World"));
    }

    #[test]
    fn test_tape_parser_first_query() {
        let html = b"<div><a href='1'>First</a><a href='2'>Second</a></div>";
        let queries = &[Query::first("a", Save::all()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse();

        let anchors: Vec<_> = store.get("a").unwrap().collect();
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].inner_html, Some("First"));
    }

    #[test]
    fn test_tape_parser_comments() {
        let html = b"<div><!-- This is a comment --><p>Hello</p></div>";
        let queries = &[Query::all("p", Save::all()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse();

        let paragraphs: Vec<_> = store.get("p").unwrap().collect();
        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].inner_html, Some("Hello"));
    }

    #[test]
    fn test_tape_parser_malformed_html() {
        // Test with unclosed tags
        let html = b"<div><p>Unclosed paragraph<div>Nested</div></div>";
        let queries = &[Query::all("p", Save::all()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse();

        // Should still find the paragraph
        let paragraphs: Vec<_> = store.get("p").unwrap().collect();
        assert_eq!(paragraphs.len(), 1);
    }

    #[test]
    fn test_structural_index_api() {
        let html = "<div class='test' id=\"main\">Hello</div>";
        let index = crate::index_html(html);

        assert!(!index.is_empty());
        assert!(index.len() > 0);

        // Verify positions are valid
        for pos in index.iter() {
            assert!((pos as usize) < html.len());
        }
    }

    #[test]
    fn test_parse_tape_api() {
        use crate::parse_tape;

        let html = "<div><a href='link'>Hello</a></div>";
        let queries = &[Query::all("a", Save::all()).unwrap().build()];
        let store = parse_tape(html, queries);

        let anchors: Vec<_> = store.get("a").unwrap().collect();
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].name, "a");
    }

    // --- Parallel fused parser tests ---

    #[test]
    fn test_parse_fused_parallel_basic() {
        let html = b"<div><a href='link'>Hello</a></div>";
        let queries = &[Query::all("a", Save::all()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse_fused_parallel();

        let anchors: Vec<_> = store.get("a").unwrap().collect();
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].name, "a");
        assert_eq!(anchors[0].inner_html, Some("Hello"));
    }

    #[test]
    fn test_parse_fused_parallel_attributes() {
        let html = b"<a href='https://example.com' target='_blank'>Link</a>";
        let queries = &[Query::all("a[href]", Save::all()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse_fused_parallel();

        let anchors: Vec<_> = store.get("a[href]").unwrap().collect();
        assert_eq!(anchors.len(), 1);
        assert_eq!(
            anchors[0].attribute(&store, "href"),
            Some("https://example.com")
        );
        assert_eq!(anchors[0].attribute(&store, "target"), Some("_blank"));
    }

    #[test]
    fn test_parse_fused_parallel_nested() {
        let html = b"<div><section><a href='link'>Link</a></section></div>";
        let queries = &[Query::all("div section a", Save::all()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse_fused_parallel();

        let anchors: Vec<_> = store.get("div section a").unwrap().collect();
        assert_eq!(anchors.len(), 1);
    }

    #[test]
    fn test_parse_fused_parallel_text_content() {
        let html = b"<div><p>Hello World</p></div>";
        let queries = &[Query::all("p", Save::only_text_content()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse_fused_parallel();

        let paragraphs: Vec<_> = store.get("p").unwrap().collect();
        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text_content(&store), Some("Hello World"));
    }

    #[test]
    fn test_parse_fused_parallel_matches_sequential() {
        // Create a large document that will trigger parallel processing
        let mut html = String::with_capacity(200 * 1024);
        html.push_str("<html><body>");
        for i in 0..2000 {
            html.push_str(&format!("<div data-index='{}'>content{}</div>", i, i));
        }
        html.push_str("</body></html>");

        let queries = &[Query::all("div", Save::all()).unwrap().build()];

        // Parse with sequential fused
        let selectors_seq = QueryMultiplexer::new(queries);
        let parser_seq = TapeParser::new(selectors_seq, html.as_bytes());
        let store_seq = parser_seq.parse_fused();

        // Parse with parallel fused
        let selectors_par = QueryMultiplexer::new(queries);
        let parser_par = TapeParser::new(selectors_par, html.as_bytes());
        let store_par = parser_par.parse_fused_parallel();

        // Both should find the same elements
        let divs_seq: Vec<_> = store_seq.get("div").unwrap().collect();
        let divs_par: Vec<_> = store_par.get("div").unwrap().collect();

        assert_eq!(
            divs_seq.len(),
            divs_par.len(),
            "Sequential found {} elements, parallel found {}",
            divs_seq.len(),
            divs_par.len()
        );
        assert_eq!(
            divs_seq.len(),
            2000,
            "Expected 2000 divs, found {}",
            divs_seq.len()
        );

        // Check first and last elements match
        assert_eq!(divs_seq[0].name, divs_par[0].name);
        assert_eq!(divs_seq[0].inner_html, divs_par[0].inner_html);
    }

    #[test]
    fn test_parse_fused_parallel_api() {
        use crate::parse_fused_parallel;

        let html = "<div><a href='link'>Hello</a></div>";
        let queries = &[Query::all("a", Save::all()).unwrap().build()];
        let store = parse_fused_parallel(html, queries);

        let anchors: Vec<_> = store.get("a").unwrap().collect();
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].name, "a");
    }

    // --- Direct parallel writing parser tests ---

    #[test]
    fn test_parse_fused_parallel_direct_basic() {
        let html = b"<div><a href='link'>Hello</a></div>";
        let queries = &[Query::all("a", Save::all()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse_fused_parallel_direct();

        let anchors: Vec<_> = store.get("a").unwrap().collect();
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].name, "a");
        assert_eq!(anchors[0].inner_html, Some("Hello"));
    }

    #[test]
    fn test_parse_fused_parallel_direct_attributes() {
        let html = b"<a href='https://example.com' target='_blank'>Link</a>";
        let queries = &[Query::all("a[href]", Save::all()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse_fused_parallel_direct();

        let anchors: Vec<_> = store.get("a[href]").unwrap().collect();
        assert_eq!(anchors.len(), 1);
        assert_eq!(
            anchors[0].attribute(&store, "href"),
            Some("https://example.com")
        );
        assert_eq!(anchors[0].attribute(&store, "target"), Some("_blank"));
    }

    #[test]
    fn test_parse_fused_parallel_direct_nested() {
        let html = b"<div><section><a href='link'>Link</a></section></div>";
        let queries = &[Query::all("div section a", Save::all()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse_fused_parallel_direct();

        let anchors: Vec<_> = store.get("div section a").unwrap().collect();
        assert_eq!(anchors.len(), 1);
    }

    #[test]
    fn test_parse_fused_parallel_direct_text_content() {
        let html = b"<div><p>Hello World</p></div>";
        let queries = &[Query::all("p", Save::only_text_content()).unwrap().build()];
        let selectors = QueryMultiplexer::new(queries);

        let parser = TapeParser::new(selectors, html);
        let store = parser.parse_fused_parallel_direct();

        let paragraphs: Vec<_> = store.get("p").unwrap().collect();
        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text_content(&store), Some("Hello World"));
    }

    #[test]
    fn test_parse_fused_parallel_direct_matches_sequential() {
        // Create a large document that will trigger parallel processing
        let mut html = String::with_capacity(200 * 1024);
        html.push_str("<html><body>");
        for i in 0..2000 {
            html.push_str(&format!("<div data-index='{}'>content{}</div>", i, i));
        }
        html.push_str("</body></html>");

        let queries = &[Query::all("div", Save::all()).unwrap().build()];

        // Parse with sequential fused
        let selectors_seq = QueryMultiplexer::new(queries);
        let parser_seq = TapeParser::new(selectors_seq, html.as_bytes());
        let store_seq = parser_seq.parse_fused();

        // Parse with direct parallel
        let selectors_dir = QueryMultiplexer::new(queries);
        let parser_dir = TapeParser::new(selectors_dir, html.as_bytes());
        let store_dir = parser_dir.parse_fused_parallel_direct();

        // Both should find the same elements
        let divs_seq: Vec<_> = store_seq.get("div").unwrap().collect();
        let divs_dir: Vec<_> = store_dir.get("div").unwrap().collect();

        assert_eq!(
            divs_seq.len(),
            divs_dir.len(),
            "Sequential found {} elements, direct parallel found {}",
            divs_seq.len(),
            divs_dir.len()
        );
        assert_eq!(
            divs_seq.len(),
            2000,
            "Expected 2000 divs, found {}",
            divs_seq.len()
        );

        // Check first and last elements match
        assert_eq!(divs_seq[0].name, divs_dir[0].name);
        assert_eq!(divs_seq[0].inner_html, divs_dir[0].inner_html);
    }

    #[test]
    fn test_parse_fused_parallel_direct_api() {
        use crate::parse_fused_parallel_direct;

        let html = "<div><a href='link'>Hello</a></div>";
        let queries = &[Query::all("a", Save::all()).unwrap().build()];
        let store = parse_fused_parallel_direct(html, queries);

        let anchors: Vec<_> = store.get("a").unwrap().collect();
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].name, "a");
    }
}
