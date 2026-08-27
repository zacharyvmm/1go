//! Scalar element-tape architecture experiment.
//!
//! It first answers the scalar prerequisite question, then compares two
//! automatically selected/vectorised approaches: repeated SIMD searches and a
//! compiler-vectorised dense classification pass.
//!
//! The frontend strategies share the same tag scanner and attribute matcher:
//!
//! - `streaming_eager`: scan each tag once and tokenize every attribute.
//! - `streaming_lazy`: scan each tag once and tokenize attributes only after a
//!   name match.
//! - `span_eager`: find each tag end, then revisit its attributes.
//! - `span_lazy`: find each tag end, reject by name, then revisit attributes.
//! - `tape_eager_*`: materialise one compact record per tag, then eagerly parse.
//! - `tape_lazy_reused`: materialise records, reject by tag name first, and only
//!   parse attributes for plausible candidates.
//!
//! `production_parse` is included as a real-world calibration point, not as an
//! apples-to-apples frontend comparison: production also maintains the HTML
//! open-element stack, runs query cursors, stores matches, and handles recovery.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::{Query, Save, parse};
use std::hint::black_box;
use std::time::Duration;

const CLOSE_TAG: u8 = 1;

/// One semantic record per tag. The four offsets keep the record compact while
/// retaining enough information for a second pass to avoid rescanning text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ElementSpan {
    _tag_start: u32,
    name_start: u32,
    name_end: u32,
    tag_end: u32,
    flags: u8,
}

impl ElementSpan {
    #[inline]
    fn is_close(self) -> bool {
        self.flags & CLOSE_TAG != 0
    }

    #[inline]
    fn name(self, html: &str) -> &str {
        &html[self.name_start as usize..self.name_end as usize]
    }

    #[inline]
    fn attributes(self) -> std::ops::Range<usize> {
        self.name_end as usize..self.tag_end as usize
    }
}

#[derive(Clone, Copy)]
struct SelectorCase {
    label: &'static str,
    source: &'static str,
    name: Option<&'static str>,
    class: Option<&'static str>,
    attribute: Option<&'static str>,
}

const SELECTORS: [SelectorCase; 3] = [
    SelectorCase {
        label: "tag_only",
        source: "a",
        name: Some("a"),
        class: None,
        attribute: None,
    },
    SelectorCase {
        label: "selective_attrs",
        source: "a.promoted[href]",
        name: Some("a"),
        class: Some("promoted"),
        attribute: Some("href"),
    },
    SelectorCase {
        label: "universal_attr",
        source: "[data-index]",
        name: None,
        class: None,
        attribute: Some("data-index"),
    },
];

#[derive(Clone, Copy, Debug, Default)]
struct MatchSummary {
    matches: usize,
    attributes_scanned: usize,
    fingerprint: u64,
}

impl MatchSummary {
    #[inline]
    fn add_attributes(&mut self, parsed: ParsedAttributes) {
        self.attributes_scanned += parsed.count;
        self.fingerprint = self.fingerprint.wrapping_add(parsed.fingerprint);
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ParsedAttributes {
    class_matches: bool,
    attribute_matches: bool,
    count: usize,
    fingerprint: u64,
}

#[inline]
fn is_html_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0c | b'\r')
}

#[inline]
fn find_byte(bytes: &[u8], start: usize, needle: u8) -> Option<usize> {
    bytes[start..]
        .iter()
        .position(|&byte| byte == needle)
        .map(|offset| start + offset)
}

#[inline]
fn find_sequence(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    bytes[start..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| start + offset)
}

/// Find a tag boundary while respecting quoted attribute values. The returned
/// offset is exclusive and includes `>` when present.
fn find_tag_end(bytes: &[u8], mut cursor: usize) -> usize {
    let mut quote = None;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        match quote {
            Some(delimiter) if byte == delimiter => {
                let mut escapes = 0;
                let mut scan = cursor;
                while scan > 0 && bytes[scan - 1] == b'\\' {
                    escapes += 1;
                    scan -= 1;
                }
                if escapes % 2 == 0 {
                    quote = None;
                }
            }
            Some(_) => {}
            None if matches!(byte, b'\'' | b'"') => quote = Some(byte),
            None if byte == b'>' => return cursor + 1,
            None => {}
        }
        cursor += 1;
    }
    bytes.len()
}

/// Scan to the next semantic open/close tag. Comments and declarations are
/// consumed without generating dense structural-character records.
fn next_element_span(html: &str, cursor: &mut usize) -> Option<ElementSpan> {
    let bytes = html.as_bytes();
    loop {
        let tag_start = find_byte(bytes, *cursor, b'<')?;
        let mut position = tag_start + 1;

        if bytes.get(position) == Some(&b'!') {
            *cursor = if bytes.get(position + 1..position + 3) == Some(b"--") {
                find_sequence(bytes, position + 3, b"-->").map_or(bytes.len(), |end| end + 3)
            } else {
                find_byte(bytes, position + 1, b'>').map_or(bytes.len(), |end| end + 1)
            };
            continue;
        }
        if matches!(bytes.get(position), Some(b'?')) {
            *cursor = find_byte(bytes, position + 1, b'>').map_or(bytes.len(), |end| end + 1);
            continue;
        }

        let is_close = bytes.get(position) == Some(&b'/');
        if is_close {
            position += 1;
        }
        while bytes
            .get(position)
            .is_some_and(|byte| is_html_whitespace(*byte))
        {
            position += 1;
        }

        let name_start = position;
        while bytes
            .get(position)
            .is_some_and(|byte| !is_html_whitespace(*byte) && !matches!(*byte, b'/' | b'>'))
        {
            position += 1;
        }
        let name_end = position;
        let tag_end = find_tag_end(bytes, position);
        *cursor = tag_end;

        if name_start == name_end {
            continue;
        }

        return Some(ElementSpan {
            _tag_start: tag_start as u32,
            name_start: name_start as u32,
            name_end: name_end as u32,
            tag_end: tag_end as u32,
            flags: if is_close { CLOSE_TAG } else { 0 },
        });
    }
}

fn build_tape(html: &str, tape: &mut Vec<ElementSpan>) {
    tape.clear();
    let mut cursor = 0;
    while let Some(span) = next_element_span(html, &mut cursor) {
        tape.push(span);
    }
}

/// The same linear semantic indexer using `memchr`'s automatically selected
/// vector backend for bulk byte searches. On AArch64, `memchr` selects its NEON
/// implementation; other targets retain the same source-level algorithm and
/// select their best supported backend.
fn next_element_span_memchr_simd(html: &str, cursor: &mut usize) -> Option<ElementSpan> {
    let bytes = html.as_bytes();
    loop {
        let tag_start = memchr::memchr(b'<', &bytes[*cursor..]).map(|offset| *cursor + offset)?;
        let mut position = tag_start + 1;

        if bytes.get(position) == Some(&b'!') {
            *cursor = if bytes.get(position + 1..position + 3) == Some(b"--") {
                memchr::memmem::find(&bytes[position + 3..], b"-->")
                    .map_or(bytes.len(), |offset| position + 3 + offset + 3)
            } else {
                memchr::memchr(b'>', &bytes[position + 1..])
                    .map_or(bytes.len(), |offset| position + 1 + offset + 1)
            };
            continue;
        }
        if matches!(bytes.get(position), Some(b'?')) {
            *cursor = memchr::memchr(b'>', &bytes[position + 1..])
                .map_or(bytes.len(), |offset| position + 1 + offset + 1);
            continue;
        }

        let is_close = bytes.get(position) == Some(&b'/');
        if is_close {
            position += 1;
        }
        while bytes
            .get(position)
            .is_some_and(|byte| is_html_whitespace(*byte))
        {
            position += 1;
        }

        let name_start = position;
        while bytes
            .get(position)
            .is_some_and(|byte| !is_html_whitespace(*byte) && !matches!(*byte, b'/' | b'>'))
        {
            position += 1;
        }
        let name_end = position;

        let mut quote = None;
        let mut tag_end = bytes.len();
        for offset in memchr::memchr3_iter(b'>', b'"', b'\'', &bytes[position..]) {
            let candidate = position + offset;
            let byte = bytes[candidate];
            match quote {
                Some(delimiter) if byte == delimiter => {
                    let mut escapes = 0;
                    let mut scan = candidate;
                    while scan > 0 && bytes[scan - 1] == b'\\' {
                        escapes += 1;
                        scan -= 1;
                    }
                    if escapes % 2 == 0 {
                        quote = None;
                    }
                }
                Some(_) => {}
                None if matches!(byte, b'"' | b'\'') => quote = Some(byte),
                None => {
                    tag_end = candidate + 1;
                    break;
                }
            }
        }
        *cursor = tag_end;

        if name_start == name_end {
            continue;
        }

        return Some(ElementSpan {
            _tag_start: tag_start as u32,
            name_start: name_start as u32,
            name_end: name_end as u32,
            tag_end: tag_end as u32,
            flags: if is_close { CLOSE_TAG } else { 0 },
        });
    }
}

fn build_tape_memchr_simd(html: &str, tape: &mut Vec<ElementSpan>) {
    tape.clear();
    let mut cursor = 0;
    while let Some(span) = next_element_span_memchr_simd(html, &mut cursor) {
        tape.push(span);
    }
}

const CLASS_LT: u8 = 1;
const CLASS_GT: u8 = 1 << 1;
const CLASS_DOUBLE_QUOTE: u8 = 1 << 2;
const CLASS_SINGLE_QUOTE: u8 = 1 << 3;

/// Dense byte classification is intentional here: the loop has no stateful
/// dependencies and gives LLVM enough contiguous work to auto-vectorise. The
/// second pass consumes the reusable one-byte-per-input-byte scratch buffer.
#[inline(never)]
fn classify_structural_bytes(bytes: &[u8], classes: &mut [u8]) {
    assert_eq!(bytes.len(), classes.len());
    for (byte, class) in bytes.iter().copied().zip(classes.iter_mut()) {
        *class = (u8::from(byte == b'<') * CLASS_LT)
            | (u8::from(byte == b'>') * CLASS_GT)
            | (u8::from(byte == b'"') * CLASS_DOUBLE_QUOTE)
            | (u8::from(byte == b'\'') * CLASS_SINGLE_QUOTE);
    }
}

#[derive(Clone, Copy)]
enum DenseState {
    Text,
    Element {
        tag_start: usize,
        name_start: usize,
        name_end: usize,
        flags: u8,
    },
    Declaration,
    Comment,
}

fn build_tape_dense_auto_simd(html: &str, classes: &mut Vec<u8>, tape: &mut Vec<ElementSpan>) {
    let bytes = html.as_bytes();
    classes.resize(bytes.len(), 0);
    classify_structural_bytes(bytes, classes);
    tape.clear();

    let mut state = DenseState::Text;
    let mut quote = 0;

    for (position, &class) in classes.iter().enumerate() {
        if class == 0 {
            continue;
        }

        if quote != 0 {
            if class & quote != 0 {
                let mut escapes = 0;
                let mut scan = position;
                while scan > 0 && bytes[scan - 1] == b'\\' {
                    escapes += 1;
                    scan -= 1;
                }
                if escapes % 2 == 0 {
                    quote = 0;
                }
            }
            continue;
        }

        if class & CLASS_DOUBLE_QUOTE != 0
            && matches!(state, DenseState::Element { .. } | DenseState::Declaration)
        {
            quote = CLASS_DOUBLE_QUOTE;
            continue;
        }
        if class & CLASS_SINGLE_QUOTE != 0
            && matches!(state, DenseState::Element { .. } | DenseState::Declaration)
        {
            quote = CLASS_SINGLE_QUOTE;
            continue;
        }

        match state {
            DenseState::Text if class & CLASS_LT != 0 => {
                let mut cursor = position + 1;
                state = if bytes.get(cursor) == Some(&b'!') {
                    if bytes.get(cursor + 1..cursor + 3) == Some(b"--") {
                        DenseState::Comment
                    } else {
                        DenseState::Declaration
                    }
                } else if bytes.get(cursor) == Some(&b'?') {
                    DenseState::Declaration
                } else {
                    let is_close = bytes.get(cursor) == Some(&b'/');
                    cursor += usize::from(is_close);
                    while bytes
                        .get(cursor)
                        .is_some_and(|byte| is_html_whitespace(*byte))
                    {
                        cursor += 1;
                    }
                    let name_start = cursor;
                    while bytes.get(cursor).is_some_and(|byte| {
                        !is_html_whitespace(*byte) && !matches!(*byte, b'/' | b'>')
                    }) {
                        cursor += 1;
                    }
                    DenseState::Element {
                        tag_start: position,
                        name_start,
                        name_end: cursor,
                        flags: if is_close { CLOSE_TAG } else { 0 },
                    }
                };
            }
            DenseState::Element {
                tag_start,
                name_start,
                name_end,
                flags,
            } if class & CLASS_GT != 0 => {
                if name_start != name_end {
                    tape.push(ElementSpan {
                        _tag_start: tag_start as u32,
                        name_start: name_start as u32,
                        name_end: name_end as u32,
                        tag_end: (position + 1) as u32,
                        flags,
                    });
                }
                state = DenseState::Text;
            }
            DenseState::Declaration if class & CLASS_GT != 0 => state = DenseState::Text,
            DenseState::Comment
                if class & CLASS_GT != 0
                    && (bytes.get(position.saturating_sub(2)..position) == Some(b"--")
                        || bytes.get(position.saturating_sub(3)..position) == Some(b"--!")) =>
            {
                state = DenseState::Text;
            }
            _ => {}
        }
    }

    if let DenseState::Element {
        tag_start,
        name_start,
        name_end,
        flags,
    } = state
        && name_start != name_end
    {
        tape.push(ElementSpan {
            _tag_start: tag_start as u32,
            name_start: name_start as u32,
            name_end: name_end as u32,
            tag_end: bytes.len() as u32,
            flags,
        });
    }
}

#[inline]
fn contains_class(value: &str, wanted: &str) -> bool {
    value.split_ascii_whitespace().any(|class| class == wanted)
}

/// Parse every attribute in the supplied tag range and retain only the bits
/// required by the benchmark selector. The count/fingerprint make the eager
/// work externally observable without materialising a second dense tape.
fn parse_attributes(
    html: &str,
    range: std::ops::Range<usize>,
    selector: SelectorCase,
) -> ParsedAttributes {
    let bytes = html.as_bytes();
    let mut cursor = range.start;
    let end = range
        .end
        .saturating_sub(usize::from(bytes.get(range.end - 1) == Some(&b'>')));
    let mut result = ParsedAttributes::default();

    while cursor < end {
        while cursor < end && (is_html_whitespace(bytes[cursor]) || bytes[cursor] == b'/') {
            cursor += 1;
        }
        if cursor >= end {
            break;
        }

        let key_start = cursor;
        while cursor < end
            && !is_html_whitespace(bytes[cursor])
            && !matches!(bytes[cursor], b'=' | b'/' | b'>')
        {
            cursor += 1;
        }
        let key_end = cursor;
        if key_start == key_end {
            cursor += 1;
            continue;
        }

        while cursor < end && is_html_whitespace(bytes[cursor]) {
            cursor += 1;
        }

        let mut value = None;
        if cursor < end && bytes[cursor] == b'=' {
            cursor += 1;
            while cursor < end && is_html_whitespace(bytes[cursor]) {
                cursor += 1;
            }
            if cursor < end && matches!(bytes[cursor], b'\'' | b'"') {
                let quote = bytes[cursor];
                cursor += 1;
                let value_start = cursor;
                while cursor < end && bytes[cursor] != quote {
                    cursor += 1;
                }
                value = Some(&html[value_start..cursor]);
                cursor += usize::from(cursor < end);
            } else {
                let value_start = cursor;
                while cursor < end && !is_html_whitespace(bytes[cursor]) && bytes[cursor] != b'>' {
                    cursor += 1;
                }
                value = Some(&html[value_start..cursor]);
            }
        }

        let key = &html[key_start..key_end];
        result.count += 1;
        result.fingerprint = result
            .fingerprint
            .wrapping_mul(16_777_619)
            .wrapping_add(key.len() as u64)
            .wrapping_add(value.map_or(0, |value| value.len() as u64));
        if selector
            .attribute
            .is_some_and(|wanted| key.eq_ignore_ascii_case(wanted))
        {
            result.attribute_matches = true;
        }
        if key.eq_ignore_ascii_case("class")
            && selector
                .class
                .is_some_and(|wanted| value.is_some_and(|value| contains_class(value, wanted)))
        {
            result.class_matches = true;
        }
    }

    result
}

#[inline]
fn name_matches(name: &str, selector: SelectorCase) -> bool {
    selector
        .name
        .is_none_or(|wanted| name.eq_ignore_ascii_case(wanted))
}

#[inline]
fn attributes_match(parsed: ParsedAttributes, selector: SelectorCase) -> bool {
    (selector.class.is_none() || parsed.class_matches)
        && (selector.attribute.is_none() || parsed.attribute_matches)
}

#[inline]
fn complete_match(name: &str, parsed: ParsedAttributes, selector: SelectorCase) -> bool {
    name_matches(name, selector)
        && (selector.class.is_none() || parsed.class_matches)
        && (selector.attribute.is_none() || parsed.attribute_matches)
}

fn parse_streaming_attributes(
    html: &str,
    cursor: &mut usize,
    selector: SelectorCase,
) -> ParsedAttributes {
    let bytes = html.as_bytes();
    let mut result = ParsedAttributes::default();

    while *cursor < bytes.len() {
        while *cursor < bytes.len()
            && (is_html_whitespace(bytes[*cursor]) || bytes[*cursor] == b'/')
        {
            *cursor += 1;
        }
        if *cursor >= bytes.len() {
            break;
        }
        if bytes[*cursor] == b'>' {
            *cursor += 1;
            break;
        }

        let key_start = *cursor;
        while *cursor < bytes.len()
            && !is_html_whitespace(bytes[*cursor])
            && !matches!(bytes[*cursor], b'=' | b'/' | b'>')
        {
            *cursor += 1;
        }
        let key_end = *cursor;
        if key_start == key_end {
            *cursor += 1;
            continue;
        }

        while *cursor < bytes.len() && is_html_whitespace(bytes[*cursor]) {
            *cursor += 1;
        }

        let mut value = None;
        if bytes.get(*cursor) == Some(&b'=') {
            *cursor += 1;
            while *cursor < bytes.len() && is_html_whitespace(bytes[*cursor]) {
                *cursor += 1;
            }
            if bytes
                .get(*cursor)
                .is_some_and(|byte| matches!(byte, b'\'' | b'"'))
            {
                let quote = bytes[*cursor];
                *cursor += 1;
                let value_start = *cursor;
                while *cursor < bytes.len() && bytes[*cursor] != quote {
                    *cursor += 1;
                }
                value = Some(&html[value_start..*cursor]);
                *cursor += usize::from(*cursor < bytes.len());
            } else {
                let value_start = *cursor;
                while *cursor < bytes.len()
                    && !is_html_whitespace(bytes[*cursor])
                    && bytes[*cursor] != b'>'
                {
                    *cursor += 1;
                }
                value = Some(&html[value_start..*cursor]);
            }
        }

        let key = &html[key_start..key_end];
        result.count += 1;
        result.fingerprint = result
            .fingerprint
            .wrapping_mul(16_777_619)
            .wrapping_add(key.len() as u64)
            .wrapping_add(value.map_or(0, |value| value.len() as u64));
        if selector
            .attribute
            .is_some_and(|wanted| key.eq_ignore_ascii_case(wanted))
        {
            result.attribute_matches = true;
        }
        if key.eq_ignore_ascii_case("class")
            && selector
                .class
                .is_some_and(|wanted| value.is_some_and(|value| contains_class(value, wanted)))
        {
            result.class_matches = true;
        }
    }

    result
}

/// Advance over an attribute list without tokenising it or revisiting bytes.
/// This intentionally uses the same forward-only quote handling as
/// `parse_streaming_attributes` so lazy and eager streaming controls differ in
/// attribute work, not in how expensively they find the end of a tag.
fn skip_streaming_attributes(bytes: &[u8], cursor: &mut usize) {
    let mut quote = None;

    while *cursor < bytes.len() {
        let byte = bytes[*cursor];
        match quote {
            Some(delimiter) if byte == delimiter => quote = None,
            Some(_) => {}
            None if matches!(byte, b'\'' | b'"') => quote = Some(byte),
            None if byte == b'>' => {
                *cursor += 1;
                break;
            }
            None => {}
        }
        *cursor += 1;
    }
}

fn streaming(html: &str, selector: SelectorCase, eager: bool) -> MatchSummary {
    let bytes = html.as_bytes();
    let mut summary = MatchSummary::default();
    let mut cursor = 0;

    while let Some(tag_start) = find_byte(bytes, cursor, b'<') {
        let mut position = tag_start + 1;
        if bytes.get(position) == Some(&b'!') {
            cursor = if bytes.get(position + 1..position + 3) == Some(b"--") {
                find_sequence(bytes, position + 3, b"-->").map_or(bytes.len(), |end| end + 3)
            } else {
                find_byte(bytes, position + 1, b'>').map_or(bytes.len(), |end| end + 1)
            };
            continue;
        }
        if bytes.get(position) == Some(&b'?') {
            cursor = find_byte(bytes, position + 1, b'>').map_or(bytes.len(), |end| end + 1);
            continue;
        }

        let is_close = bytes.get(position) == Some(&b'/');
        position += usize::from(is_close);
        while bytes
            .get(position)
            .is_some_and(|byte| is_html_whitespace(*byte))
        {
            position += 1;
        }
        let name_start = position;
        while bytes
            .get(position)
            .is_some_and(|byte| !is_html_whitespace(*byte) && !matches!(*byte, b'/' | b'>'))
        {
            position += 1;
        }
        let name = &html[name_start..position];

        if name.is_empty() || is_close {
            cursor = find_tag_end(bytes, position);
            continue;
        }

        let matches_name = name_matches(name, selector);
        let needs_attributes =
            eager || (matches_name && (selector.class.is_some() || selector.attribute.is_some()));
        if needs_attributes {
            let parsed = parse_streaming_attributes(html, &mut position, selector);
            summary.add_attributes(parsed);
            summary.matches += usize::from(if eager {
                complete_match(name, parsed, selector)
            } else {
                matches_name && attributes_match(parsed, selector)
            });
            cursor = position;
        } else {
            summary.matches += usize::from(matches_name);
            skip_streaming_attributes(bytes, &mut position);
            cursor = position;
        }
    }

    summary
}

fn streaming_eager(html: &str, selector: SelectorCase) -> MatchSummary {
    streaming(html, selector, true)
}

fn streaming_lazy(html: &str, selector: SelectorCase) -> MatchSummary {
    streaming(html, selector, false)
}

fn span_eager(html: &str, selector: SelectorCase) -> MatchSummary {
    let mut summary = MatchSummary::default();
    let mut cursor = 0;
    while let Some(span) = next_element_span(html, &mut cursor) {
        if span.is_close() {
            continue;
        }
        let parsed = parse_attributes(html, span.attributes(), selector);
        summary.add_attributes(parsed);
        summary.matches += usize::from(complete_match(span.name(html), parsed, selector));
    }
    summary
}

fn span_lazy(html: &str, selector: SelectorCase) -> MatchSummary {
    let mut summary = MatchSummary::default();
    let mut cursor = 0;
    while let Some(span) = next_element_span(html, &mut cursor) {
        if span.is_close() || !name_matches(span.name(html), selector) {
            continue;
        }

        if selector.class.is_none() && selector.attribute.is_none() {
            summary.matches += 1;
            continue;
        }

        let parsed = parse_attributes(html, span.attributes(), selector);
        summary.add_attributes(parsed);
        summary.matches += usize::from(attributes_match(parsed, selector));
    }
    summary
}

fn span_lazy_memchr_simd(html: &str, selector: SelectorCase) -> MatchSummary {
    let mut summary = MatchSummary::default();
    let mut cursor = 0;
    while let Some(span) = next_element_span_memchr_simd(html, &mut cursor) {
        if span.is_close() || !name_matches(span.name(html), selector) {
            continue;
        }

        if selector.class.is_none() && selector.attribute.is_none() {
            summary.matches += 1;
            continue;
        }

        let parsed = parse_attributes(html, span.attributes(), selector);
        summary.add_attributes(parsed);
        summary.matches += usize::from(attributes_match(parsed, selector));
    }
    summary
}

fn tape_eager(html: &str, tape: &[ElementSpan], selector: SelectorCase) -> MatchSummary {
    let mut summary = MatchSummary::default();
    for &span in tape {
        if span.is_close() {
            continue;
        }
        let parsed = parse_attributes(html, span.attributes(), selector);
        summary.add_attributes(parsed);
        summary.matches += usize::from(complete_match(span.name(html), parsed, selector));
    }
    summary
}

fn tape_lazy(html: &str, tape: &[ElementSpan], selector: SelectorCase) -> MatchSummary {
    let mut summary = MatchSummary::default();
    for &span in tape {
        if span.is_close() || !name_matches(span.name(html), selector) {
            continue;
        }

        if selector.class.is_none() && selector.attribute.is_none() {
            summary.matches += 1;
            continue;
        }

        let parsed = parse_attributes(html, span.attributes(), selector);
        summary.add_attributes(parsed);
        summary.matches += usize::from(attributes_match(parsed, selector));
    }
    summary
}

fn generate_html(rows: usize) -> String {
    let mut html = String::with_capacity(rows * 360);
    html.push_str("<!doctype html><html><body><main id=\"catalog\">");
    for row in 0..rows {
        let promotion = if row % 16 == 0 { " promoted" } else { "" };
        html.push_str(&format!(
            "<article class=\"card filler\" data-index=\"{row}\" aria-label=\"Product {row}\"><a class=\"link{promotion}\" href=\"/post/{row}\" data-track=\"catalog\"><span class=\"label\">Post {row}</span></a><img src=\"/img/{row}.png\" alt=\"Product {row}\"></article>"
        ));
    }
    html.push_str("</main></body></html>");
    html
}

fn generate_attribute_sparse_html(rows: usize) -> String {
    let mut html = String::with_capacity(rows * 80);
    html.push_str("<!doctype html><html><body><main>");
    for row in 0..rows {
        html.push_str("<section><p>Row ");
        html.push_str(&row.to_string());
        html.push_str("</p><a>open</a><span>label</span></section>");
    }
    html.push_str("</main></body></html>");
    html
}

fn assert_indexers_equivalent() {
    let cases = [
        r#"<div data='a>b'><span title="x>y"></span></div>"#,
        r#"<!-- bare > remains comment --><p>x</p>"#,
        r#"<!-- \" --><p>x</p>"#,
        r#"<!-- ' --><p>x</p>"#,
        r#"<!doctype html><main><img src='/x>y'></main>"#,
        r#"<?xml version='1.0'?><a href="/x"></a>"#,
        r#"<div data="escaped\" > quote"></div>"#,
        r#"<div data="unterminated"#,
    ];

    for html in cases {
        let mut scalar = Vec::new();
        let mut memchr_simd = Vec::new();
        let mut dense_classes = Vec::new();
        let mut dense_auto_simd = Vec::new();
        build_tape(html, &mut scalar);
        build_tape_memchr_simd(html, &mut memchr_simd);
        build_tape_dense_auto_simd(html, &mut dense_classes, &mut dense_auto_simd);
        assert_eq!(scalar, memchr_simd, "memchr SIMD mismatch for {html:?}");
        assert_eq!(
            scalar, dense_auto_simd,
            "dense auto-SIMD mismatch for {html:?}"
        );
    }
}

fn bench_element_tape(c: &mut Criterion) {
    assert_indexers_equivalent();
    let mut group = c.benchmark_group("scalar_element_tape");
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));

    for rows in [100, 1_000, 10_000] {
        let html = generate_html(rows);
        group.throughput(Throughput::Bytes(html.len() as u64));

        let mut index_tape = Vec::new();
        build_tape(&html, &mut index_tape);
        let mut index_only_tape = Vec::with_capacity(index_tape.len());
        group.bench_with_input(
            BenchmarkId::new("index_only_reused", rows),
            &html,
            |b, html| {
                b.iter(|| {
                    build_tape(black_box(html), &mut index_only_tape);
                    black_box(index_only_tape.len())
                })
            },
        );

        let mut memchr_simd_index_tape = Vec::with_capacity(index_tape.len());
        build_tape_memchr_simd(&html, &mut memchr_simd_index_tape);
        assert_eq!(index_tape, memchr_simd_index_tape);
        group.bench_with_input(
            BenchmarkId::new("index_memchr_simd_reused", rows),
            &html,
            |b, html| {
                b.iter(|| {
                    build_tape_memchr_simd(black_box(html), &mut memchr_simd_index_tape);
                    black_box(memchr_simd_index_tape.len())
                })
            },
        );

        let mut dense_classes = Vec::with_capacity(html.len());
        let mut dense_tape = Vec::with_capacity(index_tape.len());
        build_tape_dense_auto_simd(&html, &mut dense_classes, &mut dense_tape);
        assert_eq!(index_tape, dense_tape);
        let mut classify_only_classes = vec![0; html.len()];
        group.bench_with_input(
            BenchmarkId::new("classify_dense_auto_simd", rows),
            &html,
            |b, html| {
                b.iter(|| {
                    classify_structural_bytes(
                        black_box(html.as_bytes()),
                        &mut classify_only_classes,
                    );
                    black_box(classify_only_classes.as_ptr())
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("index_dense_auto_simd_reused", rows),
            &html,
            |b, html| {
                b.iter(|| {
                    build_tape_dense_auto_simd(
                        black_box(html),
                        &mut dense_classes,
                        &mut dense_tape,
                    );
                    black_box(dense_tape.len())
                })
            },
        );

        for selector in SELECTORS {
            let streaming_eager_result = streaming_eager(&html, selector);
            let streaming_lazy_result = streaming_lazy(&html, selector);
            let span = span_eager(&html, selector);
            let span_lazy_result = span_lazy(&html, selector);
            let span_lazy_memchr_simd_result = span_lazy_memchr_simd(&html, selector);
            let eager = tape_eager(&html, &index_tape, selector);
            let lazy = tape_lazy(&html, &index_tape, selector);
            assert_eq!(
                streaming_eager_result.matches,
                streaming_lazy_result.matches
            );
            assert_eq!(streaming_eager_result.matches, span.matches);
            assert_eq!(streaming_eager_result.matches, span_lazy_result.matches);
            assert_eq!(
                streaming_eager_result.matches,
                span_lazy_memchr_simd_result.matches
            );
            assert_eq!(streaming_eager_result.matches, eager.matches);
            assert_eq!(streaming_eager_result.matches, lazy.matches);

            let validation_queries = &[Query::all(selector.source, Save::none())
                .expect("benchmark selector should compile")
                .build()];
            let production_matches = parse(&html, validation_queries)
                .unwrap()
                .get(selector.source)
                .unwrap()
                .count();
            assert_eq!(streaming_eager_result.matches, production_matches);

            let parameter = format!("{}/{rows}", selector.label);
            group.bench_with_input(
                BenchmarkId::new("streaming_eager", &parameter),
                &html,
                |b, html| b.iter(|| black_box(streaming_eager(black_box(html), selector))),
            );

            group.bench_with_input(
                BenchmarkId::new("streaming_lazy", &parameter),
                &html,
                |b, html| b.iter(|| black_box(streaming_lazy(black_box(html), selector))),
            );

            group.bench_with_input(
                BenchmarkId::new("span_eager", &parameter),
                &html,
                |b, html| b.iter(|| black_box(span_eager(black_box(html), selector))),
            );

            group.bench_with_input(
                BenchmarkId::new("span_lazy", &parameter),
                &html,
                |b, html| b.iter(|| black_box(span_lazy(black_box(html), selector))),
            );

            group.bench_with_input(
                BenchmarkId::new("span_lazy_memchr_simd", &parameter),
                &html,
                |b, html| b.iter(|| black_box(span_lazy_memchr_simd(black_box(html), selector))),
            );

            group.bench_with_input(
                BenchmarkId::new("tape_eager_consume_only", &parameter),
                &html,
                |b, html| b.iter(|| black_box(tape_eager(black_box(html), &index_tape, selector))),
            );

            group.bench_with_input(
                BenchmarkId::new("tape_lazy_consume_only", &parameter),
                &html,
                |b, html| b.iter(|| black_box(tape_lazy(black_box(html), &index_tape, selector))),
            );

            group.bench_with_input(
                BenchmarkId::new("tape_eager_fresh", &parameter),
                &html,
                |b, html| {
                    b.iter(|| {
                        let mut tape = Vec::new();
                        build_tape(black_box(html), &mut tape);
                        black_box(tape_eager(html, &tape, selector))
                    })
                },
            );

            let mut eager_tape = Vec::with_capacity(index_tape.len());
            group.bench_with_input(
                BenchmarkId::new("tape_eager_reused", &parameter),
                &html,
                |b, html| {
                    b.iter(|| {
                        build_tape(black_box(html), &mut eager_tape);
                        black_box(tape_eager(html, &eager_tape, selector))
                    })
                },
            );

            let mut lazy_tape = Vec::with_capacity(index_tape.len());
            group.bench_with_input(
                BenchmarkId::new("tape_lazy_reused", &parameter),
                &html,
                |b, html| {
                    b.iter(|| {
                        build_tape(black_box(html), &mut lazy_tape);
                        black_box(tape_lazy(html, &lazy_tape, selector))
                    })
                },
            );

            let mut memchr_simd_lazy_tape = Vec::with_capacity(index_tape.len());
            group.bench_with_input(
                BenchmarkId::new("tape_lazy_memchr_simd_reused", &parameter),
                &html,
                |b, html| {
                    b.iter(|| {
                        build_tape_memchr_simd(black_box(html), &mut memchr_simd_lazy_tape);
                        black_box(tape_lazy(html, &memchr_simd_lazy_tape, selector))
                    })
                },
            );

            let mut dense_lazy_classes = Vec::with_capacity(html.len());
            let mut dense_lazy_tape = Vec::with_capacity(index_tape.len());
            group.bench_with_input(
                BenchmarkId::new("tape_lazy_dense_auto_simd_reused", &parameter),
                &html,
                |b, html| {
                    b.iter(|| {
                        build_tape_dense_auto_simd(
                            black_box(html),
                            &mut dense_lazy_classes,
                            &mut dense_lazy_tape,
                        );
                        black_box(tape_lazy(html, &dense_lazy_tape, selector))
                    })
                },
            );

            let queries = &[Query::all(selector.source, Save::none())
                .expect("benchmark selector should compile")
                .build()];
            group.bench_with_input(
                BenchmarkId::new("production_parse", &parameter),
                &html,
                |b, html| {
                    b.iter(|| {
                        let store = parse(black_box(html), black_box(queries)).unwrap();
                        black_box(store.get(selector.source).unwrap().count())
                    })
                },
            );
        }
    }
    group.finish();
}

fn bench_production_query_scaling(c: &mut Criterion) {
    let dense = generate_html(1_000);
    let sparse = generate_attribute_sparse_html(1_000);
    let mut group = c.benchmark_group("production_query_scaling");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));

    for (shape, html) in [("attribute_dense", dense), ("attribute_sparse", sparse)] {
        group.throughput(Throughput::Bytes(html.len() as u64));
        for (workload, selector) in [
            ("matching_output", "a.promoted[href]"),
            ("zero_output", "a.__never_matches__[href]"),
        ] {
            for query_count in [1, 4, 16, 64] {
                let queries = (0..query_count)
                    .map(|_| {
                        Query::all(selector, Save::none())
                            .expect("benchmark selector should compile")
                            .build()
                    })
                    .collect::<Vec<_>>();
                let parameter = format!("{shape}/{workload}");
                group.bench_with_input(
                    BenchmarkId::new(parameter, query_count),
                    &html,
                    |b, html| {
                        b.iter(|| {
                            let store = parse(black_box(html), black_box(&queries)).unwrap();
                            black_box(store.elements.len())
                        })
                    },
                );
            }
        }
    }
    group.finish();
}

criterion_group!(benches, bench_element_tape, bench_production_query_scaling);
criterion_main!(benches);
