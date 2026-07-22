use super::element::builder::XHtmlTag;
use super::open_elements::{OpenElement, OpenElementStack};
use super::tag::TagFlags;
use crate::debug::ImpliedCloseReason;
#[cfg(any(debug_assertions, test))]
use crate::debug::TraceEvent;
use crate::engine::MAX_ELEMENT_DEPTH;
use crate::engine::multiplexer::{DocumentPosition, QueryMultiplexer, SaveHit};
use crate::store::Store;
use crate::{ParseError, QuerySpec, Reader, XHtmlElement};

#[derive(Default)]
struct ParserTempState<'html> {
    closing_elements: Vec<OpenElement<'html>>,
    implied_closes: Vec<OpenElement<'html>>,
    save_hits: Vec<SaveHit>,
}

/// Parser for queries that do not capture raw or normalized text.
///
/// This intentionally lives outside the text-capturing parser module so
/// no-text-only binaries do not retain text extraction implementation details.
pub(crate) struct NoTextParser<'html, 'query, Q> {
    position: DocumentPosition,
    pub selectors: QueryMultiplexer<'query, Q>,
    store: Store<'html, 'query>,
    element: XHtmlElement<'html>,
    open_elements: OpenElementStack<'html>,
    temp_state: ParserTempState<'html>,
    raw_text_close: Option<&'static str>,
    eof_drained: bool,
    parse_error: Option<ParseError>,
}

/// A raw-text end tag is appropriate only when the tag name is followed by
/// HTML whitespace, `/`, `>`, or end of input.
#[inline]
fn is_raw_text_end_terminator(byte: Option<u8>) -> bool {
    matches!(
        byte,
        None | Some(b' ' | b'\t' | b'\n' | 0x0C | b'\r' | b'/' | b'>')
    )
}

impl<'html, 'query: 'html, Q> NoTextParser<'html, 'query, Q>
where
    Q: QuerySpec<'query>,
{
    pub fn new(selectors: QueryMultiplexer<'query, Q>) -> Self {
        Self {
            position: DocumentPosition {
                element_depth: 0,
                reader_position: 0,
                self_closing: false,
            },
            selectors,
            store: Store::default(),
            element: XHtmlElement::default(),
            open_elements: OpenElementStack::default(),
            temp_state: ParserTempState::default(),
            raw_text_close: None,
            eof_drained: false,
            parse_error: None,
        }
    }

    pub fn with_capacity(selectors: QueryMultiplexer<'query, Q>, capacity: usize) -> Self {
        Self {
            store: Store::with_capacity_options(
                capacity,
                crate::CapacityOptions {
                    reserve_raw_text: false,
                    reserve_text: false,
                    ..crate::CapacityOptions::default()
                },
            ),
            ..Self::new(selectors)
        }
    }

    pub fn next(&mut self, reader: &mut Reader<'html>) -> bool {
        if self.parse_error.is_some() {
            return false;
        }
        if let Some(close_tag) = self.raw_text_close {
            loop {
                reader.next_until(b'<');
                if reader.peek().is_none() {
                    self.drain_open_elements(reader);
                    return false;
                }

                if reader.match_ignore_case(close_tag)
                    && is_raw_text_end_terminator(reader.peek_at(close_tag.len()))
                {
                    self.raw_text_close = None;
                    self.position.reader_position = reader.get_position();
                    reader.next_until(b'>');
                    reader.skip();

                    let closing_tag = &close_tag[2..];
                    let early_exit = self.handle_close_tag(closing_tag, reader);
                    return !early_exit && !reader.eof();
                }
                reader.skip();
            }
        }

        reader.next_until(b'<');
        if reader.peek().is_none() {
            self.drain_open_elements(reader);
            return false;
        }

        let tag = loop {
            self.position.reader_position = reader.get_position();
            if let Some(tag) = XHtmlTag::from(reader) {
                if matches!(tag, XHtmlTag::Open) {
                    self.element.from(reader, &mut self.store.attributes);
                }
                break tag;
            }

            reader.next_until(b'<');
            if reader.peek().is_none() {
                self.drain_open_elements(reader);
                return false;
            }
        };
        let tag_start_position = self.position.reader_position;
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
                self.drain_implied_closes(reader, Some(ImpliedCloseReason::OpenTagRule), None);
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

                self.selectors.next_into(
                    &self.element,
                    &self.position,
                    &mut self.store,
                    &mut self.temp_state.save_hits,
                );
                if is_self_closing {
                    early_exit = self.selectors.back(
                        self.element.name,
                        &self.position,
                        reader,
                        &mut self.store,
                    );
                } else {
                    for save_hit in &self.temp_state.save_hits {
                        if save_hit.needs_close_finalization() {
                            self.open_elements.attach_saved_without_text(
                                save_hit.element_id,
                                save_hit
                                    .save_inner_html
                                    .then_some(self.position.reader_position),
                            );
                        }
                    }
                }
                self.element.clear();
            }
            XHtmlTag::Close(closing_tag) => {
                early_exit = self.handle_close_tag(closing_tag, reader);
            }
        }

        !early_exit && !reader.eof()
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

    pub fn take_parse_error(&mut self) -> Option<ParseError> {
        self.parse_error.take()
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

    fn record_parse_error(&mut self, err: ParseError) {
        if self.parse_error.is_none() {
            self.parse_error = Some(err);
        }
    }

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

    fn drain_implied_closes(
        &mut self,
        reader: &Reader<'html>,
        implied_close_reason: Option<ImpliedCloseReason>,
        expected_tag: Option<&'html str>,
    ) -> bool {
        let base_depth = self.open_elements.depth();
        let mut elements = std::mem::take(&mut self.temp_state.implied_closes);
        let total = elements.len();
        let mut early_exit = false;

        for (index, open_element) in elements.drain(..).enumerate() {
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
            early_exit = self.pop_open_element(open_element, close_depth, reader) || early_exit;
        }

        self.temp_state.implied_closes = elements;
        early_exit
    }

    fn handle_close_tag(&mut self, closing_tag: &'html str, reader: &Reader<'html>) -> bool {
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
        self.pop_closing_elements(
            reader,
            Some(ImpliedCloseReason::MismatchedEndTag),
            Some(closing_tag),
        )
    }

    fn pop_closing_elements(
        &mut self,
        reader: &Reader<'html>,
        implied_close_reason: Option<ImpliedCloseReason>,
        expected_tag: Option<&'html str>,
    ) -> bool {
        let base_depth = self.open_elements.depth();
        let mut elements = std::mem::take(&mut self.temp_state.closing_elements);
        let total = elements.len();
        let mut early_exit = false;

        for (index, open_element) in elements.drain(..).enumerate() {
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
            early_exit = self.pop_open_element(open_element, close_depth, reader) || early_exit;
        }

        self.temp_state.closing_elements = elements;
        early_exit
    }

    fn finalize_open_element(&mut self, open_element: &OpenElement<'html>, reader: &Reader<'html>) {
        for saved in &open_element.saved {
            let inner_html = saved
                .inner_html_start()
                .map(|start| reader.slice(start..self.position.reader_position));
            self.store
                .set_content(saved.element_id, inner_html, None, None);
        }
    }

    fn drain_open_elements(&mut self, reader: &Reader<'html>) {
        if self.eof_drained {
            return;
        }
        self.position.reader_position = reader.get_position();
        self.open_elements
            .close_all_at_eof_into(&mut self.temp_state.implied_closes);
        self.drain_implied_closes(reader, Some(ImpliedCloseReason::EofDrain), None);
        self.eof_drained = true;
    }
}
