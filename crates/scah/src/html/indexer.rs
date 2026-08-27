use std::ops::Range;

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
}

/// Scalar reference backend for [`TagIndexer`].
#[derive(Debug, Default)]
pub(crate) struct ScalarTagIndexer;

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

fn find_comment_end(source: &[u8], content_start: usize) -> usize {
    // Preserve the abrupt-close forms handled by the existing parser.
    if source.get(content_start) == Some(&b'>') {
        return content_start + 1;
    }
    if source.get(content_start..content_start + 2) == Some(b"->") {
        return content_start + 2;
    }

    let mut position = content_start;
    while let Some(offset) = source[position..].iter().position(|&byte| byte == b'>') {
        let gt = position + offset;
        let prefix = &source[..gt];
        if prefix.ends_with(b"--") || prefix.ends_with(b"--!") {
            return gt + 1;
        }
        position = gt + 1;
    }
    source.len()
}

impl TagIndexer for ScalarTagIndexer {
    fn next(&mut self, source: &[u8], from: usize) -> Option<TagEvent> {
        let relative_start = source.get(from..)?.iter().position(|&byte| byte == b'<')?;
        let start = from + relative_start;
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
                let gt = source[content_start..]
                    .iter()
                    .position(|&byte| byte == b'>')
                    .map_or(source.len(), |offset| content_start + offset);
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
                    find_comment_end(source, after_bang + 2)
                } else {
                    source[after_bang..]
                        .iter()
                        .position(|&byte| byte == b'>')
                        .map_or(source.len(), |offset| after_bang + offset + 1)
                };
                Some(TagEvent::Complete(TagSpan {
                    start,
                    end,
                    kind: TagKind::Ignored,
                    name: position..position,
                }))
            }
            None => Some(TagEvent::Open(OpenTagStart {
                start,
                name: source.len()..source.len(),
                attributes_start: source.len(),
                end_hint: Some(source.len()),
            })),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(source: &str, from: usize) -> TagEvent {
        ScalarTagIndexer.next(source.as_bytes(), from).unwrap()
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
}
