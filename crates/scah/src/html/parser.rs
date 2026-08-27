use super::element::builder::XHtmlTag;
use super::indexer::{AutoTagIndexer, IndexingMode, TagEvent, TagIndexer, TagKind};
use super::open_elements::{OpenElement, OpenElementStack, SavedElement};
use super::tag::{ClassifiedTag, TagFlags, TextTagFlags};
use super::text_edge::TextEdgePolicy;
use super::text_state::{
    ParserTextState, PendingSeparator, TextCaptureMode, TextElementBehavior, TextElementFlags,
};
use crate::Attribute;
use crate::ParseError;
use crate::QuerySpec;
use crate::Reader;
use crate::XHtmlElement;
use crate::debug::ImpliedCloseReason;
#[cfg(any(debug_assertions, test))]
use crate::debug::TraceEvent;
use crate::engine::MAX_ELEMENT_DEPTH;
use crate::engine::multiplexer::{
    DocumentPosition, ElementPreflight, QueryMultiplexer, SaveHit, SiblingCallback,
};
use crate::store::{Store, trim_collapsed_range};

#[derive(Default)]
struct ParserTempState<'html, 'query> {
    closing_elements: Vec<OpenElement<'html>>,
    implied_closes: Vec<OpenElement<'html>>,
    saved_elements: Vec<SavedElement>,
    attributes: Vec<Attribute<'html>>,
    attribute_start: usize,
    save_hits: Vec<SaveHit>,
    preflight: ElementPreflight<'query>,

    sibling: Option<Box<SiblingParserState>>,
}

#[derive(Default)]
struct SiblingParserState {
    // Reused scratch output from the current open-tag query dispatch.
    pending: Vec<SiblingCallback>,

    // Persistent storage for callbacks belonging to currently open elements.
    arena: Vec<SiblingCallback>,

    // Callback arena starts aligned with the open-element stack.
    callback_starts: Vec<usize>,
}

impl<'html, 'query> ParserTempState<'html, 'query> {
    fn sibling(&self) -> &SiblingParserState {
        self.sibling
            .as_deref()
            .expect("sibling parser state requires sibling queries")
    }

    fn sibling_mut(&mut self) -> &mut SiblingParserState {
        self.sibling
            .as_deref_mut()
            .expect("sibling parser state requires sibling queries")
    }
}

pub struct XHtmlParser<'html, 'query, Q> {
    position: DocumentPosition,
    pub selectors: QueryMultiplexer<'query, Q>,
    store: Store<'html, 'query>,
    element: crate::XHtmlElement<'html>,
    open_elements: OpenElementStack<'html>,
    temp_state: ParserTempState<'html, 'query>,
    /// Hot-path capture mode (mirrors `text_state.mode` for cheaper checks).
    capture_mode: TextCaptureMode,
    text_state: ParserTextState,
    raw_source_start: Option<usize>,
    raw_active_count: usize,
    text_active_count: usize,
    persist_attributes: bool,
    raw_text_close: Option<&'static str>,
    eof_drained: bool,
    parse_error: Option<ParseError>,
    indexer: AutoTagIndexer,
    #[cfg(test)]
    attribute_parse_count: usize,
    #[cfg(test)]
    selected_attribute_count: usize,
}

#[inline]
fn element_has_hidden(element: &XHtmlElement<'_>, text_state: &mut ParserTextState) -> bool {
    #[cfg(feature = "bench-internals")]
    {
        text_state.path_stats.hidden_attribute_scans += 1;
    }
    #[cfg(not(feature = "bench-internals"))]
    let _ = text_state;
    element
        .attributes
        .iter()
        .any(|attr| attr.key.eq_ignore_ascii_case("hidden"))
}

#[inline]
fn text_behavior_for(
    tag: TextTagFlags,
    has_hidden: bool,
    text_state: &mut ParserTextState,
) -> TextElementBehavior {
    #[cfg(feature = "bench-internals")]
    {
        text_state.path_stats.normalized_behavior_computations += 1;
    }
    #[cfg(not(feature = "bench-internals"))]
    let _ = text_state;
    let suppressed = tag.is_suppressed() || has_hidden;
    let preformatted = tag.is_preformatted();
    let opening_separator = if suppressed {
        PendingSeparator::None
    } else if tag.is_block() || tag.is_row() {
        PendingSeparator::LineBreak
    } else {
        PendingSeparator::None
    };
    TextElementBehavior {
        suppressed,
        preformatted,
        opening_separator,
    }
}

impl<'html, 'query: 'html, Q> XHtmlParser<'html, 'query, Q>
where
    Q: QuerySpec<'query>,
{
    pub fn new(selectors: QueryMultiplexer<'query, Q>) -> Self {
        let indexing_mode = if selectors.allows_early_exit() {
            IndexingMode::Rolling
        } else {
            IndexingMode::FullDocument
        };
        Self::with_indexing_mode(selectors, None, indexing_mode)
    }

    pub(crate) fn with_indexing_mode(
        selectors: QueryMultiplexer<'query, Q>,
        capacity: Option<usize>,
        indexing_mode: IndexingMode,
    ) -> Self {
        let requirements = selectors.text_requirements();
        let text_state = ParserTextState::new(requirements);
        let persist_attributes = selectors.requires_attribute_storage();
        let parse_attributes = selectors.requires_attribute_parsing() || requirements.text;
        let has_sibling_queries = selectors.features().has_sibling_queries;
        let store = capacity.map_or_else(Store::default, |capacity| {
            Store::with_capacity_requirements(
                capacity,
                crate::CapacityOptions {
                    reserve_raw_text: requirements.raw_text,
                    reserve_text: requirements.text,
                    ..crate::CapacityOptions::default()
                },
                persist_attributes,
                false,
            )
        });

        Self {
            position: DocumentPosition {
                element_depth: 0,
                reader_position: 0, // for inner_html
                self_closing: false,
            },
            selectors,
            element: XHtmlElement::default(),
            open_elements: OpenElementStack::default(),
            temp_state: ParserTempState {
                sibling: has_sibling_queries.then(Box::default),
                ..ParserTempState::default()
            },
            capture_mode: text_state.mode,
            text_state,
            raw_source_start: None,
            raw_active_count: 0,
            text_active_count: 0,
            persist_attributes,
            raw_text_close: None,
            eof_drained: false,
            parse_error: None,
            indexer: AutoTagIndexer::new(indexing_mode, parse_attributes),
            #[cfg(test)]
            attribute_parse_count: 0,
            #[cfg(test)]
            selected_attribute_count: 0,
            store,
        }
    }

    pub fn with_capacity(selectors: QueryMultiplexer<'query, Q>, capacity: usize) -> Self {
        let indexing_mode = if selectors.allows_early_exit() {
            IndexingMode::Rolling
        } else {
            IndexingMode::FullDocument
        };
        Self::with_indexing_mode(selectors, Some(capacity), indexing_mode)
    }

    fn flush_source_text(&mut self, reader: &Reader<'html>, end: usize) {
        let raw_start = self.raw_source_start.take();
        let text_start = self.text_state.source_start.take();
        if raw_start.is_none() && text_start.is_none() {
            return;
        }
        #[cfg(feature = "bench-internals")]
        {
            self.text_state.path_stats.flush_calls += 1;
        }
        if let Some(start) = raw_start.filter(|start| *start < end) {
            self.store.text.raw_text.push_str(reader.slice(start..end));
        }
        if let Some(start) = text_start.filter(|start| *start < end) {
            let depth = self.position.element_depth;
            self.text_state.write_normalized_fragment(
                &mut self.store.text.text,
                reader.slice(start..end),
                depth,
            );
        }
    }

    #[inline]
    fn mark_active_source_start(&mut self, position: usize) {
        if self.raw_active_count > 0 {
            self.raw_source_start = Some(position);
        }
        if self.text_active_count > 0 {
            self.text_state.mark_source_start(position);
        }
    }

    pub fn next(&mut self, reader: &mut Reader<'html>) -> bool {
        if self.parse_error.is_some() {
            return false;
        }
        self.indexer.prepare(reader.source());
        let features = self.selectors.features();
        match (
            self.capture_mode.captures_any(),
            features.has_sibling_queries,
            features.has_retiring_runners,
        ) {
            (false, false, false) => self.next_mode::<false, false, false>(reader),
            (false, false, true) => self.next_without_capture_retiring(reader),
            (false, true, false) => self.next_without_capture_with_siblings(reader),
            (false, true, true) => self.next_without_capture_with_siblings_retiring(reader),
            (true, false, false) => self.next_with_capture::<false, false>(reader),
            (true, false, true) => self.next_with_capture::<false, true>(reader),
            (true, true, false) => self.next_with_capture::<true, false>(reader),
            (true, true, true) => self.next_with_capture::<true, true>(reader),
        }
    }

    pub(crate) fn run(&mut self, reader: &mut Reader<'html>) {
        if self.parse_error.is_some() {
            return;
        }
        // A full run keeps one Reader source, so the index policy only needs
        // preparation once. `next` prepares per call because its caller owns
        // the Reader and may step a different source between calls.
        self.indexer.prepare(reader.source());
        let features = self.selectors.features();
        match (
            self.capture_mode.captures_any(),
            features.has_sibling_queries,
            features.has_retiring_runners,
        ) {
            (false, false, false) => while self.next_mode::<false, false, false>(reader) {},
            (false, false, true) => self.run_without_capture_retiring(reader),
            (false, true, false) => self.run_without_capture_with_siblings(reader),
            (false, true, true) => self.run_without_capture_with_siblings_retiring(reader),
            (true, false, false) => self.run_with_capture::<false, false>(reader),
            (true, false, true) => self.run_with_capture::<false, true>(reader),
            (true, true, false) => self.run_with_capture::<true, false>(reader),
            (true, true, true) => self.run_with_capture::<true, true>(reader),
        }
    }

    pub(crate) fn run_without_text_capture(&mut self, reader: &mut Reader<'html>) {
        debug_assert!(!self.capture_mode.captures_any());
        if self.parse_error.is_some() {
            return;
        }
        self.indexer.prepare(reader.source());
        let features = self.selectors.features();
        match (features.has_sibling_queries, features.has_retiring_runners) {
            (false, false) => while self.next_mode::<false, false, false>(reader) {},
            (false, true) => self.run_without_capture_retiring(reader),
            (true, false) => self.run_without_capture_with_siblings(reader),
            (true, true) => self.run_without_capture_with_siblings_retiring(reader),
        }
    }

    #[inline(never)]
    fn next_without_capture_retiring(&mut self, reader: &mut Reader<'html>) -> bool {
        self.next_mode::<false, false, true>(reader)
    }

    #[cold]
    #[inline(never)]
    fn next_without_capture_with_siblings(&mut self, reader: &mut Reader<'html>) -> bool {
        self.next_mode::<false, true, false>(reader)
    }

    #[cold]
    #[inline(never)]
    fn next_without_capture_with_siblings_retiring(&mut self, reader: &mut Reader<'html>) -> bool {
        self.next_mode::<false, true, true>(reader)
    }

    #[inline(never)]
    fn next_with_capture<const SIBLINGS: bool, const RETIREMENT: bool>(
        &mut self,
        reader: &mut Reader<'html>,
    ) -> bool {
        self.next_mode::<true, SIBLINGS, RETIREMENT>(reader)
    }

    #[inline(never)]
    fn run_without_capture_retiring(&mut self, reader: &mut Reader<'html>) {
        while self.next_mode::<false, false, true>(reader) {}
    }

    #[cold]
    #[inline(never)]
    fn run_without_capture_with_siblings(&mut self, reader: &mut Reader<'html>) {
        while self.next_mode::<false, true, false>(reader) {}
    }

    #[cold]
    #[inline(never)]
    fn run_without_capture_with_siblings_retiring(&mut self, reader: &mut Reader<'html>) {
        while self.next_mode::<false, true, true>(reader) {}
    }

    #[inline(never)]
    fn run_with_capture<const SIBLINGS: bool, const RETIREMENT: bool>(
        &mut self,
        reader: &mut Reader<'html>,
    ) {
        while self.next_mode::<true, SIBLINGS, RETIREMENT>(reader) {}
    }

    #[inline(always)]
    fn next_mode<const CAPTURE: bool, const SIBLINGS: bool, const RETIREMENT: bool>(
        &mut self,
        reader: &mut Reader<'html>,
    ) -> bool {
        if let Some(close_tag) = self.raw_text_close {
            let source = reader.source();
            let Some(close_position) =
                self.indexer
                    .find_raw_text_close(source, reader.get_position(), close_tag)
            else {
                reader.advance_to(source.len());
                self.drain_open_elements::<CAPTURE, SIBLINGS, RETIREMENT>(reader);
                return false;
            };
            reader.advance_to(close_position);

            // Consume an appropriate raw end tag here instead of delegating
            // to `XHtmlTag::from`: that parser intentionally keeps text after
            // `/` as part of the closing tag name, which would leave the raw
            // element open for a tolerated form such as `</style ignored>`.
            if CAPTURE && self.capture_mode.captures_any() {
                self.flush_source_text(reader, reader.get_position());
            }
            self.raw_text_close = None;

            self.position.reader_position = reader.get_position();
            reader.next_until(b'>');
            reader.skip();

            let closing_tag = &close_tag[2..];
            if CAPTURE && self.capture_mode.captures_text() {
                self.text_state.cancel_initial_newline();
            }
            let early_exit =
                self.handle_close_tag::<CAPTURE, SIBLINGS, RETIREMENT>(closing_tag, reader);
            if CAPTURE && self.capture_mode.captures_any() {
                self.mark_active_source_start(reader.get_position());
            }
            return !early_exit && !reader.eof();
        }

        let source = reader.source();
        let mut early_exit = false;
        let mut open_tag_flags = None;
        let mut open_text_tag_flags = TextTagFlags::default();
        let tag = loop {
            let Some(span) = self.indexer.next(source, reader.get_position()) else {
                reader.advance_to(source.len());
                self.drain_open_elements::<CAPTURE, SIBLINGS, RETIREMENT>(reader);
                return false;
            };

            match span {
                TagEvent::Complete(span) if span.kind == TagKind::Ignored => {
                    self.position.reader_position = span.start;
                    if CAPTURE && self.capture_mode.captures_any() {
                        self.flush_source_text(reader, self.position.reader_position);
                    }
                    if CAPTURE && self.capture_mode.captures_text() {
                        self.text_state.cancel_initial_newline();
                    }
                    reader.advance_to(span.end);
                    if CAPTURE && self.capture_mode.captures_any() {
                        self.mark_active_source_start(reader.get_position());
                    }
                }
                TagEvent::Open(open) => {
                    self.position.reader_position = open.start;
                    let name = open.name(source);
                    self.element.set_name(name);
                    self.temp_state.attribute_start = self.store.attributes.len();

                    let (tag_flags, text_tag_flags) =
                        if CAPTURE && self.capture_mode.captures_text() {
                            let classified = ClassifiedTag::classify(name);
                            (classified.parser, classified.text)
                        } else {
                            (TagFlags::classify(name), TextTagFlags::default())
                        };
                    if CAPTURE && self.capture_mode.captures_any() {
                        self.flush_source_text(reader, open.start);
                    }
                    if tag_flags.can_trigger_implied_close() {
                        self.open_elements
                            .prepare_for_open_into(tag_flags, &mut self.temp_state.implied_closes);
                        if !self.temp_state.implied_closes.is_empty() {
                            early_exit =
                                self.drain_implied_closes::<CAPTURE, SIBLINGS, RETIREMENT>(
                                    reader,
                                    Some(ImpliedCloseReason::OpenTagRule),
                                    None,
                                    true,
                                ) || early_exit;
                        }
                    }

                    self.selectors.prepare_element::<SIBLINGS, RETIREMENT>(
                        name,
                        &mut self.temp_state.preflight,
                    );
                    if CAPTURE && self.capture_mode.captures_text() {
                        self.temp_state
                            .preflight
                            .attribute_interest
                            .require_attribute("hidden");
                    }
                    let end = if !self.temp_state.preflight.attribute_interest.is_empty() {
                        #[cfg(test)]
                        {
                            self.attribute_parse_count += 1;
                        }
                        let mut attributes = Reader::from_bytes(&source[open.attributes_start..]);
                        if self.persist_attributes {
                            self.element.parse_attributes(
                                &mut attributes,
                                &mut self.store.attributes,
                                &self.temp_state.preflight.attribute_interest,
                            );
                        } else {
                            self.temp_state.attributes.clear();
                            self.element.parse_attributes(
                                &mut attributes,
                                &mut self.temp_state.attributes,
                                &self.temp_state.preflight.attribute_interest,
                            );
                        }
                        #[cfg(test)]
                        {
                            self.selected_attribute_count += self.element.attributes.len();
                        }
                        open.attributes_start + attributes.get_position()
                    } else {
                        self.indexer.finish_open(source, &open)
                    };
                    reader.advance_to(end);
                    open_tag_flags = Some(tag_flags);
                    open_text_tag_flags = text_tag_flags;
                    break XHtmlTag::Open;
                }
                TagEvent::Complete(span) => {
                    debug_assert_eq!(span.kind, TagKind::Close);
                    self.position.reader_position = span.start;
                    let name = span.name(source);
                    reader.advance_to(span.end);
                    break XHtmlTag::Close(name);
                }
            }
        };
        let tag_start_position = self.position.reader_position;

        if CAPTURE && self.capture_mode.captures_any() {
            self.flush_source_text(reader, tag_start_position);
        }
        match tag {
            XHtmlTag::Open => {
                let tag = open_tag_flags.expect("opening tags are classified before preflight");

                if let Some(close_tag) = tag.raw_text_close_tag() {
                    self.raw_text_close = Some(close_tag);
                }

                self.position.reader_position = tag_start_position;
                self.position.reader_position = reader.get_position();

                let is_self_closing = tag.is_void();
                self.position.self_closing = is_self_closing;

                let (text_behavior, text_edge_policy) = if CAPTURE
                    && self.capture_mode.captures_text()
                {
                    let has_hidden = element_has_hidden(&self.element, &mut self.text_state);
                    let behavior =
                        text_behavior_for(open_text_tag_flags, has_hidden, &mut self.text_state);
                    let edge = self
                        .text_state
                        .edge_policy_for_child(behavior, open_text_tag_flags.is_cell());
                    self.text_state.cancel_initial_newline();
                    (Some(behavior), edge)
                } else {
                    (None, TextEdgePolicy::TrimCollapsedSeparators)
                };
                let text_flags = text_behavior
                    .map(|behavior| behavior.stack_flags(open_text_tag_flags))
                    .unwrap_or_else(TextElementFlags::empty);
                if is_self_closing {
                    let depth = self.open_elements.depth().saturating_add(1);
                    if depth > MAX_ELEMENT_DEPTH {
                        self.record_parse_error(ParseError::MaximumDepthExceeded);
                        return false;
                    }
                    self.position.element_depth = depth;
                } else if let Err(err) = self.open_elements.push_classified(
                    self.element.name,
                    tag,
                    self.temp_state.saved_elements.len(),
                    text_flags,
                ) {
                    self.record_parse_error(err);
                    return false;
                } else {
                    if SIBLINGS {
                        let sibling = self.temp_state.sibling_mut();
                        sibling.callback_starts.push(sibling.arena.len());
                    }
                    self.position.element_depth = self.open_elements.depth();
                }

                crate::scah_trace!(
                    self.store,
                    TraceEvent::OpenTag {
                        tag: self.element.name,
                        depth: self.position.element_depth,
                        reader_position: self.position.reader_position,
                        self_closing: is_self_closing,
                    }
                );

                if SIBLINGS {
                    let ParserTempState {
                        save_hits,
                        preflight,
                        sibling,
                        ..
                    } = &mut self.temp_state;
                    let sibling = sibling
                        .as_deref_mut()
                        .expect("sibling parser state requires sibling queries");
                    self.selectors.next_with_siblings_into(
                        &self.element,
                        &self.position,
                        &mut self.store,
                        save_hits,
                        preflight,
                        &mut sibling.pending,
                    );
                } else {
                    self.selectors.next_plain_into(
                        &self.element,
                        &self.position,
                        &mut self.store,
                        &mut self.temp_state.save_hits,
                        &self.temp_state.preflight,
                    );
                }
                if self.persist_attributes {
                    let attributes_saved = match self.temp_state.save_hits.as_slice() {
                        [] => false,
                        [hit] => hit.save_attributes,
                        hits => hits.iter().any(|hit| hit.save_attributes),
                    };
                    if !attributes_saved {
                        self.store
                            .attributes
                            .truncate(self.temp_state.attribute_start);
                    }
                }
                let (text_was_active, new_raw_count, new_text_count) =
                    if CAPTURE && !is_self_closing {
                        (
                            self.text_active_count > 0,
                            self.temp_state
                                .save_hits
                                .iter()
                                .filter(|hit| hit.save_raw_text)
                                .count(),
                            self.temp_state
                                .save_hits
                                .iter()
                                .filter(|hit| hit.save_text)
                                .count(),
                        )
                    } else {
                        (CAPTURE && self.text_active_count > 0, 0, 0)
                    };
                if let Some(behavior) = text_behavior
                    && (text_was_active || new_text_count > 0)
                {
                    if !text_was_active {
                        self.text_state.discard_pending();
                    }
                    self.text_state.before_open_element(
                        &mut self.store.text.text,
                        behavior,
                        is_self_closing,
                    );
                    self.text_state.before_text_range_start(
                        &mut self.store.text.text,
                        behavior,
                        text_edge_policy,
                        open_text_tag_flags.is_cell(),
                    );
                }
                let (raw_start, text_start) = if CAPTURE {
                    (self.store.text.raw_text.len(), self.store.text.text.len())
                } else {
                    (0, 0)
                };
                if is_self_closing {
                    for hit in &self.temp_state.save_hits {
                        if hit.needs_close_finalization() {
                            self.store.set_content(
                                hit.element_id,
                                None,
                                hit.save_raw_text.then_some(raw_start..raw_start),
                                hit.save_text.then_some(text_start..text_start),
                            );
                        }
                    }
                    if let Some(behavior) = text_behavior
                        && text_was_active
                        && !behavior.suppressed
                        && open_text_tag_flags.is_break()
                    {
                        self.text_state.queue_separator(PendingSeparator::LineBreak);
                    }
                    let source_depth = self.position.element_depth;
                    if SIBLINGS {
                        let sibling = self.temp_state.sibling_mut();
                        self.selectors.activate_sibling_callbacks(
                            &sibling.pending,
                            source_depth,
                            &mut self.store,
                        );
                        sibling.pending.clear();
                    }
                    early_exit = self.selectors.back::<RETIREMENT>(
                        self.element.name,
                        &self.position,
                        reader,
                        &mut self.store,
                    ) || early_exit;
                } else {
                    for hit in &self.temp_state.save_hits {
                        if !hit.needs_close_finalization() {
                            continue;
                        }
                        let saved_index = self.temp_state.saved_elements.len();
                        self.temp_state.saved_elements.push(SavedElement::new(
                            hit.element_id,
                            hit.save_inner_html.then_some(self.position.reader_position),
                            hit.save_raw_text.then_some(raw_start),
                            hit.save_text.then_some(text_start),
                            text_edge_policy,
                        ));
                        self.open_elements.attach_saved(saved_index);
                    }
                    if CAPTURE {
                        self.raw_active_count += new_raw_count;
                        self.text_active_count += new_text_count;
                    }
                    if SIBLINGS {
                        let sibling = self.temp_state.sibling_mut();
                        self.open_elements
                            .attach_sibling_callbacks(&mut sibling.pending, &mut sibling.arena);
                    }
                    if let Some(behavior) = text_behavior {
                        self.text_state
                            .after_open_element(behavior, self.position.element_depth);
                    }
                }

                self.element.clear();
                if CAPTURE && self.capture_mode.captures_any() {
                    self.mark_active_source_start(reader.get_position());
                }
            }
            XHtmlTag::Close(closing_tag) => {
                if CAPTURE && self.capture_mode.captures_text() {
                    self.text_state.cancel_initial_newline();
                }
                early_exit = self
                    .handle_close_tag::<CAPTURE, SIBLINGS, RETIREMENT>(closing_tag, reader)
                    || early_exit;
                if CAPTURE && self.capture_mode.captures_any() {
                    self.mark_active_source_start(reader.get_position());
                }
            }
        }

        !early_exit && !reader.eof()
    }

    pub fn matches(self) -> Store<'html, 'query> {
        self.store
    }

    pub fn trace_parse_started(
        &mut self,
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] html_len: usize,
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] query_count: usize,
    ) {
        crate::scah_trace!(
            self.store,
            TraceEvent::ParseStarted {
                html_len,
                query_count,
            }
        );
    }

    pub fn take_parse_error(&mut self) -> Option<ParseError> {
        self.parse_error.take()
    }

    fn record_parse_error(&mut self, err: ParseError) {
        if self.parse_error.is_none() {
            self.parse_error = Some(err);
        }
    }

    pub fn finish(
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_mut))] mut self,
    ) -> Store<'html, 'query> {
        crate::scah_trace!(
            self.store,
            TraceEvent::ParseFinished {
                element_count: self.store.elements.len(),
                query_node_count: self.store.queries.len(),
                attribute_count: self.store.attributes.len(),
                raw_text_len: self.store.text.raw_text.len(),
                text_len: self.store.text.text.len(),
            }
        );
        self.store
    }

    fn pop_open_element<const CAPTURE: bool, const SIBLINGS: bool, const RETIREMENT: bool>(
        &mut self,
        open_element: OpenElement<'html>,
        close_depth: crate::engine::DepthSize,
        reader: &Reader<'html>,
        activate_sibling_callbacks: bool,
    ) -> bool {
        let saved_range = OpenElementStack::saved_range(&open_element);
        debug_assert_eq!(saved_range.end, self.temp_state.saved_elements.len());
        let (closing_raw_count, closing_text_count) = if CAPTURE {
            (
                self.temp_state.saved_elements[saved_range.clone()]
                    .iter()
                    .filter(|saved| saved.raw_text_start().is_some())
                    .count(),
                self.temp_state.saved_elements[saved_range.clone()]
                    .iter()
                    .filter(|saved| saved.text_start().is_some())
                    .count(),
            )
        } else {
            (0, 0)
        };
        self.finalize_open_element(&open_element, reader);
        self.temp_state.saved_elements.truncate(saved_range.start);
        if CAPTURE {
            debug_assert!(closing_raw_count <= self.raw_active_count);
            debug_assert!(closing_text_count <= self.text_active_count);
            self.raw_active_count = self.raw_active_count.saturating_sub(closing_raw_count);
            self.text_active_count = self.text_active_count.saturating_sub(closing_text_count);
        }
        if CAPTURE && self.capture_mode.captures_text() {
            let text_flags = open_element.text_flags();
            self.text_state.after_close_element(text_flags, close_depth);
            if self.text_active_count == 0 {
                self.text_state.discard_pending();
            } else if !text_flags.contains(TextElementFlags::SUPPRESSED) {
                if text_flags.contains(TextElementFlags::CELL) {
                    self.text_state.queue_cell_boundary();
                } else if let Some(separator) = text_flags.post_text_separator() {
                    self.text_state.queue_separator(separator);
                }
            }
        }
        self.position.element_depth = close_depth;
        let early_exit = self.selectors.back::<RETIREMENT>(
            open_element.name,
            &self.position,
            reader,
            &mut self.store,
        );

        if SIBLINGS {
            let callback_start = self
                .temp_state
                .sibling_mut()
                .callback_starts
                .pop()
                .expect("open sibling scope requires a callback range");
            self.finish_sibling_callback_range(
                callback_start,
                close_depth,
                activate_sibling_callbacks,
            );
        }

        early_exit
    }

    fn finish_sibling_callback_range(
        &mut self,
        start: usize,
        source_depth: crate::engine::DepthSize,
        activate: bool,
    ) {
        let end = self.temp_state.sibling().arena.len();

        debug_assert!(start <= end, "invalid sibling callback arena range");

        if activate {
            for index in start..end {
                let callback = self.temp_state.sibling().arena[index];
                self.selectors
                    .activate_sibling_callback(callback, source_depth, &mut self.store);
            }
        }

        self.temp_state.sibling_mut().arena.truncate(start);
    }

    /// Drain the implied-closes vector, finalizing each element, and restore
    /// the vector's capacity for reuse. Returns `true` on early exit.
    fn drain_implied_closes<const CAPTURE: bool, const SIBLINGS: bool, const RETIREMENT: bool>(
        &mut self,
        reader: &Reader<'html>,
        implied_close_reason: Option<ImpliedCloseReason>,
        expected_tag: Option<&'html str>,
        activate_sibling_callbacks: bool,
    ) -> bool {
        let base_depth = self.open_elements.depth();
        let mut elems = std::mem::take(&mut self.temp_state.implied_closes);
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
                    TraceEvent::ImpliedClose {
                        tag: open_element.name,
                        depth: close_depth,
                        reason: implied_close_reason.unwrap(),
                    }
                );
            }
            // Only the final pop in a batch can have later siblings under its parent.
            let parent_survives_batch = activate_sibling_callbacks && index + 1 == total;
            early_exit = self.pop_open_element::<CAPTURE, SIBLINGS, RETIREMENT>(
                open_element,
                close_depth,
                reader,
                parent_survives_batch,
            ) || early_exit;
        }

        self.temp_state.implied_closes = elems;
        early_exit
    }

    /// Apply a close tag: trace, pop from the open-element stack, and run
    /// the close-element path. Returns `true` on early exit.
    fn handle_close_tag<const CAPTURE: bool, const SIBLINGS: bool, const RETIREMENT: bool>(
        &mut self,
        closing_tag: &'html str,
        reader: &Reader<'html>,
    ) -> bool {
        crate::scah_trace!(
            self.store,
            TraceEvent::CloseTag {
                tag: closing_tag,
                depth: self.position.element_depth,
                reader_position: self.position.reader_position,
            }
        );

        // Well-formed markup closes the element on top of the stack. Handling
        // that here duplicates `close_by_end_tag_into`'s fast path on purpose:
        // it keeps the common case off `temp_state.closing_elements` entirely.
        // The outcome is identical, because `pop_closing_elements` suppresses
        // the implied-close trace when the popped name matches `expected_tag`
        // and derives the same `close_depth` for a single popped element.
        if let Some(open_element) = self.open_elements.pop_matching_top(closing_tag) {
            let close_depth = self.open_elements.depth().saturating_add(1);
            return self.pop_open_element::<CAPTURE, SIBLINGS, RETIREMENT>(
                open_element,
                close_depth,
                reader,
                true,
            );
        }

        self.open_elements
            .close_by_end_tag_into(closing_tag, &mut self.temp_state.closing_elements);
        self.pop_closing_elements::<CAPTURE, SIBLINGS, RETIREMENT>(
            reader,
            Some(ImpliedCloseReason::MismatchedEndTag),
            Some(closing_tag),
        )
    }

    fn pop_closing_elements<const CAPTURE: bool, const SIBLINGS: bool, const RETIREMENT: bool>(
        &mut self,
        reader: &Reader<'html>,
        implied_close_reason: Option<ImpliedCloseReason>,
        expected_tag: Option<&'html str>,
    ) -> bool {
        let base_depth = self.open_elements.depth();
        let mut closing_elements = std::mem::take(&mut self.temp_state.closing_elements);
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
                    TraceEvent::ImpliedClose {
                        tag: open_element.name,
                        depth: close_depth,
                        reason: implied_close_reason.unwrap(),
                    }
                );
            }
            let parent_survives_batch = index + 1 == total;
            early_exit = self.pop_open_element::<CAPTURE, SIBLINGS, RETIREMENT>(
                open_element,
                close_depth,
                reader,
                parent_survives_batch,
            ) || early_exit;
        }

        self.temp_state.closing_elements = closing_elements;
        early_exit
    }

    fn finalize_open_element(&mut self, open_element: &OpenElement<'html>, reader: &Reader<'html>) {
        for saved_index in OpenElementStack::saved_range(open_element) {
            let saved = &self.temp_state.saved_elements[saved_index];
            let inner_html = saved
                .inner_html_start()
                .map(|start| reader.slice(start..self.position.reader_position));
            let raw_text = saved
                .raw_text_start()
                .map(|start| start..self.store.text.raw_text.len());
            let text = saved.text_start().map(|start| {
                let range = start..self.store.text.text.len();
                match saved.text_edge_policy() {
                    TextEdgePolicy::TrimCollapsedSeparators => {
                        trim_collapsed_range(&self.store.text.text, range)
                    }
                    TextEdgePolicy::Preserve => range,
                }
            });
            self.store
                .set_content(saved.element_id, inner_html, raw_text, text);
        }
    }

    fn drain_open_elements<const CAPTURE: bool, const SIBLINGS: bool, const RETIREMENT: bool>(
        &mut self,
        reader: &Reader<'html>,
    ) {
        if self.eof_drained {
            return;
        }

        if CAPTURE && self.capture_mode.captures_any() {
            self.flush_source_text(reader, reader.get_position());
        }
        self.position.reader_position = reader.get_position();
        self.open_elements
            .close_all_at_eof_into(&mut self.temp_state.implied_closes);
        self.drain_implied_closes::<CAPTURE, SIBLINGS, RETIREMENT>(
            reader,
            Some(ImpliedCloseReason::EofDrain),
            None,
            false,
        );
        if SIBLINGS {
            debug_assert!(
                self.temp_state.sibling().arena.is_empty(),
                "sibling callback arena leaked callbacks after EOF"
            );
            debug_assert!(
                self.temp_state.sibling().callback_starts.is_empty(),
                "sibling callback range stack leaked entries after EOF"
            );
        }
        self.eof_drained = true;
    }
}
#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use super::*;
    use crate::Attribute;
    use crate::engine::multiplexer::QueryMultiplexer;
    use crate::store::Element;
    use crate::{Query, Reader, Save, parse};
    use pretty_assertions::assert_eq;

    const BASIC_HTML: &str = r#"
        <html>
            <h1>Hello World</h1>
            <p class="indent">
                My name is <span id="name" class="bold">Zachary</span>
            </p>
        </html>
        "#;

    #[test]
    fn sibling_state_is_allocated_only_for_sibling_queries() {
        let plain_queries = &[Query::all("div", Save::none()).unwrap().build()];
        let plain_parser = XHtmlParser::new(QueryMultiplexer::new(plain_queries));
        assert!(plain_parser.temp_state.sibling.is_none());

        let sibling_queries = &[Query::all("div + p", Save::none()).unwrap().build()];
        let sibling_parser = XHtmlParser::new(QueryMultiplexer::new(sibling_queries));
        assert!(sibling_parser.temp_state.sibling.is_some());
    }

    #[test]
    fn sibling_executor_path_matches_plain_path_for_plain_queries() {
        let html = r#"
            <main>
                <article><h1>one</h1><p class="hit">alpha</p></article>
                <article><h1>two</h1><br><p class="hit">beta</p></article>
                <section><p class="hit">malformed</section>
            </main>
        "#;
        let queries = [
            Query::all("article > p.hit", Save::all()).unwrap().build(),
            Query::all("main p", Save::only_text()).unwrap().build(),
        ];

        let mut plain = XHtmlParser::new(QueryMultiplexer::new(&queries));
        let mut plain_reader = Reader::new(html);
        plain.run(&mut plain_reader);

        let mut forced_sibling = XHtmlParser::new(QueryMultiplexer::new(&queries));
        forced_sibling.temp_state.sibling = Some(Box::default());
        let mut sibling_reader = Reader::new(html);
        while forced_sibling.next_mode::<true, true, false>(&mut sibling_reader) {}

        assert_eq!(forced_sibling.finish(), plain.finish());
    }

    #[test]
    fn specialized_sibling_modes_match_parse_results_across_retirement() {
        let html = "<main><h1></h1><p id='one'></p><p id='two'></p></main>";

        let all_queries = &[Query::all("h1 ~ p", Save::none()).unwrap().build()];
        assert!(
            !QueryMultiplexer::new(all_queries)
                .features()
                .has_retiring_runners
        );
        let expected_all = parse(html, all_queries).unwrap();
        let mut all_parser = XHtmlParser::new(QueryMultiplexer::new(all_queries));
        let mut all_reader = Reader::new(html);
        while all_parser.next(&mut all_reader) {}
        assert!(all_parser.selectors.active_set_is_dense());
        let actual_all = all_parser.matches();
        assert_eq!(actual_all.get("h1 ~ p").unwrap().count(), 2);
        assert_eq!(
            actual_all.get("h1 ~ p").unwrap().count(),
            expected_all.get("h1 ~ p").unwrap().count()
        );

        let first_queries = &[Query::first("h1 ~ p", Save::none()).unwrap().build()];
        assert!(
            QueryMultiplexer::new(first_queries)
                .features()
                .has_retiring_runners
        );
        let expected_first = parse(html, first_queries).unwrap();
        let mut first_parser = XHtmlParser::new(QueryMultiplexer::new(first_queries));
        let mut first_reader = Reader::new(html);
        while first_parser.next(&mut first_reader) {}
        let actual_first = first_parser.matches();
        assert_eq!(actual_first.get("h1 ~ p").unwrap().count(), 1);
        assert_eq!(
            actual_first.get("h1 ~ p").unwrap().count(),
            expected_first.get("h1 ~ p").unwrap().count()
        );
    }

    #[test]
    fn test_basic_html() {
        let mut reader = Reader::new(BASIC_HTML);

        let queries = &[Query::all("p.indent > .bold", Save::none())
            .unwrap()
            .build()];

        let manager = QueryMultiplexer::new(queries);

        let mut parser = XHtmlParser::new(manager);

        // STEP 1
        //let mut continue_parser = parser.next(&mut reader);

        println!("{:?}", queries);

        while parser.next(&mut reader) {
            // println!("{:?}", parser.selectors);
        }

        let store = parser.matches();

        println!("{:?}", store);

        assert_eq!(store.get("p.indent > .bold").unwrap().count(), 1);
        let children = store.get("p.indent > .bold").unwrap();

        let children: Vec<&Element> = children.collect();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "span");
        assert_eq!(children[0].id, Some("name"));
        assert_eq!(children[0].class, Some("bold"));
    }

    #[test]
    fn test_text_content() {
        let queries = &[Query::all("p.indent > .bold", Save::only_text())
            .unwrap()
            .build()];
        let store = parse(BASIC_HTML, queries).unwrap();
        let bold = store.get("p.indent > .bold").unwrap().next().unwrap();
        assert_eq!(bold.text(&store), Some("Zachary"));
    }

    #[test]
    fn test_text_content_buffer_is_empty_when_queries_do_not_request_text() {
        let html = "<div><a href='x'>Hello <b>World</b></a></div>";
        let mut reader = Reader::new(html);
        let queries = &[Query::all("a", Save::only_inner_html()).unwrap().build()];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let anchor = store.get("a").unwrap().next().unwrap();
        assert_eq!(anchor.inner_html, Some("Hello <b>World</b>"));
        assert_eq!(anchor.text(&store), None);
        assert!(store.text.text.as_bytes().is_empty());
    }

    #[test]
    fn unmatched_text_query_leaves_both_tapes_empty() {
        let html = "<main>outside &amp; text<div>more text</div></main>";
        let queries = &[Query::all(".missing", Save::all()).unwrap().build()];
        let mut parser = XHtmlParser::new(QueryMultiplexer::new(queries));
        let mut reader = Reader::new(html);

        parser.run(&mut reader);

        assert!(parser.store.text.raw_text.as_bytes().is_empty());
        assert!(parser.store.text.text.as_bytes().is_empty());
        assert_eq!(parser.raw_active_count, 0);
        assert_eq!(parser.text_active_count, 0);
        #[cfg(feature = "bench-internals")]
        {
            assert_eq!(parser.text_state.path_stats.flush_calls, 0);
            assert_eq!(parser.text_state.path_stats.decoded_fragments, 0);
        }
    }

    #[test]
    fn sparse_text_query_captures_only_the_matching_subtree() {
        let html = concat!(
            "<main>outside &amp; text",
            "<div class='hit'>selected <b>&amp; nested</b></div>",
            "<section>trailing &amp; text</section></main>"
        );
        let queries = &[Query::all(".hit", Save::all()).unwrap().build()];
        let store = parse(html, queries).unwrap();
        let hit = store.get(".hit").unwrap().next().unwrap();

        assert_eq!(hit.raw_text(&store), Some("selected &amp; nested"));
        assert_eq!(hit.text(&store), Some("selected & nested"));
        assert_eq!(store.text.raw_text.as_bytes(), b"selected &amp; nested");
        assert_eq!(store.text.text.as_bytes(), b"selected & nested");
    }

    #[test]
    fn test_mixed_queries_keep_text_content_available_when_any_query_requests_it() {
        let html = "<div><a href='x'>Hello <b>World</b></a></div>";
        let mut reader = Reader::new(html);
        let queries = &[
            Query::all("a", Save::only_inner_html()).unwrap().build(),
            Query::all("b", Save::only_text()).unwrap().build(),
        ];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let anchor = store.get("a").unwrap().next().unwrap();
        let bold = store.get("b").unwrap().next().unwrap();
        assert_eq!(anchor.inner_html, Some("Hello <b>World</b>"));
        assert_eq!(anchor.text(&store), None);
        assert_eq!(bold.text(&store), Some("World"));
    }

    #[test]
    fn test_top_level_multi_selection() {
        let mut reader = Reader::new(BASIC_HTML);

        let queries = &[
            Query::all("p.indent > .bold", Save::none())
                .unwrap()
                .build(),
            Query::all(".indent #name", Save::none()).unwrap().build(),
        ];

        let manager = QueryMultiplexer::new(queries);

        let mut parser = XHtmlParser::new(manager);

        // STEP 1
        //let mut continue_parser = parser.next(&mut reader);

        while parser.next(&mut reader) {}
    }

    const MORE_ADVANCED_BASIC_HTML: &str = r#"
        <html>
            <h1>Hello World</h1>
            <main>
                <section>
                    <a href="https://hello.com">Hello</a>
                    <div>
                        <a href="https://world.com">World</a>
                    </div>
                </section>
            </main>

            <main>
                <section>
                    <a href="https://hello2.com">Hello2</a>

                    <div>
                        <a href="https://world2.com">World2</a>
                        <div>
                            <a href="https://world3.com">World3</a>
                        </div>
                    </div>
                </section>
            </main>
        </html>
        "#;

    #[test]
    #[ignore = "Known issue: Duplication of elements is not handled"]
    fn test_multi_selection() {
        let mut reader = Reader::new(MORE_ADVANCED_BASIC_HTML);
        let queries = Query::all("main > section", Save::all())
            .unwrap()
            .then(|section| {
                Ok([
                    section.all("> a[href]", Save::all())?,
                    section.all("div a", Save::all())?,
                ])
            })
            .unwrap();
        let queries = &[queries.build()];
        let manager = QueryMultiplexer::new(queries);

        let mut parser = XHtmlParser::new(manager);

        // STEP 1
        //let mut continue_parser = parser.next(&mut reader);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        println!("{:#?}", store);

        let sections: Vec<&Element> = store.get("main > section").unwrap().collect();
        assert_eq!(sections.len(), 2);

        // Section 1
        let s1 = sections[0];
        assert_eq!(s1.text(&store), Some("Hello World"));

        let s1_div_a: Vec<&Element> = s1.get(&store, "div a").unwrap().collect();
        assert_eq!(s1_div_a.len(), 1);
        assert_eq!(s1_div_a[0].text(&store), Some("World"));
        assert_eq!(
            s1_div_a[0].attributes(&store).unwrap()[0].value,
            Some("https://world.com")
        );

        println!("{:#?}", s1);

        let s1_direct_a: Vec<&Element> = s1.get(&store, "> a[href]").unwrap().collect();
        assert_eq!(s1_direct_a.len(), 1);
        assert_eq!(s1_direct_a[0].text(&store), Some("Hello"));
        assert_eq!(
            s1_direct_a[0].attributes(&store).unwrap()[0].value,
            Some("https://hello.com")
        );

        // Section 2
        let s2 = sections[1];
        assert_eq!(s2.text(&store), Some("Hello2 World2 World3"));

        let s2_div_a: Vec<&Element> = s2.get(&store, "div a").unwrap().collect();
        assert_eq!(s2_div_a.len(), 2, "World3 Element duplicated");
        assert_eq!(s2_div_a[0].text(&store), Some("World2"));
        assert_eq!(s2_div_a[1].text(&store), Some("World3"));

        let s2_direct_a: Vec<&Element> = s2.get(&store, "> a[href]").unwrap().collect();
        assert_eq!(s2_direct_a.len(), 1);
        assert_eq!(s2_direct_a[0].text(&store), Some("Hello2"));
    }

    const BASIC_HTML_WITH_SCRIPT: &str = r#"
        <html>
            <h1>Hello World</h1>

            <script>
                let x = 123132.2;
                let y = "<div>" + "Hello" + "</" + "div>";
            </script>
        </html>
        "#;

    #[test]
    fn test_script_tag_with_html_like_content() {
        let mut reader = Reader::new(BASIC_HTML_WITH_SCRIPT);

        let queries = &[Query::all("div", Save::none()).unwrap().build()];

        let manager = QueryMultiplexer::new(queries);

        let mut parser = XHtmlParser::new(manager);

        // STEP 1
        //let mut continue_parser = parser.next(&mut reader);

        println!("{:?}", queries);

        while parser.next(&mut reader) {
            // println!("{:?}", parser.selectors);
        }

        let store = parser.matches();

        // It should NOT find any div
        if let Some(div_idx) = store.get("div") {
            assert_eq!(div_idx.count(), 0);
        }
    }

    #[test]
    fn full_index_skips_false_events_inside_long_raw_text() {
        let html = format!(
            "<script>{}<span>fake</span></script><span>real</span>",
            "const x = 1;".repeat(2_000)
        );
        let queries = &[Query::all("span", Save::name_only()).unwrap().build()];

        let store = parse(&html, queries).unwrap();
        let matches = store.get("span").unwrap().collect::<Vec<_>>();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].inner_html, None);
    }

    #[test]
    fn full_index_finds_raw_close_after_tag_like_quote_inside_script() {
        let html = format!(
            "<script>{}const s = \"<div data='unterminated\";</script><span>real</span>",
            "x".repeat(20_000)
        );
        let queries = &[Query::all("span", Save::name_only()).unwrap().build()];

        let store = parse(&html, queries).unwrap();

        assert_eq!(store.get("span").unwrap().count(), 1);
    }

    #[test]
    fn full_index_uses_earliest_literal_raw_close() {
        for (open, close) in [("script", "script"), ("style", "style")] {
            let html = format!(
                "<{open}>{}<div data=\"</{close}><span>first</span>\"></{close}><span>second</span>",
                "x".repeat(20_000)
            );
            let queries = &[Query::all("span", Save::name_only()).unwrap().build()];

            let store = parse(&html, queries).unwrap();

            assert_eq!(store.get("span").unwrap().count(), 2, "raw tag {open}");
        }
    }

    const BASIC_HTML_WITH_SELF_CLOSING_TAG: &str = r#"
        <html>
            <h1>Hello World</h1>
            <form action="/my-handling-form-page" method="post">
                <p>
                    <label for="name">Name:</label>
                    <input type="text" id="name" name="user_name" />
                </p>
                <p>
                    <label for="mail">Email:</label>
                    <input type="email" id="mail" name="user_email" />
                </p>
                <p>
                    <label for="msg">Message:</label>
                    <textarea id="msg" name="user_message"></textarea>
                </p>
            </form>
        </html>
        "#;

    #[test]
    fn test_self_closing_tags() {
        let mut reader = Reader::new(BASIC_HTML_WITH_SELF_CLOSING_TAG);
        let queries = &[Query::all("form > p > input", Save::none())
            .unwrap()
            .build()];

        let manager = QueryMultiplexer::new(queries);

        let mut parser = XHtmlParser::new(manager);

        println!("{:?}", queries);

        while parser.next(&mut reader) {}

        let store = parser.matches();

        let inputs: Vec<&Element> = store.get("form > p > input").unwrap().collect();
        assert_eq!(inputs.len(), 2);

        assert_eq!(inputs[0].name, "input");
        assert_eq!(inputs[0].id, Some("name"));
        assert_eq!(inputs[0].attributes(&store).unwrap()[0].key, "type");
        assert_eq!(inputs[0].attributes(&store).unwrap()[0].value, Some("text"));

        assert_eq!(inputs[1].name, "input");
        assert_eq!(inputs[1].id, Some("mail"));
        assert_eq!(inputs[1].attributes(&store).unwrap()[0].key, "type");
        assert_eq!(
            inputs[1].attributes(&store).unwrap()[0].value,
            Some("email")
        );
    }

    #[test]
    fn test_self_closing_tags_with_content_query() {
        let mut reader = Reader::new(BASIC_HTML_WITH_SELF_CLOSING_TAG);

        let queries = &[Query::all("form > p > input", Save::all()).unwrap().build()];

        let manager = QueryMultiplexer::new(queries);

        let mut parser = XHtmlParser::new(manager);

        // STEP 1
        //let mut continue_parser = parser.next(&mut reader);

        println!("{:?}", queries);

        while parser.next(&mut reader) {
            // println!("{:?}", parser.selectors);
        }

        let store = parser.matches();

        let inputs: Vec<&Element> = store.get("form > p > input").unwrap().collect();
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].text(&store), Some(""));
        assert_eq!(inputs[0].inner_html, None);

        assert_eq!(inputs[1].text(&store), Some(""));
        assert_eq!(inputs[1].inner_html, None);
    }

    const BASIC_ANCHOR_LIST: &str = r#"
        <a>Hello 1</a>
        <a>Hello 2</a>
        <a>Hello 3</a>
        "#;

    #[test]
    fn test_anchor_list_selection() {
        let mut reader = Reader::new(BASIC_ANCHOR_LIST);

        let queries = &[Query::all("a", Save::all()).unwrap().build()];

        let manager = QueryMultiplexer::new(queries);

        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();

        let anchors: Vec<&Element> = store.get("a").unwrap().collect();
        assert_eq!(anchors.len(), 3);

        assert_eq!(anchors[0].text(&store), Some("Hello 1"));
        assert_eq!(anchors[1].text(&store), Some("Hello 2"));
        assert_eq!(anchors[2].text(&store), Some("Hello 3"));
    }

    const POSTS: &str = r#"<div class="article"><a href="/post/0"><b>Post</b> &lt;0&gt;</a></div><div class="article"><a href="/post/1"><b>Post</b> &lt;1&gt;</a></div>"#;

    #[test]
    fn test_first_anchor_in_list_selection() {
        let mut reader = Reader::new(POSTS);

        let queries = &[Query::first("div.article a", Save::all()).unwrap().build()];

        let manager = QueryMultiplexer::new(queries);

        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();

        let anchor = store.get("div.article a").unwrap().next().unwrap();

        assert_eq!(anchor.name, "a");
        assert_eq!(anchor.attributes(&store).unwrap()[0].value, Some("/post/0"));
        assert_eq!(anchor.inner_html, Some("<b>Post</b> &lt;0&gt;"));
        assert_eq!(anchor.text(&store), Some("Post <0>"));
    }

    const PYTHON_TEST_HTML: &str = r#"
    <span class="hello" id="world" hello="world">
        Hello <a href="https://www.example.com">World</a>
    </span>
    <p class="example_class" id="example_id" hello="example">
        My <a href="https://www.example.com">Example</a> or <a href="https://www.notexample.com">Not Example</a>
    </p>
    "#;

    #[test]
    fn test_python_test_html() {
        let mut reader = Reader::new(PYTHON_TEST_HTML);

        let queries = &[Query::all("#world", Save::all())
            .unwrap()
            .all("a", Save::all())
            .unwrap()
            .build()];

        // assert_eq!(queries, &[Query {
        //     queries: vec![].into_boxed_slice(),
        //     states: vec![].into_boxed_slice(),
        //     exit_at_section_end: None,
        // }]);

        let manager = QueryMultiplexer::new(queries);

        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();

        assert_eq!(
            store.attributes.deref().clone(),
            vec![
                Attribute {
                    key: "hello",
                    value: Some("world")
                },
                Attribute {
                    key: "href",
                    value: Some("https://www.example.com")
                },
            ]
        );

        let worlds: Vec<&Element> = store.get("#world").unwrap().collect();
        assert_eq!(worlds.len(), 1);

        let span = worlds[0];
        assert_eq!(span.name, "span");
        assert_eq!(span.class, Some("hello"));
        assert_eq!(span.id, Some("world"));
        assert_eq!(
            span.attributes(&store).unwrap(),
            &[Attribute {
                key: "hello",
                value: Some("world")
            },]
        );
        assert_eq!(
            span.inner_html,
            Some(
                r#"
        Hello <a href="https://www.example.com">World</a>
    "#
            )
        );
        assert!(span.text(&store).is_some());

        let anchors: Vec<&Element> = span.get(&store, "a").unwrap().collect();
        assert_eq!(anchors.len(), 1);

        let a = anchors[0];
        assert_eq!(a.name, "a");
        assert_eq!(a.class, None);
        assert_eq!(a.id, None);
        assert_eq!(
            a.attributes(&store).unwrap(),
            &[Attribute {
                key: "href",
                value: Some("https://www.example.com")
            },]
        );
        assert_eq!(a.inner_html, Some("World"));
        assert!(a.text(&store).is_some());
    }

    #[test]
    fn test_first_anchor_tag_from_bench() {
        fn generate_html(count: usize) -> String {
            let mut html = String::with_capacity(count * 100);
            html.push_str("<html><body><div id='content'>");
            for i in 0..count {
                // Added some entities (&lt;) and bold tags (<b>) to make text extraction work harder
                html.push_str(&format!(
                    r#"<div class="article"><a href="/post/{}"><b>Post</b> &lt;{}&gt;</a></div>"#,
                    i, i
                ));
            }
            html.push_str("</div></body></html>");
            html
        }

        let html = generate_html(100);
        let mut reader = Reader::from_bytes(html.as_bytes());

        let query = Query::first("a", Save::all()).unwrap().build();
        assert_eq!(query.exit_at_section_end, Some(crate::QuerySectionId(0)));
        let queries = &[query];

        let manager = QueryMultiplexer::new(queries);

        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();

        let element = store.get("a").unwrap().next().unwrap();

        assert_eq!(
            store.attributes.deref().clone(),
            vec![Attribute {
                key: "href",
                value: Some("/post/0"),
            }]
        );

        assert_eq!(element.inner_html, Some("<b>Post</b> &lt;0&gt;"));
        assert_eq!(element.text(&store), Some("Post <0>"));
    }

    #[test]
    fn test_implicit_p_close_finalizes_content() {
        let html = "<div><p>Hello<div>World</div></div>";
        let mut reader = Reader::new(html);
        let queries = &[Query::all("p", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let p = store.get("p").unwrap().next().unwrap();
        assert_eq!(p.inner_html, Some("Hello"));
        assert_eq!(p.text(&store), Some("Hello"));
    }

    #[test]
    fn implied_close_updates_query_frontier_before_open_tag_preflight() {
        let html = "<p><a>one</a><p><a>two</a>";
        let queries = &[Query::all("p a", Save::name_only()).unwrap().build()];

        let store = parse(html, queries).unwrap();

        assert_eq!(store.get("p a").unwrap().count(), 2);
    }

    #[test]
    fn test_misnested_close_finalizes_bubbled_elements() {
        let html = "<div><span>Hello</div>";
        let mut reader = Reader::new(html);
        let queries = &[
            Query::all("div", Save::all()).unwrap().build(),
            Query::all("span", Save::all()).unwrap().build(),
        ];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let div = store.get("div").unwrap().next().unwrap();
        let span = store.get("span").unwrap().next().unwrap();

        assert_eq!(span.inner_html, Some("Hello"));
        assert_eq!(span.text(&store), Some("Hello"));
        assert_eq!(div.inner_html, Some("<span>Hello"));
        assert_eq!(div.text(&store), Some("Hello"));
    }

    #[test]
    fn test_stray_close_tag_is_ignored() {
        let html = "<div><span>Hello</bogus></span></div>";
        let mut reader = Reader::new(html);
        let queries = &[Query::all("div span", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let span = store.get("div span").unwrap().next().unwrap();
        assert_eq!(span.text(&store), Some("Hello"));
        assert_eq!(span.inner_html, Some("Hello</bogus>"));
    }

    #[test]
    fn test_eof_drain_finalizes_open_elements() {
        let html = "<section><a href='x'>Link";
        let mut reader = Reader::new(html);
        let queries = &[
            Query::all("section", Save::all()).unwrap().build(),
            Query::all("a", Save::all()).unwrap().build(),
        ];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let section = store.get("section").unwrap().next().unwrap();
        let a = store.get("a").unwrap().next().unwrap();

        assert_eq!(a.inner_html, Some("Link"));
        assert_eq!(a.text(&store), Some("Link"));
        assert_eq!(section.inner_html, Some("<a href='x'>Link"));
        assert_eq!(section.text(&store), Some("Link"));
    }

    #[test]
    fn test_li_auto_close_on_next_li() {
        let html = "<ul><li>One<li>Two</ul>";
        let mut reader = Reader::new(html);
        let queries = &[Query::all("li", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let items: Vec<&Element> = store.get("li").unwrap().collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text(&store), Some("One"));
        assert_eq!(items[0].inner_html, Some("One"));
        assert_eq!(items[1].text(&store), Some("Two"));
        assert_eq!(items[1].inner_html, Some("Two"));
    }

    #[test]
    fn implied_close_preflight_matches_reactivated_attribute_cursor() {
        let html = "<ul><li data-x='one'>One<li data-x='two'>Two</ul>";
        let queries = &[Query::all("li[data-x]", Save::name_only()).unwrap().build()];

        let store = parse(html, queries).unwrap();
        assert_eq!(store.get("li[data-x]").unwrap().count(), 2);
    }

    #[test]
    fn test_dt_dd_auto_close_sequence() {
        let html = "<dl><dt>Term<dd>Def<dt>Next</dl>";
        let mut reader = Reader::new(html);
        let queries = &[
            Query::all("dt", Save::all()).unwrap().build(),
            Query::all("dd", Save::all()).unwrap().build(),
        ];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let dts: Vec<&Element> = store.get("dt").unwrap().collect();
        let dds: Vec<&Element> = store.get("dd").unwrap().collect();

        assert_eq!(dts.len(), 2);
        assert_eq!(dds.len(), 1);
        assert_eq!(dts[0].text(&store), Some("Term"));
        assert_eq!(dds[0].text(&store), Some("Def"));
        assert_eq!(dts[1].text(&store), Some("Next"));
    }

    #[test]
    fn test_option_auto_close_on_next_option() {
        let html = "<select><option>One<option>Two</select>";
        let mut reader = Reader::new(html);
        let queries = &[Query::all("option", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let options: Vec<&Element> = store.get("option").unwrap().collect();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].text(&store), Some("One"));
        assert_eq!(options[1].text(&store), Some("Two"));
    }

    #[test]
    fn test_optgroup_closes_previous_option_and_optgroup() {
        let html = "<select><optgroup><option>One<optgroup><option>Two</select>";
        let mut reader = Reader::new(html);
        let queries = &[
            Query::all("optgroup", Save::all()).unwrap().build(),
            Query::all("option", Save::all()).unwrap().build(),
        ];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let optgroups: Vec<&Element> = store.get("optgroup").unwrap().collect();
        let options: Vec<&Element> = store.get("option").unwrap().collect();

        assert_eq!(optgroups.len(), 2);
        assert_eq!(options.len(), 2);
        assert_eq!(optgroups[0].text(&store), Some("One"));
        assert_eq!(optgroups[1].text(&store), Some("Two"));
        assert_eq!(options[0].text(&store), Some("One"));
        assert_eq!(options[1].text(&store), Some("Two"));
    }

    #[test]
    fn test_td_auto_close_on_next_td() {
        let html = "<table><tr><td>One<td>Two</tr></table>";
        let mut reader = Reader::new(html);
        let queries = &[Query::all("td", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let cells: Vec<&Element> = store.get("td").unwrap().collect();
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].text(&store), Some("One"));
        assert_eq!(cells[1].text(&store), Some("Two"));
    }

    #[test]
    fn test_multiple_queries_attach_to_same_open_element() {
        let html = "<div class='x'>Hello</div>";
        let mut reader = Reader::new(html);
        let queries = &[
            Query::all("div", Save::all()).unwrap().build(),
            Query::all(".x", Save::all()).unwrap().build(),
        ];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let div = store.get("div").unwrap().next().unwrap();
        let class_match = store.get(".x").unwrap().next().unwrap();

        assert_eq!(div.inner_html, Some("Hello"));
        assert_eq!(div.text(&store), Some("Hello"));
        assert_eq!(class_match.inner_html, Some("Hello"));
        assert_eq!(class_match.text(&store), Some("Hello"));
    }

    #[test]
    fn test_descendant_and_child_queries_remain_stable_on_malformed_html() {
        let html = "<div><span>One</div><div><span>Two</span></div>";
        let mut reader = Reader::new(html);
        let queries = &[
            Query::all("div span", Save::all()).unwrap().build(),
            Query::all("div > span", Save::all()).unwrap().build(),
        ];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        assert_eq!(store.get("div span").unwrap().count(), 2);
        assert_eq!(store.get("div > span").unwrap().count(), 2);
    }

    #[test]
    fn attribute_query_ignores_incomplete_open_delimiters_at_eof() {
        let queries = &[Query::all("[data-x]", Save::all()).unwrap().build()];

        for html in ["<", "hello <", "<<<", "<div data-x>text<"] {
            let store = parse(html, queries).unwrap();
            let expected = usize::from(html.starts_with("<div "));
            assert_eq!(
                store.get("[data-x]").map_or(0, Iterator::count),
                expected,
                "{html:?}"
            );
        }
    }

    #[test]
    fn comment_does_not_drop_following_text_content() {
        let queries = &[Query::all("div", Save::only_text()).unwrap().build()];
        let store = parse("<div>abc<!--c-->def</div>", queries).unwrap();
        let div = store.get("div").unwrap().next().unwrap();

        assert_eq!(div.text(&store), Some("abcdef"));
    }

    #[test]
    fn full_index_trailing_bare_delimiter_still_drains_open_elements() {
        let unit = format!("{}<span></span>", "x".repeat(900));
        let html = format!("<div>{}<", unit.repeat(200));
        let queries = &[Query::all("div", Save::all()).unwrap().build()];

        let store = parse(&html, queries).unwrap();
        let div = store.get("div").unwrap().next().unwrap();

        assert_eq!(div.inner_html, Some(&html["<div>".len()..]));
    }

    #[test]
    fn full_index_ignores_a_trailing_bare_open_delimiter() {
        let unit = format!("<p data-x='1'>a</p>{}", "y".repeat(400));
        let html = format!("{}<", unit.repeat(512));
        let queries = &[Query::all("[data-x]", Save::name_only()).unwrap().build()];

        let store = parse(&html, queries).unwrap();

        assert_eq!(store.get("[data-x]").unwrap().count(), 512);
    }

    #[test]
    fn test_text_before_first_tag_does_not_break_text_content() {
        let html = "intro<div>Hello</div>";
        let mut reader = Reader::new(html);
        let queries = &[Query::all("div", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let div = store.get("div").unwrap().next().unwrap();
        assert_eq!(div.text(&store), Some("Hello"));
    }

    #[test]
    fn save_none_does_not_accumulate_text_content() {
        let queries = &[Query::all("a", Save::none()).unwrap().build()];
        let store = parse("<div><a>Hello <b>World</b></a></div>", queries).unwrap();

        let anchor = store.get("a").unwrap().next().unwrap();
        assert_eq!(anchor.text(&store), None);
        assert_eq!(store.text.text.len(), 0);
    }

    #[test]
    fn mixed_save_queries_keep_text_content_for_text_query() {
        let queries = &[
            Query::all("a", Save::none()).unwrap().build(),
            Query::all("b", Save::only_text()).unwrap().build(),
        ];
        let store = parse("<a>Hello <b>World</b></a>", queries).unwrap();

        let anchor = store.get("a").unwrap().next().unwrap();
        let bold = store.get("b").unwrap().next().unwrap();
        assert_eq!(anchor.text(&store), None);
        assert_eq!(bold.text(&store), Some("World"));
    }

    const SINGLE_PRODUCT_HTML: &str = r#"
    <section id="products">
        <div class="product">
            <h1>Product #1</h1>
            <img src="https://example.com/p1.png"/>
            <p>
                Hello World for Product #1
            </p>
        </div>
    </section>
    "#;

    #[test]
    fn test_single_product_listing_html() {
        let mut reader = Reader::new(SINGLE_PRODUCT_HTML);

        let queries = &[Query::all("#products", Save::all())
            .unwrap()
            .all(".product", Save::all())
            .unwrap()
            .then(|p| {
                Ok([
                    p.first("h1", Save::all())?,
                    p.first("img", Save::none())?,
                    p.first("p", Save::all())?,
                ])
            })
            .unwrap()
            .build()];

        let manager = QueryMultiplexer::new(queries);

        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();

        println!("Store: {:#?}", store);

        assert_eq!(store.elements.len(), 5);

        assert_eq!(
            store.attributes.deref().clone(),
            vec![Attribute {
                key: "src",
                value: Some("https://example.com/p1.png")
            }]
        );

        let products_sections: Vec<&Element> = store.get("#products").unwrap().collect();
        assert_eq!(products_sections.len(), 1);

        let section = products_sections[0];
        assert_eq!(section.name, "section");
        assert_eq!(section.id, Some("products"));
        assert!(section.inner_html.is_some());
        assert!(section.text(&store).is_some());

        let products: Vec<&Element> = section.get(&store, ".product").unwrap().collect();
        assert_eq!(products.len(), 1);

        let product = products[0];
        assert_eq!(product.name, "div");
        assert_eq!(product.class, Some("product"));
        assert!(product.inner_html.is_some());
        assert!(product.text(&store).is_some());

        let h1 = product.get(&store, "h1").unwrap().next().unwrap();
        assert_eq!(h1.name, "h1");
        assert_eq!(h1.inner_html, Some("Product #1"));
        assert!(h1.text(&store).is_some());

        let img = product.get(&store, "img").unwrap().next().unwrap();
        assert_eq!(img.name, "img");
        assert!(img.attributes(&store).is_some());

        let p = product.get(&store, "p").unwrap().next().unwrap();
        assert_eq!(p.name, "p");
        assert!(p.inner_html.is_some());
        assert!(p.text(&store).is_some());
    }

    const PRODUCT_HTML: &str = r#"
    <section id="products">
        <div class="product">
            <h1>Product #1</h1>
            <img src="https://example.com/p1.png"/>
            <p>
                Hello World for Product #1
            </p>
        </div>
        
        <div class="product">
            <h1>Product #2</h1>
            <img src="https://example.com/p2.png"/>
            <p>
                Hello World for Product #2
            </p>
        </div>
    </section>
    "#;

    #[test]
    fn test_product_listing_html() {
        let mut reader = Reader::new(PRODUCT_HTML);

        let queries = &[Query::all("#products", Save::all())
            .unwrap()
            .all(".product", Save::all())
            .unwrap()
            .then(|p| {
                Ok([
                    p.first("h1", Save::all())?,
                    p.first("img", Save::none())?,
                    p.first("p", Save::all())?,
                ])
            })
            .unwrap()
            .build()];

        let manager = QueryMultiplexer::new(queries);

        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();

        println!("Store: {:#?}", store);

        assert_eq!(store.elements.len(), 9);

        assert_eq!(
            store.attributes.deref().clone(),
            vec![
                Attribute {
                    key: "src",
                    value: Some("https://example.com/p1.png")
                },
                Attribute {
                    key: "src",
                    value: Some("https://example.com/p2.png")
                },
            ]
        );

        let products_sections: Vec<&Element> = store.get("#products").unwrap().collect();
        assert_eq!(products_sections.len(), 1);

        let section = products_sections[0];
        assert_eq!(section.name, "section");
        assert_eq!(section.id, Some("products"));
        assert!(section.inner_html.is_some());
        assert!(section.text(&store).is_some());

        let products: Vec<&Element> = section.get(&store, ".product").unwrap().collect();
        assert_eq!(products.len(), 2);

        // Product 1
        let p1 = products[0];
        assert_eq!(p1.name, "div");
        assert_eq!(p1.class, Some("product"));
        assert!(p1.inner_html.is_some());
        assert!(p1.text(&store).is_some());

        let p1_h1 = p1.get(&store, "h1").unwrap().next().unwrap();
        assert_eq!(p1_h1.name, "h1");
        assert_eq!(p1_h1.inner_html, Some("Product #1"));
        assert!(p1_h1.text(&store).is_some());

        let p1_img = p1.get(&store, "img").unwrap().next().unwrap();
        assert_eq!(p1_img.name, "img");
        assert!(p1_img.attributes(&store).is_some());

        let p1_p = p1.get(&store, "p").unwrap().next().unwrap();
        assert_eq!(p1_p.name, "p");
        assert!(p1_p.inner_html.is_some());
        assert!(p1_p.text(&store).is_some());

        // Product 2
        let p2 = products[1];
        assert_eq!(p2.name, "div");
        assert_eq!(p2.class, Some("product"));
        assert!(p2.inner_html.is_some());
        assert!(p2.text(&store).is_some());

        let p2_h1 = p2.get(&store, "h1").unwrap().next().unwrap();
        assert_eq!(p2_h1.name, "h1");
        assert!(p2_h1.inner_html.is_some());
        assert!(p2_h1.text(&store).is_some());

        let p2_img = p2.get(&store, "img").unwrap().next().unwrap();
        assert_eq!(p2_img.name, "img");
        assert!(p2_img.attributes(&store).is_some());

        let p2_p = p2.get(&store, "p").unwrap().next().unwrap();
        assert_eq!(p2_p.name, "p");
        assert!(p2_p.inner_html.is_some());
        assert!(p2_p.text(&store).is_some());
    }

    // --- parse() Result tests ---

    #[test]
    fn empty_query_list_returns_error() {
        let html = "<main><a href='x'>x</a></main>";
        let queries: Vec<Query> = Vec::new();

        let result = parse(html, &queries);

        assert!(matches!(result, Err(crate::ParseError::EmptyQueries)));
    }

    #[test]
    fn non_empty_query_list_still_parses() {
        let html = "<main><a href='x'>x</a></main>";
        let queries = &[Query::all("a", Save::all())
            .expect("valid selector")
            .build()];

        let store = parse(html, queries).expect("parse succeeds");

        assert_eq!(store.get("a").unwrap().count(), 1);
    }

    #[test]
    fn tag_only_prefixes_skip_attributes_but_saved_elements_keep_them() {
        let html = concat!(
            "<main data-unused='root'>",
            "<section data-unused='middle'>",
            "<a href='/kept' rel='next'>link</a>",
            "</section></main>"
        );
        let queries = &[Query::all("main a", Save::none()).unwrap().build()];
        let mut reader = Reader::new(html);
        let mut parser = XHtmlParser::new(QueryMultiplexer::new(queries));

        while parser.next(&mut reader) {}

        // `main` is a tag-only, non-save transition and `section` cannot
        // match either active name. Only the terminal `a` needs attributes,
        // both for saving and for preserving the public result contract.
        assert_eq!(parser.attribute_parse_count, 1);
        let store = parser.matches();
        let anchor = store.get("main a").unwrap().next().unwrap();
        assert_eq!(anchor.attribute(&store, "href"), Some("/kept"));
        assert_eq!(anchor.attribute(&store, "rel"), Some("next"));
    }

    #[test]
    fn attribute_selectors_parse_each_name_viable_candidate() {
        let html = "<main><div class='miss'></div><div class='hit'></div></main>";
        let queries = &[Query::all("div.hit", Save::none()).unwrap().build()];
        let mut reader = Reader::new(html);
        let mut parser = XHtmlParser::new(QueryMultiplexer::new(queries));

        while parser.next(&mut reader) {}

        assert_eq!(parser.attribute_parse_count, 2);
        assert_eq!(parser.matches().get("div.hit").unwrap().count(), 1);
    }

    #[test]
    fn normalized_text_requests_only_hidden_on_unmatched_tags() {
        let html = concat!(
            "<div data-a='1' data-b='2'></div>",
            "<span hidden data-c='3' data-d='4'></span>"
        );
        let queries = &[
            Query::all("article", Save::only_text().without_attributes())
                .unwrap()
                .build(),
        ];
        let mut reader = Reader::new(html);
        let mut parser = XHtmlParser::new(QueryMultiplexer::new(queries));

        while parser.next(&mut reader) {}

        assert_eq!(parser.attribute_parse_count, 2);
        assert_eq!(parser.selected_attribute_count, 1);
        assert!(parser.matches().elements.is_empty());
    }

    #[test]
    fn empty_opening_tags_do_not_reach_attribute_preflight() {
        let html = "<>< > <<>><p data-x='hit'></p>";
        let queries = &[Query::all("[data-x]", Save::all()).unwrap().build()];

        let store = parse(html, queries).expect("parse succeeds");

        assert_eq!(store.get("[data-x]").unwrap().count(), 1);
    }

    #[test]
    fn sibling_callback_arena_empty_after_eof_drain() {
        let html = "<main><h1></h1>";
        let mut reader = Reader::new(html);
        let queries = &[
            Query::all("h1 + p", Save::none()).unwrap().build(),
            Query::all("h1 ~ p", Save::none()).unwrap().build(),
        ];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        assert!(
            parser.temp_state.sibling().arena.is_empty(),
            "EOF must truncate every callback range"
        );
        assert!(parser.temp_state.sibling().pending.is_empty());
    }

    #[test]
    fn void_sibling_source_bypasses_callback_arena() {
        let html = "<main><br><p id='hit'></p></main>";
        let mut reader = Reader::new(html);
        let queries = &[Query::all("br + p", Save::none()).unwrap().build()];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        // Open <main>
        assert!(parser.next(&mut reader));
        assert!(parser.temp_state.sibling().arena.is_empty());

        // Void <br>: callback activates immediately and must not enter the arena.
        assert!(parser.next(&mut reader));
        assert!(
            parser.temp_state.sibling().arena.is_empty(),
            "void sources must not append callbacks to the arena"
        );
        assert!(parser.temp_state.sibling().pending.is_empty());

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let hits: Vec<_> = store
            .get("br + p")
            .unwrap()
            .map(|element| element.id)
            .collect();
        assert_eq!(hits, [Some("hit")]);
    }

    #[test]
    fn chained_void_sibling_keeps_callback_arena_empty() {
        // div + br + p: matching <br> registers a continuation that activates
        // immediately, so the void middle element never parks a callback range.
        let html = "<main><div></div><br><p id='hit'></p></main>";
        let mut reader = Reader::new(html);
        let queries = &[Query::all("div + br + p", Save::none()).unwrap().build()];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        assert!(
            parser.temp_state.sibling().arena.is_empty(),
            "chained void activation must leave the arena empty"
        );
        assert!(parser.temp_state.sibling().pending.is_empty());

        let store = parser.matches();
        let hits: Vec<_> = store
            .get("div + br + p")
            .unwrap()
            .map(|element| element.id)
            .collect();
        assert_eq!(hits, [Some("hit")]);
    }

    #[test]
    fn same_batch_discard_truncates_callback_arena() {
        // Closing </main> pops section then div then main. Discarded callback
        // ranges must still be truncated from the arena.
        let html = r#"
        <main>
          <div>
            <section>
        </main>
        <p id="outside-miss"></p>
        "#;
        let mut reader = Reader::new(html);
        let queries = &[
            Query::all("section + p", Save::none()).unwrap().build(),
            Query::all("div ~ p", Save::none()).unwrap().build(),
        ];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        assert!(
            parser.temp_state.sibling().arena.is_empty(),
            "same-batch discards must still truncate arena ranges"
        );
        let store = parser.matches();
        assert_eq!(
            store
                .get("section + p")
                .map(|iter| iter.count())
                .unwrap_or(0),
            0
        );
        assert_eq!(
            store.get("div ~ p").map(|iter| iter.count()).unwrap_or(0),
            0
        );
    }
}
