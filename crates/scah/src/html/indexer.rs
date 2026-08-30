use std::ops::Range;

use super::simd_classifier::{BlockClassifier, ClassMasks};

/// Pay SIMD classification only after a scalar probe shows that the current
/// delimiter span is not one of HTML's common short spans.
const SCALAR_SEARCH_PREFIX: usize = 32;
const SIMD_BLOCK_BYTES: usize = 16;
const ROLLING_WINDOW_BLOCKS: usize = 256;
const FULL_INDEX_CHUNK_BYTES: usize = 64 * 1024;
const FULL_INDEX_CHUNK_EVENTS: usize = 2_048;
const FULL_INDEX_MIN_BYTES: usize = 16 * 1024;
// Attribute-sensitive sampling does more setup work; below this crossover the
// rolling path is faster even for one long sparse text span.
const FULL_INDEX_MIN_ATTRIBUTE_BYTES: usize = 128 * 1024;
const FULL_INDEX_MIN_BYTES_PER_TAG: usize = 67;
// A full index needs enough structural events to amortize its extra pass.
// Inputs with too few sampled tags stay on the rolling path.
const FULL_INDEX_MIN_SAMPLED_TAGS: usize = 4;
const FULL_INDEX_SAMPLE_WINDOWS: usize = 4;
const FULL_INDEX_DENSITY_WINDOW_BYTES: usize = 8 * 1024;
const FULL_INDEX_MARKUP_WINDOW_BYTES: usize = 1024;
const FULL_INDEX_MAX_MARKUP_PERCENT: usize = 50;
const FULL_INDEX_MAX_SAMPLED_TAG_BYTES: usize = 1024;
const FULL_INDEX_MAX_UNKNOWN_PROBE_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexingMode {
    Rolling,
    FullDocument,
    /// Force the full-document backend, bypassing adaptive strategy selection.
    /// This is intended for isolated strategy benchmarks.
    ForcedFullDocument,
}

/// The kind of completed structural span discovered by a [`TagIndexer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagKind {
    Close,
    /// A comment, declaration, or bogus `<!...>` construct. The parser uses
    /// these spans as text boundaries but does not emit an element event.
    Ignored,
}

/// A bounded structural event in the original HTML source.
///
/// `start..end` covers the complete markup, including `<` and the terminating
/// `>` when present. Close events also carry the zero-copy name range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TagSpan {
    pub start: usize,
    pub end: usize,
    pub kind: TagKind,
    pub name: Range<usize>,
}

impl TagSpan {
    #[inline]
    pub fn name<'html>(&self, source: &'html [u8]) -> &'html str {
        // SAFETY: input originates from `&str`; the scanner only moves name
        // boundaries over ASCII structural bytes.
        unsafe { std::str::from_utf8_unchecked(&source[self.name.clone()]) }
    }
}

/// The cheap first phase of an opening tag.
///
/// The scalar backend intentionally stops after the name. The parser then
/// chooses exactly one continuation: tokenize attributes when a query needs
/// them, or scan directly to the tag end when it does not. Vector backends may
/// provide an `end_hint` discovered while classifying a larger input block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenTagStart {
    pub start: usize,
    pub name: Range<usize>,
    pub attributes_start: usize,
    pub end_hint: Option<usize>,
}

impl OpenTagStart {
    #[inline]
    pub fn name<'html>(&self, source: &'html [u8]) -> &'html str {
        // SAFETY: input originates from `&str`; the scanner only moves name
        // boundaries over ASCII structural bytes.
        unsafe { std::str::from_utf8_unchecked(&source[self.name.clone()]) }
    }

    #[inline]
    pub fn finish(&self, source: &[u8]) -> usize {
        self.end_hint
            .unwrap_or_else(|| find_unquoted_tag_end(source, self.attributes_start))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TagEvent {
    Open(OpenTagStart),
    Complete(TagSpan),
}

/// Incremental structural scanner used by the streaming parser.
///
/// Backends return one span at a time so callers retain early-exit behavior.
/// A future SWAR/SIMD implementation can cache masks or several spans inside
/// the backend without changing parser or query-executor semantics.
pub(crate) trait TagIndexer {
    fn prepare(&mut self, _source: &[u8]) {}

    fn next(&mut self, source: &[u8], from: usize) -> Option<TagEvent>;

    fn finish_open(&mut self, source: &[u8], open: &OpenTagStart) -> usize {
        open.finish(source)
    }

    fn find_raw_text_close(
        &mut self,
        source: &[u8],
        from: usize,
        close_tag: &str,
    ) -> Option<usize> {
        find_raw_text_close_scalar(source, from, close_tag.as_bytes())
    }
}

/// Scalar reference backend for [`TagIndexer`].
#[derive(Debug, Default)]
pub(crate) struct ScalarTagIndexer;

trait StructuralSearch {
    fn find_byte(&mut self, source: &[u8], from: usize, needle: u8) -> Option<usize>;

    fn find_tag_end(&mut self, source: &[u8], from: usize) -> usize;

    fn find_comment_end(&mut self, source: &[u8], content_start: usize) -> usize {
        if source.get(content_start) == Some(&b'>') {
            return content_start + 1;
        }
        if source.get(content_start..content_start + 2) == Some(b"->") {
            return content_start + 2;
        }

        let mut position = content_start;
        while let Some(gt) = self.find_byte(source, position, b'>') {
            let prefix = &source[..gt];
            if prefix.ends_with(b"--") || prefix.ends_with(b"--!") {
                return gt + 1;
            }
            position = gt + 1;
        }
        source.len()
    }
}

impl StructuralSearch for ScalarTagIndexer {
    #[inline]
    fn find_byte(&mut self, source: &[u8], from: usize, needle: u8) -> Option<usize> {
        source[from..]
            .iter()
            .position(|byte| *byte == needle)
            .map(|offset| from + offset)
    }

    fn find_tag_end(&mut self, source: &[u8], from: usize) -> usize {
        find_unquoted_tag_end(source, from)
    }
}

#[derive(Debug, Default)]
struct MaskCache {
    source_pointer: usize,
    source_len: usize,
    first_block: usize,
    masks: Vec<ClassMasks>,
    valid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexedKind {
    Open,
    Close,
    Ignored,
}

/// Compact semantic event retained in a bounded full-index batch. `u32` keeps
/// each record at 24 bytes; documents larger than 4 GiB fall back to the
/// incremental scanner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IndexedEvent {
    start: u32,
    end: u32,
    name_start: u32,
    name_end: u32,
    attributes_start: u32,
    kind: IndexedKind,
}

impl IndexedEvent {
    fn start(self) -> usize {
        self.start as usize
    }

    fn into_event(self) -> TagEvent {
        match self.kind {
            IndexedKind::Open => TagEvent::Open(OpenTagStart {
                start: self.start as usize,
                name: self.name_start as usize..self.name_end as usize,
                attributes_start: self.attributes_start as usize,
                end_hint: Some(self.end as usize),
            }),
            IndexedKind::Close | IndexedKind::Ignored => TagEvent::Complete(TagSpan {
                start: self.start as usize,
                end: self.end as usize,
                kind: if self.kind == IndexedKind::Close {
                    TagKind::Close
                } else {
                    TagKind::Ignored
                },
                name: self.name_start as usize..self.name_end as usize,
            }),
        }
    }
}

/// Sequential SIMD structural stream used only while constructing the full
/// semantic event tape. It retains one classified source block so consecutive
/// `<`, quote, and `>` searches reuse register-derived masks without writing an
/// intermediate full-document mask array.
struct FusedMaskStream<'source> {
    source: &'source [u8],
    classifier: BlockClassifier,
    cached_base: usize,
    cached_masks: ClassMasks,
    cache_valid: bool,
}

impl<'source> FusedMaskStream<'source> {
    fn new(source: &'source [u8], classifier: BlockClassifier) -> Self {
        Self {
            source,
            classifier,
            cached_base: 0,
            cached_masks: ClassMasks::default(),
            cache_valid: false,
        }
    }

    #[inline]
    fn masks(&mut self, base: usize) -> ClassMasks {
        if !self.cache_valid || self.cached_base != base {
            self.cached_base = base;
            self.cached_masks = self.classifier.classify(self.source, base);
            self.cache_valid = true;
        }
        self.cached_masks
    }

    fn find_mask_matching(
        &mut self,
        from: usize,
        mask_for: impl Fn(ClassMasks) -> u16,
        mut predicate: impl FnMut(u8) -> bool,
    ) -> Option<usize> {
        let mut position = from;
        while position < self.source.len() {
            let base = position & !(SIMD_BLOCK_BYTES - 1);
            if base + SIMD_BLOCK_BYTES > self.source.len() {
                return self.source[position..]
                    .iter()
                    .position(|byte| predicate(*byte))
                    .map(|offset| position + offset);
            }

            let offset = position - base;
            let mut mask = mask_for(self.masks(base)) & (u16::MAX << offset);
            while mask != 0 {
                let lane = mask.trailing_zeros() as usize;
                let candidate = base + lane;
                if predicate(self.source[candidate]) {
                    return Some(candidate);
                }
                mask &= mask - 1;
            }
            position = base + SIMD_BLOCK_BYTES;
        }
        None
    }

    #[inline]
    fn find_less_than(&mut self, from: usize) -> Option<usize> {
        self.find_mask_matching(from, |masks| masks.less_than, |byte| byte == b'<')
    }

    #[inline]
    fn find_greater_than(&mut self, from: usize) -> Option<usize> {
        self.find_mask_matching(from, |masks| masks.tag_end, |byte| byte == b'>')
    }

    fn find_unquoted_tag_end(&mut self, from: usize) -> usize {
        let mut position = from;
        let mut quote = None;

        while let Some(candidate) = self.find_mask_matching(
            position,
            |masks| masks.tag_end,
            |byte| matches!(byte, b'\'' | b'"' | b'>'),
        ) {
            let byte = self.source[candidate];
            match quote {
                Some(delimiter) if byte == delimiter => {
                    let mut backslashes = 0;
                    let mut scan = candidate;
                    while scan > 0 && self.source[scan - 1] == b'\\' {
                        backslashes += 1;
                        scan -= 1;
                    }
                    if backslashes % 2 == 0 {
                        quote = None;
                    }
                }
                Some(_) => {}
                None if matches!(byte, b'\'' | b'"') => quote = Some(byte),
                None => return candidate + 1,
            }
            position = candidate + 1;
        }
        self.source.len()
    }

    fn find_comment_end(&mut self, content_start: usize) -> usize {
        if self.source.get(content_start) == Some(&b'>') {
            return content_start + 1;
        }
        if self.source.get(content_start..content_start + 2) == Some(b"->") {
            return content_start + 2;
        }

        let mut position = content_start;
        while let Some(gt) = self.find_greater_than(position) {
            let prefix = &self.source[..gt];
            if prefix.ends_with(b"--") || prefix.ends_with(b"--!") {
                return gt + 1;
            }
            position = gt + 1;
        }
        self.source.len()
    }

    fn next_event(&mut self, from: usize) -> Option<(IndexedEvent, usize)> {
        let mut from = from;
        loop {
            let start = self.find_less_than(from)?;
            let mut position = start + 1;
            while self
                .source
                .get(position)
                .is_some_and(|&byte| is_html_whitespace(byte) || byte == b'<')
            {
                position += 1;
            }

            let event = match self.source.get(position).copied() {
                Some(b'/') => {
                    let content_start = position + 1;
                    let gt = self
                        .find_greater_than(content_start)
                        .unwrap_or(self.source.len());
                    let mut name_start = content_start;
                    let mut name_end = gt;
                    while name_start < name_end && is_html_whitespace(self.source[name_start]) {
                        name_start += 1;
                    }
                    while name_end > name_start && is_html_whitespace(self.source[name_end - 1]) {
                        name_end -= 1;
                    }
                    IndexedEvent {
                        start: start as u32,
                        end: if gt < self.source.len() {
                            (gt + 1) as u32
                        } else {
                            gt as u32
                        },
                        name_start: name_start as u32,
                        name_end: name_end as u32,
                        attributes_start: 0,
                        kind: IndexedKind::Close,
                    }
                }
                Some(b'!') => {
                    let after_bang = position + 1;
                    let end = if self.source.get(after_bang..after_bang + 2) == Some(b"--") {
                        self.find_comment_end(after_bang + 2)
                    } else {
                        self.find_greater_than(after_bang)
                            .map_or(self.source.len(), |position| position + 1)
                    };
                    IndexedEvent {
                        start: start as u32,
                        end: end as u32,
                        name_start: position as u32,
                        name_end: position as u32,
                        attributes_start: 0,
                        kind: IndexedKind::Ignored,
                    }
                }
                // Match the scalar indexer: a bare or repeated `<` at EOF is
                // incomplete markup, not an opening element. In particular, an
                // empty name violates the element-builder preflight contract.
                None => return None,
                Some(_) => {
                    let name_start = position;
                    while self
                        .source
                        .get(position)
                        .is_some_and(|&byte| !is_name_boundary(byte))
                    {
                        position += 1;
                    }
                    let mut name_end = position;
                    if self.source.get(position) == Some(&b'>')
                        && self.source.get(name_end.wrapping_sub(1)) == Some(&b'/')
                    {
                        name_end -= 1;
                    }
                    if name_start == name_end {
                        from = self.find_unquoted_tag_end(position).max(start + 1);
                        continue;
                    }
                    IndexedEvent {
                        start: start as u32,
                        end: self.find_unquoted_tag_end(position) as u32,
                        name_start: name_start as u32,
                        name_end: name_end as u32,
                        attributes_start: position as u32,
                        kind: IndexedKind::Open,
                    }
                }
            };

            let next = (event.end as usize).max(start + 1);
            return Some((event, next));
        }
    }
}

impl MaskCache {
    fn reset_for(&mut self, source: &[u8]) {
        let source_pointer = source.as_ptr() as usize;
        if !self.valid || self.source_pointer != source_pointer || self.source_len != source.len() {
            self.source_pointer = source_pointer;
            self.source_len = source.len();
            self.first_block = 0;
            self.masks.clear();
            self.valid = true;
        }
    }

    fn refill(
        &mut self,
        classifier: BlockClassifier,
        source: &[u8],
        first_block: usize,
        block_count: usize,
    ) {
        self.reset_for(source);
        self.first_block = first_block;
        self.masks.resize(block_count, ClassMasks::default());
        for (offset, masks) in self.masks.iter_mut().enumerate() {
            *masks = classifier.classify(source, (first_block + offset) * SIMD_BLOCK_BYTES);
        }
    }

    fn contains(&self, block: usize) -> bool {
        self.valid && block >= self.first_block && block - self.first_block < self.masks.len()
    }

    fn get(&self, block: usize) -> ClassMasks {
        self.masks[block - self.first_block]
    }
}

#[derive(Debug)]
pub(crate) struct PackedTagIndexer {
    classifier: BlockClassifier,
    mode: IndexingMode,
    cache: MaskCache,
    full_events: Vec<IndexedEvent>,
    full_cursor: usize,
    full_scan_position: usize,
    full_complete: bool,
    indexed_source: usize,
    indexed_len: usize,
}

impl Default for PackedTagIndexer {
    fn default() -> Self {
        Self::new(IndexingMode::Rolling)
    }
}

impl PackedTagIndexer {
    fn new(mode: IndexingMode) -> Self {
        Self {
            classifier: BlockClassifier::default(),
            mode,
            cache: MaskCache::default(),
            full_events: Vec::new(),
            full_cursor: 0,
            full_scan_position: 0,
            full_complete: false,
            indexed_source: 0,
            indexed_len: 0,
        }
    }

    fn prepare_masks(&mut self, source: &[u8]) {
        self.cache.reset_for(source);
    }

    fn prepare_chunked_index(&mut self, source: &[u8]) {
        let source_pointer = source.as_ptr() as usize;
        if self.indexed_source == source_pointer && self.indexed_len == source.len() {
            return;
        }

        self.full_events.clear();
        self.full_cursor = 0;
        self.full_scan_position = 0;
        self.full_complete = source.is_empty() || source.len() > u32::MAX as usize;
        self.indexed_source = source_pointer;
        self.indexed_len = source.len();
    }

    fn refill_full_events(&mut self, source: &[u8]) {
        debug_assert_eq!(self.mode, IndexingMode::FullDocument);
        debug_assert_eq!(self.indexed_source, source.as_ptr() as usize);
        debug_assert_eq!(self.indexed_len, source.len());
        if self.full_complete {
            return;
        }

        self.full_events.clear();
        self.full_cursor = 0;
        self.full_events.reserve(FULL_INDEX_CHUNK_EVENTS);
        let chunk_end = self
            .full_scan_position
            .saturating_add(FULL_INDEX_CHUNK_BYTES)
            .min(source.len());
        let mut stream = FusedMaskStream::new(source, self.classifier);
        let mut position = self.full_scan_position;
        while let Some((indexed, next)) = stream.next_event(position) {
            self.full_events.push(indexed);
            position = next;
            if position >= chunk_end || self.full_events.len() >= FULL_INDEX_CHUNK_EVENTS {
                break;
            }
        }
        self.full_scan_position = position;
        if self.full_events.is_empty() || position >= source.len() {
            self.full_scan_position = source.len();
            self.full_complete = true;
        }
    }

    fn next_indexed(&mut self, source: &[u8], from: usize) -> Option<TagEvent> {
        loop {
            while self
                .full_events
                .get(self.full_cursor)
                .is_some_and(|event| event.start() < from)
            {
                self.full_cursor += 1;
            }
            if let Some(event) = self.full_events.get(self.full_cursor).copied() {
                return Some(event.into_event());
            }
            if self.full_complete {
                return None;
            }
            self.refill_full_events(source);
        }
    }

    fn find_indexed_raw_text_close(
        &mut self,
        source: &[u8],
        from: usize,
        close_tag: &[u8],
    ) -> Option<usize> {
        // The semantic event stream applies ordinary tag quote rules, which
        // cannot establish the first literal raw-text terminator. Find that
        // boundary from exact `<` candidates before consulting the tape.
        let mut stream = FusedMaskStream::new(source, self.classifier);
        let mut position = from;
        let close = loop {
            let candidate = stream.find_less_than(position)?;
            if raw_text_candidate_matches(source, candidate, close_tag) {
                break candidate;
            }
            position = candidate + 1;
        };

        // Retain the indexed tail only when it contains the exact earliest
        // boundary. A tag-like sequence in raw text may otherwise swallow the
        // close and events immediately after it.
        loop {
            while self
                .full_events
                .get(self.full_cursor)
                .is_some_and(|event| event.start() < close)
            {
                self.full_cursor += 1;
            }
            if let Some(event) = self.full_events.get(self.full_cursor) {
                if event.start() == close {
                    return Some(close);
                }
                break;
            }
            if self.full_complete {
                break;
            }
            self.refill_full_events(source);
        }

        self.full_events.clear();
        self.full_cursor = 0;
        self.full_scan_position = close;
        self.full_complete = false;
        Some(close)
    }

    fn masks(&mut self, source: &[u8], base: usize) -> ClassMasks {
        debug_assert_eq!(base % SIMD_BLOCK_BYTES, 0);
        debug_assert!(base + SIMD_BLOCK_BYTES <= source.len());
        self.prepare_masks(source);
        let block = base / SIMD_BLOCK_BYTES;
        if !self.cache.contains(block) {
            debug_assert!(
                self.mode == IndexingMode::Rolling || source.len() > u32::MAX as usize,
                "full-document indexing only falls back to mask search above the u32 event limit"
            );
            let total_blocks = source.len() / SIMD_BLOCK_BYTES;
            let block_count = ROLLING_WINDOW_BLOCKS.min(total_blocks - block);
            self.cache
                .refill(self.classifier, source, block, block_count);
        }
        self.cache.get(block)
    }

    #[inline]
    fn uses_scalar_prefix(&self) -> bool {
        self.mode == IndexingMode::Rolling
    }

    fn find_mask_matching(
        &mut self,
        source: &[u8],
        from: usize,
        mask_for: impl Fn(ClassMasks) -> u16,
        mut predicate: impl FnMut(u8) -> bool,
    ) -> Option<usize> {
        let mut position = from;
        if self.uses_scalar_prefix() {
            let scalar_end = source
                .len()
                .min(position.saturating_add(SCALAR_SEARCH_PREFIX));
            if let Some(offset) = source[position..scalar_end]
                .iter()
                .position(|byte| predicate(*byte))
            {
                return Some(position + offset);
            }
            position = scalar_end;
        }

        while position < source.len() {
            let base = position & !(SIMD_BLOCK_BYTES - 1);
            if base + SIMD_BLOCK_BYTES > source.len() {
                return source[position..]
                    .iter()
                    .position(|byte| predicate(*byte))
                    .map(|offset| position + offset);
            }

            let offset = position - base;
            let mut mask = mask_for(self.masks(source, base)) & (u16::MAX << offset);
            while mask != 0 {
                let lane = mask.trailing_zeros() as usize;
                let candidate = base + lane;
                if predicate(source[candidate]) {
                    return Some(candidate);
                }
                mask &= mask - 1;
            }
            position = base + SIMD_BLOCK_BYTES;
        }
        None
    }
}

impl PackedTagIndexer {
    fn find_tag_end_matching(
        &mut self,
        source: &[u8],
        from: usize,
        predicate: impl FnMut(u8) -> bool,
    ) -> Option<usize> {
        self.find_mask_matching(source, from, |masks| masks.tag_end, predicate)
    }
}

impl StructuralSearch for PackedTagIndexer {
    fn find_byte(&mut self, source: &[u8], from: usize, needle: u8) -> Option<usize> {
        debug_assert!(matches!(needle, b'<' | b'>' | b'\'' | b'"'));
        if needle == b'<' {
            self.classifier.find_less_than(source, from)
        } else {
            self.find_tag_end_matching(source, from, |byte| byte == needle)
        }
    }

    fn find_tag_end(&mut self, source: &[u8], from: usize) -> usize {
        let mut position = from;
        let mut quote = None;

        while let Some(candidate) =
            self.find_tag_end_matching(source, position, |byte| matches!(byte, b'\'' | b'"' | b'>'))
        {
            let byte = source[candidate];
            match quote {
                Some(delimiter) if byte == delimiter => {
                    let mut backslashes = 0;
                    let mut scan = candidate;
                    while scan > 0 && source[scan - 1] == b'\\' {
                        backslashes += 1;
                        scan -= 1;
                    }
                    if backslashes % 2 == 0 {
                        quote = None;
                    }
                }
                Some(_) => {}
                None if matches!(byte, b'\'' | b'"') => quote = Some(byte),
                None => return candidate + 1,
            }
            position = candidate + 1;
        }

        source.len()
    }
}

#[inline]
fn is_html_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0C | b'\r')
}

#[inline]
fn is_name_boundary(byte: u8) -> bool {
    is_html_whitespace(byte) || matches!(byte, b'\'' | b'"' | b'=' | b'>')
}

fn find_unquoted_tag_end(source: &[u8], mut position: usize) -> usize {
    let mut quote = None;
    let mut backslash_run = 0usize;

    while let Some(&byte) = source.get(position) {
        match quote {
            Some(delimiter) => {
                if byte == b'\\' {
                    backslash_run += 1;
                } else {
                    if byte == delimiter && backslash_run.is_multiple_of(2) {
                        quote = None;
                    }
                    backslash_run = 0;
                }
            }
            None => match byte {
                b'\'' | b'"' => {
                    quote = Some(byte);
                    backslash_run = 0;
                }
                b'>' => return position + 1,
                _ => {}
            },
        }
        position += 1;
    }

    source.len()
}

#[inline]
fn is_raw_text_end_terminator(byte: Option<u8>) -> bool {
    matches!(
        byte,
        None | Some(b' ' | b'\t' | b'\n' | 0x0C | b'\r' | b'/' | b'>')
    )
}

fn raw_text_candidate_matches(source: &[u8], start: usize, close_tag: &[u8]) -> bool {
    let end = start + close_tag.len();
    source
        .get(start..end)
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(close_tag))
        && is_raw_text_end_terminator(source.get(end).copied())
}

fn find_raw_text_close_scalar(source: &[u8], from: usize, close_tag: &[u8]) -> Option<usize> {
    let mut position = from;
    while let Some(offset) = source[position..].iter().position(|byte| *byte == b'<') {
        let candidate = position + offset;
        if raw_text_candidate_matches(source, candidate, close_tag) {
            return Some(candidate);
        }
        position = candidate + 1;
    }
    None
}

fn find_raw_text_close_packed(
    search: &mut impl StructuralSearch,
    source: &[u8],
    from: usize,
    close_tag: &[u8],
) -> Option<usize> {
    let mut position = from;
    while let Some(candidate) = search.find_byte(source, position, b'<') {
        if raw_text_candidate_matches(source, candidate, close_tag) {
            return Some(candidate);
        }
        position = candidate + 1;
    }
    None
}

fn next_event(search: &mut impl StructuralSearch, source: &[u8], from: usize) -> Option<TagEvent> {
    let mut from = from;
    loop {
        let start = search.find_byte(source, from, b'<')?;
        let mut position = start + 1;

        while source
            .get(position)
            .is_some_and(|&byte| is_html_whitespace(byte) || byte == b'<')
        {
            position += 1;
        }

        return match source.get(position).copied() {
            Some(b'/') => {
                let content_start = position + 1;
                let gt = search
                    .find_byte(source, content_start, b'>')
                    .unwrap_or(source.len());
                let mut name_start = content_start;
                let mut name_end = gt;
                while name_start < name_end && is_html_whitespace(source[name_start]) {
                    name_start += 1;
                }
                while name_end > name_start && is_html_whitespace(source[name_end - 1]) {
                    name_end -= 1;
                }
                Some(TagEvent::Complete(TagSpan {
                    start,
                    end: if gt < source.len() { gt + 1 } else { gt },
                    kind: TagKind::Close,
                    name: name_start..name_end,
                }))
            }
            Some(b'!') => {
                let after_bang = position + 1;
                let end = if source.get(after_bang..after_bang + 2) == Some(b"--") {
                    search.find_comment_end(source, after_bang + 2)
                } else {
                    search
                        .find_byte(source, after_bang, b'>')
                        .map_or(source.len(), |position| position + 1)
                };
                Some(TagEvent::Complete(TagSpan {
                    start,
                    end,
                    kind: TagKind::Ignored,
                    name: position..position,
                }))
            }
            // A bare or repeated `<` at EOF is incomplete markup, not an opening
            // element. Returning an empty name would violate the element-builder
            // contract when an attribute query reaches preflight.
            None => None,
            Some(_) => {
                let name_start = position;
                while source
                    .get(position)
                    .is_some_and(|&byte| !is_name_boundary(byte))
                {
                    position += 1;
                }

                let mut name_end = position;
                if source.get(position) == Some(&b'>')
                    && source.get(name_end.wrapping_sub(1)) == Some(&b'/')
                {
                    name_end -= 1;
                }
                if name_start == name_end {
                    from = search.find_tag_end(source, position).max(start + 1);
                    continue;
                }

                Some(TagEvent::Open(OpenTagStart {
                    start,
                    name: name_start..name_end,
                    attributes_start: position,
                    end_hint: None,
                }))
            }
        };
    }
}

impl TagIndexer for ScalarTagIndexer {
    fn next(&mut self, source: &[u8], from: usize) -> Option<TagEvent> {
        next_event(self, source, from)
    }
}

impl TagIndexer for PackedTagIndexer {
    fn prepare(&mut self, source: &[u8]) {
        if self.mode == IndexingMode::FullDocument {
            self.prepare_chunked_index(source);
        } else {
            self.prepare_masks(source);
        }
    }

    fn next(&mut self, source: &[u8], from: usize) -> Option<TagEvent> {
        if self.mode == IndexingMode::FullDocument && source.len() <= u32::MAX as usize {
            self.prepare_chunked_index(source);
            self.next_indexed(source, from)
        } else {
            next_event(self, source, from)
        }
    }

    fn finish_open(&mut self, source: &[u8], open: &OpenTagStart) -> usize {
        open.end_hint
            .unwrap_or_else(|| self.find_tag_end(source, open.attributes_start))
    }

    fn find_raw_text_close(
        &mut self,
        source: &[u8],
        from: usize,
        close_tag: &str,
    ) -> Option<usize> {
        if self.mode == IndexingMode::FullDocument {
            self.find_indexed_raw_text_close(source, from, close_tag.as_bytes())
        } else {
            find_raw_text_close_packed(self, source, from, close_tag.as_bytes())
        }
    }
}

fn find_sampled_tag_end(source: &[u8], mut position: usize, limit: usize) -> Option<usize> {
    let mut quote = None;
    let mut backslash_run = 0;

    while position < limit {
        let byte = source[position];
        if let Some(delimiter) = quote {
            if byte == b'\\' {
                backslash_run += 1;
                position += 1;
                continue;
            }
            if byte == delimiter && backslash_run % 2 == 0 {
                quote = None;
            }
            backslash_run = 0;
        } else {
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'>' => return Some(position + 1),
                _ => {}
            }
        }
        position += 1;
    }
    None
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum SampleLexicalState {
    #[default]
    Normal,
    OversizedTag,
    Unknown,
}

fn sampled_markup_bytes(source: &[u8], start: usize, end: usize) -> (usize, SampleLexicalState) {
    let mut markup_bytes = 0;
    let mut position = if start == 0 {
        0
    } else {
        let anchor_start = start.saturating_sub(FULL_INDEX_MAX_SAMPLED_TAG_BYTES);
        let Some(offset) = source[anchor_start..start]
            .iter()
            .rposition(|&byte| byte == b'<')
        else {
            return (0, SampleLexicalState::Unknown);
        };
        anchor_start + offset
    };

    while position < end {
        let Some(offset) = source[position..end].iter().position(|&byte| byte == b'<') else {
            break;
        };
        let tag_start = position + offset;
        let scan_limit = source
            .len()
            .min(tag_start + FULL_INDEX_MAX_SAMPLED_TAG_BYTES + 1);
        let Some(tag_end) = find_sampled_tag_end(source, tag_start + 1, scan_limit) else {
            return (
                markup_bytes + end.saturating_sub(tag_start),
                SampleLexicalState::OversizedTag,
            );
        };
        markup_bytes += end.min(tag_end).saturating_sub(start.max(tag_start));
        position = tag_end.max(tag_start + 1);
    }

    (markup_bytes, SampleLexicalState::Normal)
}

fn resolve_unknown_sample_start(source: &[u8], start: usize) -> (SampleLexicalState, usize) {
    let probe_end = source
        .len()
        .min(start.saturating_add(FULL_INDEX_MAX_UNKNOWN_PROBE_BYTES));
    let Some(offset) = source[start..probe_end]
        .iter()
        .position(|&byte| matches!(byte, b'<' | b'>'))
    else {
        return (SampleLexicalState::Unknown, probe_end);
    };
    let boundary = start + offset;
    let state = match source[boundary] {
        // A `>` in ordinary text is common. Tail probing handles a quoted
        // tag end separately, where the preceding quote is useful evidence.
        b'>' => SampleLexicalState::Normal,
        // Reaching any `<` first means the sample began in a text run. The
        // bounded parser above handles a tag whose opening delimiter is close
        // enough to establish; an ordinary next tag is not evidence that the
        // preceding text belonged to an oversized tag.
        b'<' => SampleLexicalState::Normal,
        _ => unreachable!(),
    };
    (state, boundary)
}

fn has_quoted_tag_end(source: &[u8], boundary: usize) -> bool {
    let mut position = boundary;
    for _ in 0..16 {
        let Some(&byte) = position.checked_sub(1).and_then(|index| source.get(index)) else {
            return false;
        };
        position -= 1;
        if byte.is_ascii_whitespace() {
            continue;
        }
        return matches!(byte, b'\'' | b'\"');
    }
    false
}

#[derive(Debug, Default, Clone, Copy)]
struct FullIndexWindowSample {
    bytes: usize,
    less_than_count: usize,
    markup_bytes: usize,
    lexical_state: SampleLexicalState,
    // Direct bounded tag parsing and an end-aligned `>` probe are strong
    // enough signals to reject the extra full-index pass immediately.
    decisive_oversized_tag: bool,
}

impl FullIndexWindowSample {
    fn is_dense(self) -> bool {
        self.bytes
            < self
                .less_than_count
                .saturating_mul(FULL_INDEX_MIN_BYTES_PER_TAG)
    }

    fn is_markup_heavy(self) -> bool {
        self.lexical_state == SampleLexicalState::OversizedTag
            || (self.lexical_state == SampleLexicalState::Normal
                && self.markup_bytes * 100 > self.bytes * FULL_INDEX_MAX_MARKUP_PERCENT)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct FullIndexSample {
    windows: [FullIndexWindowSample; FULL_INDEX_SAMPLE_WINDOWS],
    decisive_oversized_tag: bool,
}

impl FullIndexSample {
    fn should_build_full_index(self, attributes_may_be_parsed: bool) -> bool {
        let dense_windows = self
            .windows
            .iter()
            .filter(|window| window.is_dense())
            .count();
        if dense_windows * 2 >= FULL_INDEX_SAMPLE_WINDOWS {
            return false;
        }

        if attributes_may_be_parsed && self.decisive_oversized_tag {
            return false;
        }

        let sampled_tags = self
            .windows
            .iter()
            .map(|window| window.less_than_count)
            .sum::<usize>();
        if sampled_tags < FULL_INDEX_MIN_SAMPLED_TAGS {
            return false;
        }

        !attributes_may_be_parsed
            || self
                .windows
                .iter()
                .filter(|window| window.is_markup_heavy())
                .count()
                * 2
                < FULL_INDEX_SAMPLE_WINDOWS
    }
}

fn sample_full_index_windows(
    source: &[u8],
    classifier: BlockClassifier,
    window_bytes: usize,
    measure_markup: bool,
) -> FullIndexSample {
    let mut sample = FullIndexSample::default();
    let window_stride = source.len() / FULL_INDEX_SAMPLE_WINDOWS;
    let window_capacity = window_bytes.min(window_stride);

    // Sample all four representative quarters. The tail lexical probe below
    // is separate so it cannot replace the 75% shape sample.
    for window in 0..FULL_INDEX_SAMPLE_WINDOWS {
        let start = window_stride * window;
        let available = source.len().saturating_sub(start).min(window_capacity);
        let bytes = available / SIMD_BLOCK_BYTES * SIMD_BLOCK_BYTES;
        let end = start + bytes;

        let current = &mut sample.windows[window];
        current.bytes = bytes;
        current.less_than_count = (start..end)
            .step_by(SIMD_BLOCK_BYTES)
            .map(|block_start| {
                classifier
                    .classify(source, block_start)
                    .less_than
                    .count_ones() as usize
            })
            .sum::<usize>();
        if measure_markup {
            (current.markup_bytes, current.lexical_state) =
                sampled_markup_bytes(source, start, end);
            current.decisive_oversized_tag =
                current.lexical_state == SampleLexicalState::OversizedTag;
            sample.decisive_oversized_tag |= current.decisive_oversized_tag;
            if current.lexical_state == SampleLexicalState::Unknown {
                let (state, boundary) = resolve_unknown_sample_start(source, start);
                current.lexical_state = state;
                current.decisive_oversized_tag = state == SampleLexicalState::OversizedTag
                    || (state == SampleLexicalState::Normal
                        && has_quoted_tag_end(source, boundary));
                sample.decisive_oversized_tag |= current.decisive_oversized_tag;
            }
        }
    }

    if measure_markup {
        let suffix_start = source
            .len()
            .saturating_sub(FULL_INDEX_MAX_UNKNOWN_PROBE_BYTES);
        let tail_end = source[suffix_start..]
            .iter()
            .position(|&byte| byte == b'<')
            .map_or(source.len(), |offset| suffix_start + offset);
        let tail_start = tail_end.saturating_sub(window_capacity);
        if tail_end > tail_start {
            let (_, lexical_state) = sampled_markup_bytes(source, tail_start, tail_end);
            let (resolved_state, boundary) = resolve_unknown_sample_start(source, tail_start);
            let quoted_tag_end = has_quoted_tag_end(source, boundary);
            sample.decisive_oversized_tag |= lexical_state == SampleLexicalState::OversizedTag
                || (lexical_state == SampleLexicalState::Unknown
                    && resolved_state == SampleLexicalState::Normal
                    && quoted_tag_end);
        }
    }

    sample
}

#[derive(Debug)]
pub(crate) struct AutoTagIndexer {
    scalar: ScalarTagIndexer,
    rolling: Option<PackedTagIndexer>,
    full: Option<PackedTagIndexer>,
    allow_full_index: bool,
    force_full_index: bool,
    attributes_may_be_parsed: bool,
    prepared_source: usize,
    prepared_len: usize,
}

impl Default for AutoTagIndexer {
    fn default() -> Self {
        Self::new(IndexingMode::Rolling, false)
    }
}

impl AutoTagIndexer {
    pub(crate) fn new(mode: IndexingMode, attributes_may_be_parsed: bool) -> Self {
        let classifier = BlockClassifier::default();
        let accelerated = classifier.is_accelerated();
        Self {
            scalar: ScalarTagIndexer,
            rolling: accelerated.then(|| PackedTagIndexer::new(IndexingMode::Rolling)),
            full: None,
            allow_full_index: matches!(
                mode,
                IndexingMode::FullDocument | IndexingMode::ForcedFullDocument
            ),
            force_full_index: mode == IndexingMode::ForcedFullDocument,
            attributes_may_be_parsed,
            prepared_source: 0,
            prepared_len: 0,
        }
    }

    fn should_build_full_index(
        source: &[u8],
        classifier: BlockClassifier,
        attributes_may_be_parsed: bool,
    ) -> bool {
        let minimum_bytes = if attributes_may_be_parsed {
            FULL_INDEX_MIN_ATTRIBUTE_BYTES
        } else {
            FULL_INDEX_MIN_BYTES
        };
        if source.len() < minimum_bytes || source.len() > u32::MAX as usize {
            return false;
        }

        let sample_window_bytes = if attributes_may_be_parsed {
            FULL_INDEX_MARKUP_WINDOW_BYTES
        } else {
            FULL_INDEX_DENSITY_WINDOW_BYTES
        };
        let sample = sample_full_index_windows(
            source,
            classifier,
            sample_window_bytes,
            attributes_may_be_parsed,
        );

        sample.should_build_full_index(attributes_may_be_parsed)
    }

    #[cfg(test)]
    fn uses_full_index(&self) -> bool {
        self.full.is_some()
    }

    #[cfg(test)]
    fn rolling_mask_capacity(&self) -> usize {
        self.rolling
            .as_ref()
            .map_or(0, |indexer| indexer.cache.masks.capacity())
    }

    fn prepare_policy(&mut self, source: &[u8]) {
        let source_pointer = source.as_ptr() as usize;
        if self.prepared_source == source_pointer && self.prepared_len == source.len() {
            return;
        }
        self.prepared_source = source_pointer;
        self.prepared_len = source.len();
        self.full = None;

        if self.force_full_index {
            let mut full = PackedTagIndexer::new(IndexingMode::FullDocument);
            full.prepare(source);
            self.full = Some(full);
            return;
        }
        let Some(rolling) = self.rolling.as_ref() else {
            return;
        };
        if self.allow_full_index
            && Self::should_build_full_index(
                source,
                rolling.classifier,
                self.attributes_may_be_parsed,
            )
        {
            let mut full = PackedTagIndexer::new(IndexingMode::FullDocument);
            full.prepare(source);
            self.full = Some(full);
        }
    }
}

impl TagIndexer for AutoTagIndexer {
    fn prepare(&mut self, source: &[u8]) {
        self.prepare_policy(source);
    }

    fn next(&mut self, source: &[u8], from: usize) -> Option<TagEvent> {
        if let Some(indexer) = &mut self.full {
            indexer.next(source, from)
        } else if let Some(indexer) = &mut self.rolling {
            indexer.next(source, from)
        } else {
            self.scalar.next(source, from)
        }
    }

    fn finish_open(&mut self, source: &[u8], open: &OpenTagStart) -> usize {
        if let Some(indexer) = &mut self.full {
            indexer.finish_open(source, open)
        } else if let Some(indexer) = &mut self.rolling {
            indexer.finish_open(source, open)
        } else {
            self.scalar.finish_open(source, open)
        }
    }

    fn find_raw_text_close(
        &mut self,
        source: &[u8],
        from: usize,
        close_tag: &str,
    ) -> Option<usize> {
        if let Some(indexer) = &mut self.full {
            indexer.find_raw_text_close(source, from, close_tag)
        } else if let Some(indexer) = &mut self.rolling {
            indexer.find_raw_text_close(source, from, close_tag)
        } else {
            self.scalar.find_raw_text_close(source, from, close_tag)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(source: &str, from: usize) -> TagEvent {
        ScalarTagIndexer.next(source.as_bytes(), from).unwrap()
    }

    fn assert_packed_matches_scalar(source: &str) {
        let bytes = source.as_bytes();
        for from in 0..=bytes.len() {
            let mut scalar = ScalarTagIndexer;
            let mut packed = PackedTagIndexer::default();
            let scalar_event = scalar.next(bytes, from);
            let packed_event = packed.next(bytes, from);
            assert_eq!(packed_event, scalar_event, "source={source:?}, from={from}");

            if let (Some(TagEvent::Open(scalar_open)), Some(TagEvent::Open(packed_open))) =
                (&scalar_event, &packed_event)
            {
                assert_eq!(
                    packed.finish_open(bytes, packed_open),
                    scalar.finish_open(bytes, scalar_open),
                    "open end differs for source={source:?}, from={from}"
                );
            }
        }
    }

    #[test]
    fn discovers_open_and_close_spans() {
        let html = r#"text <a href=">" class='x'>body</a>"#;
        let TagEvent::Open(open) = next(html, 0) else {
            panic!("expected open tag");
        };
        assert_eq!(open.name(html.as_bytes()), "a");
        let open_end = open.finish(html.as_bytes());
        assert_eq!(&html[open.start..open_end], r#"<a href=">" class='x'>"#);

        let TagEvent::Complete(close) = next(html, open_end) else {
            panic!("expected close tag");
        };
        assert_eq!(close.kind, TagKind::Close);
        assert_eq!(close.name(html.as_bytes()), "a");
        assert_eq!(&html[close.start..close.end], "</a>");
    }

    #[test]
    fn comments_are_single_ignored_spans_even_with_bare_gt() {
        let html = "before<!-- a > b --!><p>after";
        let TagEvent::Complete(comment) = next(html, 0) else {
            panic!("expected ignored span");
        };
        assert_eq!(comment.kind, TagKind::Ignored);
        assert_eq!(&html[comment.start..comment.end], "<!-- a > b --!>");
        let TagEvent::Open(paragraph) = next(html, comment.end) else {
            panic!("expected open tag");
        };
        assert_eq!(paragraph.name(html.as_bytes()), "p");
    }

    #[test]
    fn trims_close_names_and_strips_open_trailing_solidus() {
        let TagEvent::Complete(close) = next("</  div  >", 0) else {
            panic!("expected close tag");
        };
        assert_eq!(close.name("</  div  >".as_bytes()), "div");

        let TagEvent::Open(open) = next("<hr/>", 0) else {
            panic!("expected open tag");
        };
        assert_eq!(open.name("<hr/>".as_bytes()), "hr");
        assert_eq!(open.finish("<hr/>".as_bytes()), 5);
    }

    #[test]
    fn incomplete_open_delimiters_at_eof_are_not_events() {
        for html in ["<", "hello <", "<<<", "hello < <<"] {
            assert_eq!(ScalarTagIndexer.next(html.as_bytes(), 0), None, "{html:?}");

            let mut packed = PackedTagIndexer::new(IndexingMode::FullDocument);
            packed.prepare(html.as_bytes());
            assert_eq!(packed.next(html.as_bytes(), 0), None, "{html:?}");
        }
    }

    #[test]
    fn empty_opening_tags_skip_the_whole_candidate() {
        for source in ["<>", "< >", "<<>>"] {
            assert_eq!(
                ScalarTagIndexer.next(source.as_bytes(), 0),
                None,
                "source={source:?}"
            );
        }

        for source in ["<><p>", "< ><p>", "<<>><p>"] {
            let TagEvent::Open(open) = ScalarTagIndexer.next(source.as_bytes(), 0).unwrap() else {
                panic!("expected opening tag for source={source:?}");
            };
            assert_eq!(open.start, source.rfind('<').unwrap());
            assert_eq!(open.name(source.as_bytes()), "p");
    }

    #[test]
    fn packed_backend_matches_scalar_across_structural_edge_cases() {
        for source in [
            "",
            "plain text without markup",
            "0123456789abcde<a x='y'>body</a>",
            r#"prefix <a title=\"a > b\" data-x='c \\' > d'>body</a> suffix"#,
            "before<!-- a > b --!><p>after",
            "<!doctype html><main><img src=x/><br></main>",
            "<<<  div class=x><span>nested</span></div>",
            "unterminated <tag attr='value",
            "text ending in a bare <",
            "text ending in repeated <<<",
            "close </  div  > after",
            "multibyte é☃ <article data-name='é'>text</article>",
        ] {
            assert_packed_matches_scalar(source);
        }
    }

    #[test]
    fn raw_text_search_skips_near_misses_and_matches_case_insensitively() {
        let html = "x </scripts> <fake> y </ScRiPt >tail";
        let expected = html.find("</ScRiPt").unwrap();
        let mut scalar = ScalarTagIndexer;
        let mut packed = PackedTagIndexer::default();

        assert_eq!(
            scalar.find_raw_text_close(html.as_bytes(), 0, "</script"),
            Some(expected)
        );
        assert_eq!(
            packed.find_raw_text_close(html.as_bytes(), 0, "</script"),
            Some(expected)
        );
    }

    #[test]
    fn raw_text_search_handles_long_content_without_a_close() {
        let html = format!("{}<scripted>", "x < y ".repeat(1_024));
        let mut scalar = ScalarTagIndexer;
        let mut packed = PackedTagIndexer::default();

        assert_eq!(
            scalar.find_raw_text_close(html.as_bytes(), 0, "</script"),
            None
        );
        assert_eq!(
            packed.find_raw_text_close(html.as_bytes(), 0, "</script"),
            None
        );
    }

    #[test]
    fn ordinary_tag_search_does_not_materialize_rolling_masks() {
        let mut packed = PackedTagIndexer::default();
        let short = "short text<a>";
        assert!(matches!(
            packed.next(short.as_bytes(), 0),
            Some(TagEvent::Open(_))
        ));
        assert!(
            packed.cache.masks.is_empty(),
            "short search should remain scalar"
        );

        let long = format!("{}<a>", "x".repeat(SCALAR_SEARCH_PREFIX + 32));
        assert!(matches!(
            packed.next(long.as_bytes(), 0),
            Some(TagEvent::Open(_))
        ));
        assert!(
            packed.cache.masks.is_empty(),
            "ordinary tag search should use the direct less-than classifier"
        );
    }

    #[test]
    fn full_document_mode_reuses_a_bounded_event_batch_without_masks() {
        let source = "<div></div>".repeat(5_000);
        let mut packed = PackedTagIndexer::new(IndexingMode::FullDocument);
        let bytes = source.as_bytes();

        packed.prepare(bytes);
        let mut position = 0;
        let mut event_count = 0;
        let mut max_batch = 0;
        while let Some(event) = packed.next(bytes, position) {
            max_batch = max_batch.max(packed.full_events.len());
            position = match event {
                TagEvent::Open(open) => open.finish(bytes),
                TagEvent::Complete(span) => span.end,
            };
            event_count += 1;
        }

        assert!(packed.cache.masks.is_empty());
        assert_eq!(event_count, 10_000);
        assert_eq!(max_batch, FULL_INDEX_CHUNK_EVENTS);
        assert!(packed.full_events.capacity() >= max_batch);
    }

    #[test]
    fn full_document_raw_text_search_skips_to_close_in_existing_batch() {
        let source = format!("<script>{}</script>", "x".repeat(FULL_INDEX_MIN_BYTES));
        let bytes = source.as_bytes();
        let mut packed = PackedTagIndexer::new(IndexingMode::FullDocument);
        packed.prepare(bytes);

        let Some(TagEvent::Open(script)) = packed.next(bytes, 0) else {
            panic!("expected script opening tag");
        };
        let batch_len = packed.full_events.len();
        let scan_position = packed.full_scan_position;
        let expected_close = source.find("</script");

        assert_eq!(
            packed.find_raw_text_close(bytes, script.finish(bytes), "</script"),
            expected_close
        );
        assert_eq!(packed.full_events.len(), batch_len);
        assert_eq!(packed.full_scan_position, scan_position);
        assert_eq!(
            packed.full_events[packed.full_cursor].start(),
            expected_close.unwrap()
        );
        assert!(packed.cache.masks.is_empty());
    }

    #[test]
    fn full_document_raw_text_search_restarts_events_at_the_real_close() {
        let source = format!(
            "<script>{}const s = \"<div data='unterminated\";</script><span>real</span>",
            "x".repeat(FULL_INDEX_MIN_BYTES)
        );
        let bytes = source.as_bytes();
        let mut packed = PackedTagIndexer::new(IndexingMode::FullDocument);
        packed.prepare(bytes);

        let Some(TagEvent::Open(script)) = packed.next(bytes, 0) else {
            panic!("expected script opening tag");
        };
        assert_eq!(script.name(bytes), "script");

        let close = packed
            .find_raw_text_close(bytes, script.finish(bytes), "</script")
            .expect("real script close should remain discoverable");
        let after_close = source[close..].find('>').unwrap() + close + 1;
        let Some(TagEvent::Open(span)) = packed.next(bytes, after_close) else {
            panic!("expected opening tag after raw text");
        };
        assert_eq!(span.name(bytes), "span");
        assert!(packed.cache.masks.is_empty());
    }

    #[test]
    fn full_document_raw_text_search_recovers_a_close_swallowed_by_quotes() {
        let source = format!(
            "<script>{}const s = \"<div data='unterminated\";</script><span>real</span>",
            "x".repeat(FULL_INDEX_MIN_BYTES)
        );
        let bytes = source.as_bytes();
        let mut packed = PackedTagIndexer::new(IndexingMode::FullDocument);
        packed.prepare(bytes);

        let Some(TagEvent::Open(script)) = packed.next(bytes, 0) else {
            panic!("expected script opening tag");
        };
        let close = packed
            .find_raw_text_close(bytes, script.finish(bytes), "</script")
            .expect("real script close should remain discoverable");
        let after_close = source[close..].find('>').unwrap() + close + 1;
        let Some(TagEvent::Open(span)) = packed.next(bytes, after_close) else {
            panic!("expected opening tag after raw text");
        };
        assert_eq!(span.name(bytes), "span");
    }

    #[test]
    fn full_document_raw_text_search_uses_the_earliest_literal_close() {
        let source = format!(
            "<script>{}<div data=\"</script><span>first</span>\"></script><span>second</span>",
            "x".repeat(FULL_INDEX_MIN_BYTES)
        );
        let bytes = source.as_bytes();
        let mut packed = PackedTagIndexer::new(IndexingMode::FullDocument);
        packed.prepare(bytes);

        let Some(TagEvent::Open(script)) = packed.next(bytes, 0) else {
            panic!("expected script opening tag");
        };
        let close = packed
            .find_raw_text_close(bytes, script.finish(bytes), "</script")
            .expect("first literal script close should be found");

        assert_eq!(close, source.find("</script>").unwrap());
        let after_close = source[close..].find('>').unwrap() + close + 1;
        let Some(TagEvent::Open(span)) = packed.next(bytes, after_close) else {
            panic!("expected first span after the raw close");
        };
        assert_eq!(span.name(bytes), "span");
        assert_eq!(&source[span.start..span.finish(bytes)], "<span>");
    }

    #[test]
    fn fused_full_document_events_match_the_scalar_indexer() {
        for source in [
            "plain text without markup",
            r#"<div data='a>b'><span title="x>y"></span></div>"#,
            "before<!-- a > b --!><p>after",
            "<!doctype html><main><img src='/x>y'></main>",
            "<?xml version='1.0'?><a href=\"/x\"></a>",
            r#"<div data="escaped\" > quote"></div>"#,
            "<<<  div class=x><span>nested</span></div>",
            "unterminated <tag attr='value",
            "text ending in a bare <",
            "text ending in repeated <<<",
            "close </  div  > after",
            "multibyte é☃ <article data-name='é'>text</article>",
        ] {
            let bytes = source.as_bytes();
            let mut scalar = ScalarTagIndexer;
            let mut expected = Vec::new();
            let mut position = 0;
            while let Some(event) = scalar.next(bytes, position) {
                let indexed = match event {
                    TagEvent::Open(open) => {
                        let end = scalar.finish_open(bytes, &open);
                        position = end.max(open.start + 1);
                        IndexedEvent {
                            start: open.start as u32,
                            end: end as u32,
                            name_start: open.name.start as u32,
                            name_end: open.name.end as u32,
                            attributes_start: open.attributes_start as u32,
                            kind: IndexedKind::Open,
                        }
                    }
                    TagEvent::Complete(span) => {
                        position = span.end.max(span.start + 1);
                        IndexedEvent {
                            start: span.start as u32,
                            end: span.end as u32,
                            name_start: span.name.start as u32,
                            name_end: span.name.end as u32,
                            attributes_start: 0,
                            kind: if span.kind == TagKind::Close {
                                IndexedKind::Close
                            } else {
                                IndexedKind::Ignored
                            },
                        }
                    }
                };
                expected.push(indexed);
                if position >= bytes.len() {
                    break;
                }
            }

            let mut packed = PackedTagIndexer::new(IndexingMode::FullDocument);
            packed.prepare(bytes);
            let mut actual = Vec::new();
            let mut position = 0;
            while let Some(event) = packed.next(bytes, position) {
                let indexed = match event {
                    TagEvent::Open(open) => {
                        let end = packed.finish_open(bytes, &open);
                        position = end.max(open.start + 1);
                        IndexedEvent {
                            start: open.start as u32,
                            end: end as u32,
                            name_start: open.name.start as u32,
                            name_end: open.name.end as u32,
                            attributes_start: open.attributes_start as u32,
                            kind: IndexedKind::Open,
                        }
                    }
                    TagEvent::Complete(span) => {
                        position = span.end.max(span.start + 1);
                        IndexedEvent {
                            start: span.start as u32,
                            end: span.end as u32,
                            name_start: span.name.start as u32,
                            name_end: span.name.end as u32,
                            attributes_start: 0,
                            kind: if span.kind == TagKind::Close {
                                IndexedKind::Close
                            } else {
                                IndexedKind::Ignored
                            },
                        }
                    }
                };
                actual.push(indexed);
                if position >= bytes.len() {
                    break;
                }
            }
            assert_eq!(actual, expected, "source={source:?}");
        }
    }

    #[test]
    fn fused_full_index_skips_large_empty_candidate_once() {
        let source = format!("{}><p>", "<".repeat(16_384));
        let bytes = source.as_bytes();
        let mut packed = PackedTagIndexer::new(IndexingMode::FullDocument);
        packed.prepare(bytes);

        let TagEvent::Open(open) = packed.next(bytes, 0).unwrap() else {
            panic!("expected opening tag after malformed delimiter run");
        };
        assert_eq!(open.start, source.len() - 3);
        assert_eq!(open.name(bytes), "p");
    }

    #[test]
    fn rolling_mode_overwrites_and_reuses_its_bounded_mask_buffer() {
        let source = "x".repeat((ROLLING_WINDOW_BLOCKS * 2 + 4) * SIMD_BLOCK_BYTES);
        let mut packed = PackedTagIndexer::new(IndexingMode::Rolling);
        let first = packed.masks(source.as_bytes(), 0);
        let allocation = packed.cache.masks.as_ptr();

        let next_block = ROLLING_WINDOW_BLOCKS + 1;
        let second = packed.masks(source.as_bytes(), next_block * SIMD_BLOCK_BYTES);

        assert_eq!(first, second);
        assert_eq!(packed.cache.first_block, next_block);
        assert_eq!(packed.cache.masks.as_ptr(), allocation);
        assert_eq!(packed.cache.masks.len(), ROLLING_WINDOW_BLOCKS);
    }

    #[test]
    fn adaptive_policy_keeps_dense_documents_scalar() {
        let source = "<div><span>x</span></div>".repeat(1_000);
        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, false);

        indexer.prepare(source.as_bytes());

        assert!(!indexer.uses_full_index());
    }

    #[test]
    fn adaptive_policy_keeps_sparse_exhaustive_documents_rolling() {
        let source = format!("<main>{}</main>", "x".repeat(FULL_INDEX_MIN_BYTES));
        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, false);

        indexer.prepare(source.as_bytes());

        assert!(!indexer.uses_full_index());
    }

    #[test]
    fn adaptive_policy_separates_density_at_the_crossover() {
        fn source(padding: usize) -> String {
            let filler = "x".repeat(padding);
            format!("<span>{filler}</span>").repeat(5_000)
        }

        let dense = source(120);
        let sparse = source(122);
        let mut dense_indexer = AutoTagIndexer::new(IndexingMode::FullDocument, false);
        let mut sparse_indexer = AutoTagIndexer::new(IndexingMode::FullDocument, false);

        dense_indexer.prepare(dense.as_bytes());
        sparse_indexer.prepare(sparse.as_bytes());

        assert!(!dense_indexer.uses_full_index());
        assert!(sparse_indexer.uses_full_index());
    }

    #[test]
    fn adaptive_policy_keeps_long_attribute_spans_scalar_when_attributes_matter() {
        let value = "x".repeat(256);
        let source = format!(
            "<a class=\"hit\" href=\"/item\" data-padding=\"{value}\" data-x=\"yes\">x</a>"
        )
        .repeat(5_000);
        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, true);

        indexer.prepare(source.as_bytes());

        assert!(!indexer.uses_full_index());
    }

    #[test]
    fn adaptive_policy_keeps_a_tag_spanning_sample_windows_scalar() {
        let value = "x".repeat(48 * 1024);
        let source = format!(
            "<main>{}<a data-padding=\"{value}\" data-x=\"yes\">x</a></main>",
            "x".repeat(8 * 1024)
        );
        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, true);

        indexer.prepare(source.as_bytes());

        assert!(!indexer.uses_full_index());
    }

    #[test]
    fn adaptive_policy_detects_misaligned_large_tag_above_attribute_cutoff() {
        let value = "x".repeat(256 * 1024);
        let source = format!(
            "<main>{}<a data-padding=\"{value}\" data-x=\"yes\">x</a></main>",
            "x".repeat(8 * 1024)
        );
        assert!(source.len() > FULL_INDEX_MIN_ATTRIBUTE_BYTES);
        let sample = sample_full_index_windows(
            source.as_bytes(),
            BlockClassifier::default(),
            FULL_INDEX_MARKUP_WINDOW_BYTES,
            true,
        );
        assert!(sample.decisive_oversized_tag);

        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, true);

        indexer.prepare(source.as_bytes());

        assert!(!indexer.uses_full_index());
    }

    #[test]
    fn adaptive_policy_full_indexes_text_gaps_when_attributes_matter() {
        let text = "x".repeat(256);
        let source =
            format!("<a class=\"hit\" href=\"/item\" data-x=\"yes\">{text}</a>").repeat(5_000);
        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, true);

        indexer.prepare(source.as_bytes());

        assert!(indexer.uses_full_index());
    }

    #[test]
    fn unknown_sample_resolution_stops_at_its_byte_budget() {
        let start = 128;
        let source = vec![b'x'; start + FULL_INDEX_MAX_UNKNOWN_PROBE_BYTES * 2];

        let (state, boundary) = resolve_unknown_sample_start(&source, start);

        assert_eq!(state, SampleLexicalState::Unknown);
        assert_eq!(boundary, start + FULL_INDEX_MAX_UNKNOWN_PROBE_BYTES);
    }

    #[test]
    fn unknown_sample_before_an_ordinary_tag_is_a_text_run() {
        let start = 128;
        let source = format!("{}<div data-x>text</div>", "x".repeat(start + 256));

        let (state, boundary) = resolve_unknown_sample_start(source.as_bytes(), start);

        assert_eq!(state, SampleLexicalState::Normal);
        assert_eq!(boundary, source.find('<').unwrap());
    }

    #[test]
    fn adaptive_policy_does_not_mistake_repeated_text_gaps_for_oversized_tags() {
        let unit = format!("<p data-x='1'>a</p>{}", "y".repeat(400));
        let source = unit.repeat(512);
        let classifier = BlockClassifier::default();
        let sample = sample_full_index_windows(
            source.as_bytes(),
            classifier,
            FULL_INDEX_MARKUP_WINDOW_BYTES,
            true,
        );

        assert!(!sample.decisive_oversized_tag);

        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, true);
        indexer.prepare(source.as_bytes());
        if classifier.is_accelerated() {
            assert!(indexer.uses_full_index());
        }
    }

    #[test]
    fn adaptive_policy_keeps_a_single_large_text_gap_scalar() {
        let text = "x".repeat(1024 * 1024);
        let source = format!("<a data-x=\"yes\">{text}</a>");
        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, true);

        indexer.prepare(source.as_bytes());

        assert!(!indexer.uses_full_index());
    }

    #[test]
    fn forced_full_mode_bypasses_adaptive_policy() {
        let text = "x".repeat(1024 * 1024);
        let source = format!("<a data-x=\"yes\">{text}</a>");
        let mut indexer = AutoTagIndexer::new(IndexingMode::ForcedFullDocument, true);

        indexer.prepare(source.as_bytes());

        assert!(indexer.uses_full_index());
    }

    #[test]
    fn adaptive_policy_keeps_small_attribute_sensitive_text_gaps_scalar() {
        let text = "x".repeat(64 * 1024);
        let source = format!("<a data-x=\"yes\">{text}</a>");
        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, true);

        indexer.prepare(source.as_bytes());

        assert!(!indexer.uses_full_index());
    }

    #[test]
    fn adaptive_policy_does_not_treat_text_gt_before_close_as_oversized_tag() {
        let mut text = "x".repeat(1024 * 1024);
        let position = text.len() - 512;
        text.replace_range(position..position + 1, ">");
        let source = format!("<a data-x=\"yes\">{text}</a>");
        let sample = sample_full_index_windows(
            source.as_bytes(),
            BlockClassifier::default(),
            FULL_INDEX_MARKUP_WINDOW_BYTES,
            true,
        );
        assert!(!sample.decisive_oversized_tag);
        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, true);

        indexer.prepare(source.as_bytes());

        assert!(!indexer.uses_full_index());
    }

    #[test]
    fn adaptive_policy_full_indexes_multi_kib_text_gaps_when_attributes_matter() {
        let text = "x".repeat(4 * 1024);
        let source =
            format!("<a class=\"hit\" href=\"/item\" data-x=\"yes\">{text}</a>").repeat(32);
        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, true);

        indexer.prepare(source.as_bytes());

        assert!(indexer.uses_full_index());
    }

    #[test]
    fn adaptive_policy_samples_attribute_heavy_body_after_sparse_script() {
        let value = "x".repeat(256);
        let body = format!(
            "<a class=\"hit\" href=\"/item\" data-padding=\"{value}\" data-x=\"yes\">x</a>"
        )
        .repeat(5_000);
        let source = format!("<script>{}</script>{body}", "x".repeat(8 * 1024));
        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, true);

        indexer.prepare(source.as_bytes());

        assert!(!indexer.uses_full_index());
    }

    #[test]
    fn adaptive_policy_samples_text_gap_body_after_sparse_script() {
        let text = "x".repeat(256);
        let body =
            format!("<a class=\"hit\" href=\"/item\" data-x=\"yes\">{text}</a>").repeat(5_000);
        let source = format!("<script>{}</script>{body}", "x".repeat(8 * 1024));
        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, true);

        indexer.prepare(source.as_bytes());

        assert!(indexer.uses_full_index());
    }

    #[test]
    fn adaptive_policy_does_not_let_dense_head_override_sparse_body() {
        let text = "x".repeat(256);
        let body =
            format!("<a class=\"hit\" href=\"/item\" data-x=\"yes\">{text}</a>").repeat(5_000);
        let source = format!("{}{body}", "<div></div>".repeat(750));
        let mut indexer = AutoTagIndexer::new(IndexingMode::FullDocument, true);

        indexer.prepare(source.as_bytes());

        assert!(indexer.uses_full_index());
    }

    #[test]
    fn early_exit_raw_text_search_avoids_a_rolling_mask_buffer() {
        let source = format!("{} </script>", "x".repeat(FULL_INDEX_MIN_BYTES));
        let expected = source.find("</script>").unwrap();
        let mut indexer = AutoTagIndexer::new(IndexingMode::Rolling, false);

        indexer.prepare(source.as_bytes());
        assert!(!indexer.uses_full_index());
        assert_eq!(indexer.rolling_mask_capacity(), 0);
        assert_eq!(
            indexer.find_raw_text_close(source.as_bytes(), 0, "</script"),
            Some(expected)
        );
        assert_eq!(indexer.rolling_mask_capacity(), 0);
    }
}
