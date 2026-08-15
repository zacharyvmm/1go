use std::ops::Range;

use super::simd_classifier::{BlockClassifier, ClassMasks};

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
pub(crate) struct PackedTagIndexer {
    classifier: BlockClassifier,
    cached_source: usize,
    cached_len: usize,
    cached_base: usize,
    cached_masks: ClassMasks,
    cache_valid: bool,
}

impl PackedTagIndexer {
    fn masks(&mut self, source: &[u8], base: usize) -> ClassMasks {
        let source_pointer = source.as_ptr() as usize;
        if self.cache_valid
            && self.cached_source == source_pointer
            && self.cached_len == source.len()
            && self.cached_base == base
        {
            return self.cached_masks;
        }

        let masks = self.classifier.classify(source, base);
        self.cached_source = source_pointer;
        self.cached_len = source.len();
        self.cached_base = base;
        self.cached_masks = masks;
        self.cache_valid = true;
        masks
    }

    fn find_structural_matching(
        &mut self,
        source: &[u8],
        from: usize,
        mut predicate: impl FnMut(u8) -> bool,
    ) -> Option<usize> {
        let mut position = from;
        while position < source.len() {
            let base = position & !15;
            if base + 16 > source.len() {
                return source[position..]
                    .iter()
                    .position(|byte| predicate(*byte))
                    .map(|offset| position + offset);
            }

            let offset = position - base;
            let mut mask = self.masks(source, base).structural & (u16::MAX << offset);
            while mask != 0 {
                let lane = mask.trailing_zeros() as usize;
                let candidate = base + lane;
                if predicate(source[candidate]) {
                    return Some(candidate);
                }
                mask &= mask - 1;
            }
            position = base + 16;
        }
        None
    }
}

impl StructuralSearch for PackedTagIndexer {
    fn find_byte(&mut self, source: &[u8], from: usize, needle: u8) -> Option<usize> {
        self.find_structural_matching(source, from, |byte| byte == needle)
    }

    fn find_tag_end(&mut self, source: &[u8], from: usize) -> usize {
        let mut position = from;
        let mut quote = None;

        while let Some(candidate) = self
            .find_structural_matching(source, position, |byte| matches!(byte, b'\'' | b'"' | b'>'))
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

#[derive(Debug)]
enum AutoBackend {
    Scalar(ScalarTagIndexer),
    Packed(PackedTagIndexer),
}

#[derive(Debug)]
pub(crate) struct AutoTagIndexer {
    backend: AutoBackend,
}

impl Default for AutoTagIndexer {
    fn default() -> Self {
        let packed = PackedTagIndexer::default();
        let backend = if packed.classifier.is_accelerated() {
            AutoBackend::Packed(packed)
        } else {
            AutoBackend::Scalar(ScalarTagIndexer)
        };
        Self { backend }
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
    let start = search.find_byte(source, from, b'<')?;
    let mut position = start + 1;

    // Match the legacy parser's tolerance for whitespace and repeated
    // `<` bytes before a tag name.
    while source
        .get(position)
        .is_some_and(|&byte| is_html_whitespace(byte) || byte == b'<')
    {
        position += 1;
    }

    match source.get(position).copied() {
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

            Some(TagEvent::Open(OpenTagStart {
                start,
                name: name_start..name_end,
                attributes_start: position,
                end_hint: None,
            }))
        }
    }
}

impl TagIndexer for ScalarTagIndexer {
    fn next(&mut self, source: &[u8], from: usize) -> Option<TagEvent> {
        next_event(self, source, from)
    }
}

impl TagIndexer for PackedTagIndexer {
    fn next(&mut self, source: &[u8], from: usize) -> Option<TagEvent> {
        next_event(self, source, from)
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
        find_raw_text_close_packed(self, source, from, close_tag.as_bytes())
    }
}

impl TagIndexer for AutoTagIndexer {
    fn next(&mut self, source: &[u8], from: usize) -> Option<TagEvent> {
        match &mut self.backend {
            AutoBackend::Scalar(indexer) => indexer.next(source, from),
            AutoBackend::Packed(indexer) => indexer.next(source, from),
        }
    }

    fn finish_open(&mut self, source: &[u8], open: &OpenTagStart) -> usize {
        match &mut self.backend {
            AutoBackend::Scalar(indexer) => indexer.finish_open(source, open),
            AutoBackend::Packed(indexer) => indexer.finish_open(source, open),
        }
    }

    fn find_raw_text_close(
        &mut self,
        source: &[u8],
        from: usize,
        close_tag: &str,
    ) -> Option<usize> {
        match &mut self.backend {
            AutoBackend::Scalar(indexer) => indexer.find_raw_text_close(source, from, close_tag),
            AutoBackend::Packed(indexer) => indexer.find_raw_text_close(source, from, close_tag),
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
}
