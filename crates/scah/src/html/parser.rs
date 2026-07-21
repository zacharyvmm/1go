use super::element::builder::XHtmlTag;
use super::open_elements::{OpenElement, OpenElementStack};
use super::tag::TagFlags;
use crate::ParseError;
use crate::QuerySpec;
use crate::Reader;
use crate::XHtmlElement;
use crate::debug::ImpliedCloseReason;
#[cfg(any(debug_assertions, test))]
use crate::debug::TraceEvent;
use crate::engine::MAX_ELEMENT_DEPTH;
use crate::engine::multiplexer::{DocumentPosition, QueryMultiplexer, SaveHit, SiblingCallback};
use crate::store::Store;

#[derive(Default)]
struct ParserTempState<'html> {
    closing_elements: Vec<OpenElement<'html>>,
    implied_closes: Vec<OpenElement<'html>>,
    save_hits: Vec<SaveHit>,

    /// Allocated only for query sets that contain sibling combinators. The
    /// optional box keeps plain parser scratch state pointer-sized.
    sibling: Option<Box<SiblingParserState>>,
}

#[derive(Default)]
struct SiblingParserState {
    callback_arena: Vec<SiblingCallback>,
    callback_ranges: Vec<SiblingCallbackRange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SiblingCallbackRange {
    source_depth: crate::engine::DepthSize,
    callback_start: u32,
}

impl SiblingParserState {
    fn record_callback_range(
        &mut self,
        source_depth: crate::engine::DepthSize,
        callback_start: usize,
    ) {
        self.callback_ranges.push(SiblingCallbackRange {
            source_depth,
            callback_start: callback_start
                .try_into()
                .expect("sibling callback arena exceeded u32 index capacity"),
        });
    }

    fn take_callback_start(&mut self, source_depth: crate::engine::DepthSize) -> Option<usize> {
        if self
            .callback_ranges
            .last()
            .is_some_and(|range| range.source_depth == source_depth)
        {
            self.callback_ranges
                .pop()
                .map(|range| range.callback_start as usize)
        } else {
            None
        }
    }
}

impl ParserTempState<'_> {
    fn new(has_sibling_queries: bool) -> Self {
        Self {
            sibling: has_sibling_queries.then(|| Box::new(SiblingParserState::default())),
            ..Self::default()
        }
    }

    fn sibling_callback_arena(&self) -> &Vec<SiblingCallback> {
        &self
            .sibling
            .as_ref()
            .expect("sibling parser path requires sibling state")
            .callback_arena
    }

    fn sibling_callback_arena_mut(&mut self) -> &mut Vec<SiblingCallback> {
        &mut self
            .sibling
            .as_mut()
            .expect("sibling parser path requires sibling state")
            .callback_arena
    }
}

pub struct XHtmlParser<'html, 'query, Q> {
    position: DocumentPosition,
    pub selectors: QueryMultiplexer<'query, Q>,
    store: Store<'html, 'query>,
    element: crate::XHtmlElement<'html>,
    open_elements: OpenElementStack<'html>,
    temp_state: ParserTempState<'html>,
    capture_text_content: bool,
    raw_text_close: Option<&'static str>,
    eof_drained: bool,
    parse_error: Option<ParseError>,
}

/// A raw-text end tag is only "appropriate" when the tag name is immediately
/// followed by HTML whitespace, a `/`, or `>` (or end of input). This prevents
/// a longer name such as `</styles>` from prematurely closing `<style>`.
#[inline]
fn is_raw_text_end_terminator(byte: Option<u8>) -> bool {
    matches!(
        byte,
        None | Some(b' ' | b'\t' | b'\n' | 0x0C | b'\r' | b'/' | b'>')
    )
}

impl<'html, 'query: 'html, Q> XHtmlParser<'html, 'query, Q>
where
    Q: QuerySpec<'query>,
{
    pub fn new(selectors: QueryMultiplexer<'query, Q>) -> Self {
        let capture_text_content = selectors.requires_text_content();
        let has_sibling_queries = selectors.features().has_sibling_queries;
        Self {
            position: DocumentPosition {
                element_depth: 0,
                reader_position: 0, // for inner_html
                text_content_position: usize::MAX,
                self_closing: false,
            },
            selectors,
            element: XHtmlElement::default(),
            open_elements: OpenElementStack::default(),
            temp_state: ParserTempState::new(has_sibling_queries),
            capture_text_content,
            raw_text_close: None,
            eof_drained: false,
            parse_error: None,
            store: Store::default(),
        }
    }

    pub fn with_capacity(selectors: QueryMultiplexer<'query, Q>, capacity: usize) -> Self {
        let capture_text_content = selectors.requires_text_content();
        let has_sibling_queries = selectors.features().has_sibling_queries;
        Self {
            position: DocumentPosition {
                element_depth: 0,
                reader_position: 0, // for inner_html
                text_content_position: usize::MAX,
                self_closing: false,
            },
            selectors,
            element: XHtmlElement::default(),
            open_elements: OpenElementStack::default(),
            temp_state: ParserTempState::new(has_sibling_queries),
            capture_text_content,
            raw_text_close: None,
            eof_drained: false,
            parse_error: None,
            store: Store::with_capacity_options(
                capacity,
                crate::CapacityOptions {
                    reserve_text_content: capture_text_content,
                    ..crate::CapacityOptions::default()
                },
            ),
        }
    }

    pub fn next(&mut self, reader: &mut Reader<'html>) -> bool {
        let features = self.selectors.features();
        if features.has_sibling_queries {
            self.next_with_siblings(reader)
        } else if features.has_retiring_runners {
            self.next_plain::<true>(reader)
        } else {
            self.next_plain::<false>(reader)
        }
    }

    pub(crate) fn run(&mut self, reader: &mut Reader<'html>) {
        let features = self.selectors.features();
        if features.has_sibling_queries {
            self.run_with_siblings(reader);
        } else if features.has_retiring_runners {
            self.run_plain::<true>(reader);
        } else {
            self.run_plain::<false>(reader);
        }
    }

    #[inline(never)]
    fn run_plain<const RETIREMENT: bool>(&mut self, reader: &mut Reader<'html>) {
        while self.next_plain::<RETIREMENT>(reader) {}
    }

    #[inline(never)]
    fn run_with_siblings(&mut self, reader: &mut Reader<'html>) {
        while self.next_with_siblings(reader) {}
    }

    /// Parser event path for query sets without sibling combinators.
    ///
    /// This deliberately mirrors the pre-sibling parser and contains no
    /// callback-arena indexing, callback activation, or sibling close logic.
    #[inline]
    fn next_plain<const RETIREMENT: bool>(&mut self, reader: &mut Reader<'html>) -> bool {
        if self.parse_error.is_some() {
            return false;
        }
        if let Some(close_tag) = self.raw_text_close {
            loop {
                reader.next_until(b'<');
                if reader.peek().is_none() {
                    self.drain_open_elements_plain::<RETIREMENT>(reader);
                    return false;
                }

                if reader.match_ignore_case(close_tag)
                    && is_raw_text_end_terminator(reader.peek_at(close_tag.len()))
                {
                    if self.capture_text_content
                        && self.store.text_content.text_start.is_some()
                        && let Some(position) =
                            self.store.text_content.push(reader, reader.get_position())
                    {
                        self.position.text_content_position = position;
                    }
                    self.raw_text_close = None;

                    self.position.reader_position = reader.get_position();
                    reader.next_until(b'>');
                    reader.skip();

                    if self.capture_text_content {
                        self.store.text_content.set_start(reader.get_position());
                    }

                    let closing_tag = &close_tag[2..];
                    let early_exit = self.handle_close_tag_plain::<RETIREMENT>(closing_tag, reader);
                    return !early_exit && !reader.eof();
                }
                reader.skip();
            }
        }

        reader.next_until(b'<');

        if reader.peek().is_none() {
            self.drain_open_elements_plain::<RETIREMENT>(reader);
            return false;
        }

        let tag = {
            let mut tag: Option<XHtmlTag> = None;

            while tag.is_none() {
                self.position.reader_position = reader.get_position();
                tag = XHtmlTag::from(reader);
                if let Some(XHtmlTag::Open) = tag {
                    self.element.from(reader, &mut self.store.attributes);
                } else if self.capture_text_content
                    && tag.is_none()
                    && self.store.text_content.text_start.is_some()
                    && let Some(position) = self
                        .store
                        .text_content
                        .push(reader, self.position.reader_position)
                {
                    self.position.text_content_position = position;
                    self.store.text_content.set_start(reader.get_position());
                }
            }

            tag.unwrap()
        };
        let tag_start_position = self.position.reader_position;

        if self.capture_text_content
            && self.store.text_content.text_start.is_some()
            && let Some(position) = self
                .store
                .text_content
                .push(reader, self.position.reader_position)
        {
            self.position.text_content_position = position;
        }

        if self.capture_text_content {
            self.store.text_content.set_start(reader.get_position());
        }

        let mut early_exit = false;

        match tag {
            XHtmlTag::Open => {
                let tag = TagFlags::classify(self.element.name);

                if let Some(close_tag) = tag.raw_text_close_tag() {
                    self.raw_text_close = Some(close_tag);
                }

                self.position.reader_position = tag_start_position;
                self.open_elements
                    .prepare_for_open_into(tag, &mut self.temp_state.implied_closes);
                self.drain_implied_closes_plain::<RETIREMENT>(
                    reader,
                    Some(ImpliedCloseReason::OpenTagRule),
                    None,
                );
                self.position.reader_position = reader.get_position();

                let is_self_closing = tag.is_void();
                self.position.self_closing = is_self_closing;
                if is_self_closing {
                    let depth = self.open_elements.depth().saturating_add(1);
                    if depth > MAX_ELEMENT_DEPTH {
                        self.record_parse_error(ParseError::MaximumDepthExceeded);
                        return false;
                    }
                    self.position.element_depth = depth;
                } else if let Err(err) = self.open_elements.push_classified(self.element.name, tag)
                {
                    self.record_parse_error(err);
                    return false;
                } else {
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

                if RETIREMENT {
                    self.selectors.next_plain_into(
                        &self.element,
                        &self.position,
                        &mut self.store,
                        &mut self.temp_state.save_hits,
                    );
                } else {
                    self.selectors.next_plain_dense_into(
                        &self.element,
                        &self.position,
                        &mut self.store,
                        &mut self.temp_state.save_hits,
                    );
                }
                if is_self_closing {
                    if RETIREMENT {
                        early_exit = self.selectors.back(
                            self.element.name,
                            &self.position,
                            reader,
                            &mut self.store,
                        ) || early_exit;
                    } else {
                        self.selectors.back_dense_nonretiring(
                            self.element.name,
                            &self.position,
                            &mut self.store,
                        );
                    }
                } else {
                    for save_hit in &self.temp_state.save_hits {
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
            XHtmlTag::Close(closing_tag) => {
                early_exit =
                    self.handle_close_tag_plain::<RETIREMENT>(closing_tag, reader) || early_exit;
            }
        }

        !early_exit && !reader.eof()
    }

    fn next_with_siblings(&mut self, reader: &mut Reader<'html>) -> bool {
        if self.parse_error.is_some() {
            return false;
        }
        if let Some(close_tag) = self.raw_text_close {
            loop {
                reader.next_until(b'<');
                if reader.peek().is_none() {
                    self.drain_open_elements_with_siblings(reader);
                    return false;
                }

                // `close_tag` is the `</name` prefix. It is only the real end
                // tag when the name is followed by an appropriate terminator
                // (HTML whitespace, `/`, or `>`), so `</styles>` does not close
                // `<style>` and `</style >` does. Consume an appropriate raw
                // end tag here instead of delegating to `XHtmlTag::from`: that
                // parser intentionally keeps text after `/` as part of the
                // closing tag name, which would leave `<style>` open for a
                // tolerated form such as `</style ignored>`.
                if reader.match_ignore_case(close_tag)
                    && is_raw_text_end_terminator(reader.peek_at(close_tag.len()))
                {
                    if self.capture_text_content
                        && self.store.text_content.text_start.is_some()
                        && let Some(position) =
                            self.store.text_content.push(reader, reader.get_position())
                    {
                        self.position.text_content_position = position;
                    }
                    self.raw_text_close = None;

                    self.position.reader_position = reader.get_position();
                    reader.next_until(b'>');
                    reader.skip();

                    if self.capture_text_content {
                        self.store.text_content.set_start(reader.get_position());
                    }

                    let closing_tag = &close_tag[2..];
                    let early_exit = self.handle_close_tag_with_siblings(closing_tag, reader);
                    return !early_exit && !reader.eof();
                } else {
                    reader.skip();
                }
            }
        }

        // move until it finds the first `<`
        reader.next_until(b'<');

        if reader.peek().is_none() {
            self.drain_open_elements_with_siblings(reader);
            return false;
        }

        let tag = {
            let mut tag: Option<XHtmlTag> = None;

            while tag.is_none() {
                self.position.reader_position = reader.get_position();
                tag = XHtmlTag::from(reader);
                if let Some(XHtmlTag::Open) = tag {
                    self.element.from(reader, &mut self.store.attributes);
                } else if self.capture_text_content
                    && tag.is_none()
                    && self.store.text_content.text_start.is_some()
                    && let Some(position) = self
                        .store
                        .text_content
                        .push(reader, self.position.reader_position)
                {
                    self.position.text_content_position = position;
                    self.store.text_content.set_start(reader.get_position());
                }
            }

            tag.unwrap()
        };
        let tag_start_position = self.position.reader_position;

        if self.capture_text_content
            && self.store.text_content.text_start.is_some()
            && let Some(position) = self
                .store
                .text_content
                .push(reader, self.position.reader_position)
        {
            self.position.text_content_position = position;
        }

        if self.capture_text_content {
            self.store.text_content.set_start(reader.get_position());
        }

        // TODO: register the start
        //reader.next_while(|c| c.is_whitespace());
        let mut early_exit = false;

        match tag {
            XHtmlTag::Open => {
                let tag = TagFlags::classify(self.element.name);

                if let Some(close_tag) = tag.raw_text_close_tag() {
                    self.raw_text_close = Some(close_tag);
                }

                self.position.reader_position = tag_start_position;
                self.open_elements
                    .prepare_for_open_into(tag, &mut self.temp_state.implied_closes);
                self.drain_implied_closes_with_siblings(
                    reader,
                    Some(ImpliedCloseReason::OpenTagRule),
                    None,
                    true,
                );
                self.position.reader_position = reader.get_position();

                let is_self_closing = tag.is_void();
                self.position.self_closing = is_self_closing;
                if is_self_closing {
                    let depth = self.open_elements.depth().saturating_add(1);
                    if depth > MAX_ELEMENT_DEPTH {
                        self.record_parse_error(ParseError::MaximumDepthExceeded);
                        return false;
                    }
                    self.position.element_depth = depth;
                } else if let Err(err) = self.open_elements.push_classified(self.element.name, tag)
                {
                    self.record_parse_error(err);
                    return false;
                } else {
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

                let callback_start = self.temp_state.sibling_callback_arena().len();
                let ParserTempState {
                    save_hits, sibling, ..
                } = &mut self.temp_state;
                let callback_arena = &mut sibling
                    .as_mut()
                    .expect("sibling parser path requires sibling state")
                    .callback_arena;
                self.selectors.next_with_siblings_into(
                    &self.element,
                    &self.position,
                    &mut self.store,
                    save_hits,
                    callback_arena,
                );
                if is_self_closing {
                    let source_depth = self.position.element_depth;
                    self.selectors.activate_sibling_callbacks(
                        &self.temp_state.sibling_callback_arena()[callback_start..],
                        source_depth,
                        &mut self.store,
                    );
                    self.temp_state
                        .sibling_callback_arena_mut()
                        .truncate(callback_start);
                    early_exit = self.selectors.back(
                        self.element.name,
                        &self.position,
                        reader,
                        &mut self.store,
                    ) || early_exit;
                } else {
                    for save_hit in &self.temp_state.save_hits {
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
                    if self.temp_state.sibling_callback_arena().len() != callback_start {
                        self.temp_state
                            .sibling
                            .as_mut()
                            .expect("sibling parser path requires sibling state")
                            .record_callback_range(self.position.element_depth, callback_start);
                    }
                }

                self.element.clear();
            }
            XHtmlTag::Close(closing_tag) => {
                early_exit = self.handle_close_tag_with_siblings(closing_tag, reader) || early_exit;
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
                text_content_len: self.store.text_content.len(),
            }
        );
        self.store
    }

    fn pop_open_element_plain<const RETIREMENT: bool>(
        &mut self,
        open_element: OpenElement<'html>,
        close_depth: crate::engine::DepthSize,
        reader: &Reader<'html>,
    ) -> bool {
        self.finalize_open_element(&open_element, reader);
        self.position.element_depth = close_depth;
        if RETIREMENT {
            self.selectors
                .back(open_element.name, &self.position, reader, &mut self.store)
        } else {
            self.selectors.back_dense_nonretiring(
                open_element.name,
                &self.position,
                &mut self.store,
            );
            false
        }
    }

    fn drain_implied_closes_plain<const RETIREMENT: bool>(
        &mut self,
        reader: &Reader<'html>,
        implied_close_reason: Option<ImpliedCloseReason>,
        expected_tag: Option<&'html str>,
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
            early_exit =
                self.pop_open_element_plain::<RETIREMENT>(open_element, close_depth, reader)
                    || early_exit;
        }

        self.temp_state.implied_closes = elems;
        early_exit
    }

    fn handle_close_tag_plain<const RETIREMENT: bool>(
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

        self.open_elements
            .close_by_end_tag_into(closing_tag, &mut self.temp_state.closing_elements);
        self.pop_closing_elements_plain::<RETIREMENT>(
            reader,
            Some(ImpliedCloseReason::MismatchedEndTag),
            Some(closing_tag),
        )
    }

    fn pop_closing_elements_plain<const RETIREMENT: bool>(
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
            early_exit =
                self.pop_open_element_plain::<RETIREMENT>(open_element, close_depth, reader)
                    || early_exit;
        }

        self.temp_state.closing_elements = closing_elements;
        early_exit
    }

    fn drain_open_elements_plain<const RETIREMENT: bool>(&mut self, reader: &Reader<'html>) {
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
            .close_all_at_eof_into(&mut self.temp_state.implied_closes);
        self.drain_implied_closes_plain::<RETIREMENT>(
            reader,
            Some(ImpliedCloseReason::EofDrain),
            None,
        );
        self.eof_drained = true;
    }

    fn pop_open_element_with_siblings(
        &mut self,
        open_element: OpenElement<'html>,
        close_depth: crate::engine::DepthSize,
        reader: &Reader<'html>,
        activate_sibling_callbacks: bool,
    ) -> bool {
        self.finalize_open_element(&open_element, reader);
        self.position.element_depth = close_depth;
        let early_exit =
            self.selectors
                .back(open_element.name, &self.position, reader, &mut self.store);

        let callback_start = self
            .temp_state
            .sibling
            .as_mut()
            .expect("sibling parser path requires sibling state")
            .take_callback_start(close_depth);
        self.finish_sibling_callback_range(callback_start, close_depth, activate_sibling_callbacks);

        early_exit
    }

    fn finish_sibling_callback_range(
        &mut self,
        callback_start: Option<usize>,
        source_depth: crate::engine::DepthSize,
        activate: bool,
    ) {
        let Some(start) = callback_start else {
            return;
        };

        let end = self.temp_state.sibling_callback_arena().len();

        debug_assert!(start <= end, "invalid sibling callback arena range");

        if activate {
            for index in start..end {
                let callback = self.temp_state.sibling_callback_arena()[index];
                self.selectors
                    .activate_sibling_callback(callback, source_depth, &mut self.store);
            }
        }

        self.temp_state.sibling_callback_arena_mut().truncate(start);
    }

    /// Drain the implied-closes vector, finalizing each element, and restore
    /// the vector's capacity for reuse. Returns `true` on early exit.
    fn drain_implied_closes_with_siblings(
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
            early_exit = self.pop_open_element_with_siblings(
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
    fn handle_close_tag_with_siblings(
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

        self.open_elements
            .close_by_end_tag_into(closing_tag, &mut self.temp_state.closing_elements);
        self.pop_closing_elements_with_siblings(
            reader,
            Some(ImpliedCloseReason::MismatchedEndTag),
            Some(closing_tag),
        )
    }

    fn pop_closing_elements_with_siblings(
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
            early_exit = self.pop_open_element_with_siblings(
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
        for saved in &open_element.saved {
            let inner_html = saved
                .inner_html_start
                .map(|start_idx| reader.slice(start_idx..self.position.reader_position));

            let text_content = saved.text_content_start.and_then(|start_idx| {
                // `get_position()` asserts the buffer is non-empty, so guard
                // empty/whitespace-only elements here to avoid a panic.
                if self.store.text_content.is_empty() {
                    return None;
                }
                let end = self.store.text_content.get_position();
                if start_idx == usize::MAX {
                    Some(0..end)
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

    fn drain_open_elements_with_siblings(&mut self, reader: &Reader<'html>) {
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
            .close_all_at_eof_into(&mut self.temp_state.implied_closes);
        self.drain_implied_closes_with_siblings(
            reader,
            Some(ImpliedCloseReason::EofDrain),
            None,
            false,
        );
        debug_assert!(
            self.temp_state.sibling_callback_arena().is_empty(),
            "sibling callback arena leaked callbacks after EOF"
        );
        debug_assert!(
            self.temp_state
                .sibling
                .as_ref()
                .expect("sibling parser path requires sibling state")
                .callback_ranges
                .is_empty(),
            "sibling callback range stack leaked entries after EOF"
        );
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

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn parser_temp_state_keeps_sibling_mode_pointer_sized() {
        assert_eq!(std::mem::size_of::<SiblingParserState>(), 48);
        assert_eq!(std::mem::size_of::<ParserTempState<'_>>(), 80);
    }

    #[test]
    fn sibling_callback_ranges_follow_nested_depth_lifo_order() {
        let mut state = SiblingParserState::default();
        state.record_callback_range(1, 0);
        state.record_callback_range(2, 7);

        assert_eq!(state.take_callback_start(3), None);
        assert_eq!(state.take_callback_start(2), Some(7));
        assert_eq!(state.take_callback_start(1), Some(0));
        assert!(state.callback_ranges.is_empty());
    }

    #[test]
    fn parser_allocates_sibling_state_only_for_sibling_queries() {
        let plain_queries = &[Query::all("main > p", Save::none()).unwrap().build()];
        let sibling_queries = &[Query::all("h1 + p", Save::none()).unwrap().build()];

        let plain = XHtmlParser::new(QueryMultiplexer::new(plain_queries));
        let sibling = XHtmlParser::new(QueryMultiplexer::new(sibling_queries));

        assert!(plain.temp_state.sibling.is_none());
        assert!(sibling.temp_state.sibling.is_some());
    }

    #[test]
    fn specialized_plain_modes_match_parse_results_and_retirement_state() {
        let html = "<main><p id='one'></p><p id='two'></p></main>";

        let all_queries = &[Query::all("p", Save::none()).unwrap().build()];
        let expected_all = parse(html, all_queries).unwrap();
        let mut all_parser = XHtmlParser::new(QueryMultiplexer::new(all_queries));
        let mut all_reader = Reader::new(html);
        while all_parser.next(&mut all_reader) {}
        assert!(all_parser.selectors.active_set_is_dense());
        let actual_all = all_parser.matches();
        assert_eq!(
            actual_all.get("p").unwrap().count(),
            expected_all.get("p").unwrap().count()
        );

        let first_queries = &[Query::first("p", Save::none()).unwrap().build()];
        let expected_first = parse(html, first_queries).unwrap();
        let mut first_parser = XHtmlParser::new(QueryMultiplexer::new(first_queries));
        let mut first_reader = Reader::new(html);
        while first_parser.next(&mut first_reader) {}
        assert!(first_parser.selectors.all_runners_retired_for_test());
        let actual_first = first_parser.matches();
        assert_eq!(
            actual_first.get("p").unwrap().count(),
            expected_first.get("p").unwrap().count()
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
        let mut reader = Reader::new(BASIC_HTML);

        let queries = &[Query::all("p.indent > .bold", Save::only_text_content())
            .unwrap()
            .build()];
        let manager = QueryMultiplexer::new(queries);

        let mut parser = XHtmlParser::new(manager);

        let mut continue_parser = parser.next(&mut reader); // <html>
        assert!(continue_parser);

        continue_parser = parser.next(&mut reader); // <h1>
        assert!(continue_parser);

        continue_parser = parser.next(&mut reader); // </h1>
        assert!(continue_parser);
        assert_eq!(parser.store.text_content.content, b"Hello World ");

        continue_parser = parser.next(&mut reader); // <p class="indent">
        assert!(continue_parser);
        assert_eq!(parser.store.text_content.content, b"Hello World ");

        continue_parser = parser.next(&mut reader); // <span id="name" class="bold">
        assert!(continue_parser);
        assert_eq!(
            parser.store.text_content.content,
            b"Hello World My name is "
        );

        continue_parser = parser.next(&mut reader); // </span>
        assert!(continue_parser);
        assert_eq!(
            parser.store.text_content.content,
            b"Hello World My name is Zachary "
        );

        continue_parser = parser.next(&mut reader); // </p>
        assert!(continue_parser);
        assert_eq!(
            parser.store.text_content.content,
            b"Hello World My name is Zachary "
        );

        continue_parser = parser.next(&mut reader); // </html>
        assert!(!continue_parser);
        assert_eq!(
            parser.store.text_content.content,
            b"Hello World My name is Zachary "
        );
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
        assert_eq!(anchor.text_content(&store), None);
        assert!(store.text_content.content.is_empty());
    }

    #[test]
    fn test_mixed_queries_keep_text_content_available_when_any_query_requests_it() {
        let html = "<div><a href='x'>Hello <b>World</b></a></div>";
        let mut reader = Reader::new(html);
        let queries = &[
            Query::all("a", Save::only_inner_html()).unwrap().build(),
            Query::all("b", Save::only_text_content()).unwrap().build(),
        ];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let anchor = store.get("a").unwrap().next().unwrap();
        let bold = store.get("b").unwrap().next().unwrap();
        assert_eq!(anchor.inner_html, Some("Hello <b>World</b>"));
        assert_eq!(anchor.text_content(&store), None);
        assert_eq!(bold.text_content(&store), Some("World"));
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
        assert_eq!(s1.text_content(&store), Some("Hello World"));

        let s1_div_a: Vec<&Element> = s1.get(&store, "div a").unwrap().collect();
        assert_eq!(s1_div_a.len(), 1);
        assert_eq!(s1_div_a[0].text_content(&store), Some("World"));
        assert_eq!(
            s1_div_a[0].attributes(&store).unwrap()[0].value,
            Some("https://world.com")
        );

        println!("{:#?}", s1);

        let s1_direct_a: Vec<&Element> = s1.get(&store, "> a[href]").unwrap().collect();
        assert_eq!(s1_direct_a.len(), 1);
        assert_eq!(s1_direct_a[0].text_content(&store), Some("Hello"));
        assert_eq!(
            s1_direct_a[0].attributes(&store).unwrap()[0].value,
            Some("https://hello.com")
        );

        // Section 2
        let s2 = sections[1];
        assert_eq!(s2.text_content(&store), Some("Hello2 World2 World3"));

        let s2_div_a: Vec<&Element> = s2.get(&store, "div a").unwrap().collect();
        assert_eq!(s2_div_a.len(), 2, "World3 Element duplicated");
        assert_eq!(s2_div_a[0].text_content(&store), Some("World2"));
        assert_eq!(s2_div_a[1].text_content(&store), Some("World3"));

        let s2_direct_a: Vec<&Element> = s2.get(&store, "> a[href]").unwrap().collect();
        assert_eq!(s2_direct_a.len(), 1);
        assert_eq!(s2_direct_a[0].text_content(&store), Some("Hello2"));
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
        assert_eq!(inputs[0].text_content(&store), None);
        assert_eq!(inputs[0].inner_html, None);

        assert_eq!(inputs[1].text_content(&store), None);
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

        assert_eq!(anchors[0].text_content(&store), Some("Hello 1"));
        assert_eq!(anchors[1].text_content(&store), Some("Hello 2"));
        assert_eq!(anchors[2].text_content(&store), Some("Hello 3"));
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
        assert_eq!(anchor.text_content(&store), Some("Post &lt;0&gt;"));
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
        assert!(span.text_content(&store).is_some());

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
        assert!(a.text_content(&store).is_some());
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
        assert_eq!(element.text_content(&store), Some("Post &lt;0&gt;"));
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
        assert_eq!(p.text_content(&store), Some("Hello"));
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
        assert_eq!(span.text_content(&store), Some("Hello"));
        assert_eq!(div.inner_html, Some("<span>Hello"));
        assert_eq!(div.text_content(&store), Some("Hello"));
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
        assert_eq!(span.text_content(&store), Some("Hello"));
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
        assert_eq!(a.text_content(&store), Some("Link"));
        assert_eq!(section.inner_html, Some("<a href='x'>Link"));
        assert_eq!(section.text_content(&store), Some("Link"));
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
        assert_eq!(items[0].text_content(&store), Some("One"));
        assert_eq!(items[0].inner_html, Some("One"));
        assert_eq!(items[1].text_content(&store), Some("Two"));
        assert_eq!(items[1].inner_html, Some("Two"));
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
        assert_eq!(dts[0].text_content(&store), Some("Term"));
        assert_eq!(dds[0].text_content(&store), Some("Def"));
        assert_eq!(dts[1].text_content(&store), Some("Next"));
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
        assert_eq!(options[0].text_content(&store), Some("One"));
        assert_eq!(options[1].text_content(&store), Some("Two"));
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
        assert_eq!(optgroups[0].text_content(&store), Some("One"));
        assert_eq!(optgroups[1].text_content(&store), Some("Two"));
        assert_eq!(options[0].text_content(&store), Some("One"));
        assert_eq!(options[1].text_content(&store), Some("Two"));
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
        assert_eq!(cells[0].text_content(&store), Some("One"));
        assert_eq!(cells[1].text_content(&store), Some("Two"));
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
        assert_eq!(div.text_content(&store), Some("Hello"));
        assert_eq!(class_match.inner_html, Some("Hello"));
        assert_eq!(class_match.text_content(&store), Some("Hello"));
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
    fn test_text_before_first_tag_does_not_break_text_content() {
        let html = "intro<div>Hello</div>";
        let mut reader = Reader::new(html);
        let queries = &[Query::all("div", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        while parser.next(&mut reader) {}

        let store = parser.matches();
        let div = store.get("div").unwrap().next().unwrap();
        assert_eq!(div.text_content(&store), Some("Hello"));
    }

    #[test]
    fn save_none_does_not_accumulate_text_content() {
        let queries = &[Query::all("a", Save::none()).unwrap().build()];
        let store = parse("<div><a>Hello <b>World</b></a></div>", queries).unwrap();

        let anchor = store.get("a").unwrap().next().unwrap();
        assert_eq!(anchor.text_content(&store), None);
        assert_eq!(store.text_content.len(), 0);
    }

    #[test]
    fn mixed_save_queries_keep_text_content_for_text_query() {
        let queries = &[
            Query::all("a", Save::none()).unwrap().build(),
            Query::all("b", Save::only_text_content()).unwrap().build(),
        ];
        let store = parse("<a>Hello <b>World</b></a>", queries).unwrap();

        let anchor = store.get("a").unwrap().next().unwrap();
        let bold = store.get("b").unwrap().next().unwrap();
        assert_eq!(anchor.text_content(&store), None);
        assert_eq!(bold.text_content(&store), Some("World"));
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
        assert!(section.text_content(&store).is_some());

        let products: Vec<&Element> = section.get(&store, ".product").unwrap().collect();
        assert_eq!(products.len(), 1);

        let product = products[0];
        assert_eq!(product.name, "div");
        assert_eq!(product.class, Some("product"));
        assert!(product.inner_html.is_some());
        assert!(product.text_content(&store).is_some());

        let h1 = product.get(&store, "h1").unwrap().next().unwrap();
        assert_eq!(h1.name, "h1");
        assert_eq!(h1.inner_html, Some("Product #1"));
        assert!(h1.text_content(&store).is_some());

        let img = product.get(&store, "img").unwrap().next().unwrap();
        assert_eq!(img.name, "img");
        assert!(img.attributes(&store).is_some());

        let p = product.get(&store, "p").unwrap().next().unwrap();
        assert_eq!(p.name, "p");
        assert!(p.inner_html.is_some());
        assert!(p.text_content(&store).is_some());
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
        assert!(section.text_content(&store).is_some());

        let products: Vec<&Element> = section.get(&store, ".product").unwrap().collect();
        assert_eq!(products.len(), 2);

        // Product 1
        let p1 = products[0];
        assert_eq!(p1.name, "div");
        assert_eq!(p1.class, Some("product"));
        assert!(p1.inner_html.is_some());
        assert!(p1.text_content(&store).is_some());

        let p1_h1 = p1.get(&store, "h1").unwrap().next().unwrap();
        assert_eq!(p1_h1.name, "h1");
        assert_eq!(p1_h1.inner_html, Some("Product #1"));
        assert!(p1_h1.text_content(&store).is_some());

        let p1_img = p1.get(&store, "img").unwrap().next().unwrap();
        assert_eq!(p1_img.name, "img");
        assert!(p1_img.attributes(&store).is_some());

        let p1_p = p1.get(&store, "p").unwrap().next().unwrap();
        assert_eq!(p1_p.name, "p");
        assert!(p1_p.inner_html.is_some());
        assert!(p1_p.text_content(&store).is_some());

        // Product 2
        let p2 = products[1];
        assert_eq!(p2.name, "div");
        assert_eq!(p2.class, Some("product"));
        assert!(p2.inner_html.is_some());
        assert!(p2.text_content(&store).is_some());

        let p2_h1 = p2.get(&store, "h1").unwrap().next().unwrap();
        assert_eq!(p2_h1.name, "h1");
        assert!(p2_h1.inner_html.is_some());
        assert!(p2_h1.text_content(&store).is_some());

        let p2_img = p2.get(&store, "img").unwrap().next().unwrap();
        assert_eq!(p2_img.name, "img");
        assert!(p2_img.attributes(&store).is_some());

        let p2_p = p2.get(&store, "p").unwrap().next().unwrap();
        assert_eq!(p2_p.name, "p");
        assert!(p2_p.inner_html.is_some());
        assert!(p2_p.text_content(&store).is_some());
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
            parser.temp_state.sibling_callback_arena().is_empty(),
            "EOF must truncate every callback range"
        );
    }

    #[test]
    fn multiple_runners_append_callbacks_to_one_source_range() {
        let html = "<main><div></div><p></p></main>";
        let mut reader = Reader::new(html);
        let queries = &[
            Query::all("div + p", Save::none()).unwrap().build(),
            Query::all("div ~ p", Save::none()).unwrap().build(),
        ];
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);

        assert!(parser.next(&mut reader)); // <main>
        assert!(parser.next(&mut reader)); // <div>

        assert_eq!(parser.temp_state.sibling_callback_arena().len(), 2);
        assert_eq!(
            parser
                .temp_state
                .sibling_callback_arena()
                .iter()
                .map(|callback| callback.runner.index())
                .collect::<Vec<_>>(),
            [0, 1]
        );

        while parser.next(&mut reader) {}
        assert!(parser.temp_state.sibling_callback_arena().is_empty());
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
        assert!(parser.temp_state.sibling_callback_arena().is_empty());

        // Void <br>: callback activates immediately and must not enter the arena.
        assert!(parser.next(&mut reader));
        assert!(
            parser.temp_state.sibling_callback_arena().is_empty(),
            "void sources must not append callbacks to the arena"
        );

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
            parser.temp_state.sibling_callback_arena().is_empty(),
            "chained void activation must leave the arena empty"
        );

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
            parser.temp_state.sibling_callback_arena().is_empty(),
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
