//! Shared tag-level utilities used by both the streaming parser (full
//! tokenization) and the lazy parser (tag-name filtering before full parse).
//!
//! These operate on raw `&[u8]` for zero-copy performance — no UTF-8
//! validation, no Reader overhead.

use scah_reader::simd::find_tag_open_scalar;
use std::ops::Range;

/// Information about a tag extracted from its raw byte representation.
#[derive(Clone)]
pub(crate) struct TagInfo {
    /// Byte range of the tag name within the input (e.g., `"div"`)
    pub name: Range<usize>,
    /// `true` if this is a closing tag (`</tagname>`)
    pub is_close: bool,
    /// `true` if this is a self-closing tag (`<br/>` or `<br />`)
    pub is_self_closing: bool,
}

/// Find the next `<` in the input starting from `start`.
///
/// Uses SIMD-accelerated scalar search (`memchr`). Returns `None` if no
/// more tags are found.
#[inline]
pub(crate) fn find_next_tag(input: &[u8], start: usize) -> Option<usize> {
    let position = find_tag_open_scalar(input, start);
    (position < input.len()).then_some(position)
}

/// Find the closing `>` of a tag starting at `position` (immediately after `<`).
///
/// Quote-aware: ignores `>` inside quoted attribute values (`"..."` or `'...'`).
/// Handles HTML comments (`<!-- -->`) by scanning for `-->`.
/// Returns `None` if EOF is reached before finding `>`.
pub(crate) fn find_tag_end(input: &[u8], mut position: usize) -> Option<usize> {
    // Quick check for comments: <!-- ... -->
    if position + 3 < input.len()
        && input[position] == b'!'
        && input[position + 1] == b'-'
        && input[position + 2] == b'-'
    {
        // Scan for -->
        let mut pos = position + 3;
        while pos + 2 < input.len() {
            if input[pos] == b'-' && input[pos + 1] == b'-' && input[pos + 2] == b'>' {
                return Some(pos + 2);
            }
            pos += 1;
        }
        return None;
    }

    let mut quote = None;
    while position < input.len() {
        match (input[position], quote) {
            (b'"', None) => quote = Some(b'"'),
            (b'\'', None) => quote = Some(b'\''),
            (byte, Some(open_quote)) if byte == open_quote => quote = None,
            (b'>', None) => return Some(position),
            _ => {}
        }
        position += 1;
    }
    None
}

/// Extract the tag name, close-ness, and self-closing status from a tag.
///
/// `tag_start` is the position of `<`, `tag_end` is the position of `>`.
/// Returns `None` for comments (`<!...>`) and doctypes (`<?...>`).
pub(crate) fn tag_info(input: &[u8], tag_start: usize, tag_end: usize) -> Option<TagInfo> {
    let mut position = tag_start + 1;
    // Skip leading whitespace after `<`
    while position < tag_end
        && (input[position] == b'<'
            || matches!(
                input[position],
                b' ' | b'\t' | b'\n' | b'\r' | b'\x0C'
            ))
    {
        position += 1;
    }

    // Reject comments and processing instructions
    if position >= tag_end || matches!(input[position], b'!' | b'?') {
        return None;
    }

    let is_close = input[position] == b'/';
    if is_close {
        position += 1;
        // Skip whitespace after `/`
        while position < tag_end
            && matches!(
                input[position],
                b' ' | b'\t' | b'\n' | b'\r' | b'\x0C'
            )
        {
            position += 1;
        }
    }

    let name_start = position;
    while position < tag_end {
        match input[position] {
            b' ' | b'\t' | b'\n' | b'\r' | b'\x0C' | b'/' => break,
            _ => position += 1,
        }
    }

    if position == name_start {
        return None; // empty tag name
    }

    // Check for self-closing: `<tagname/>` or `<tagname />`
    let mut before_end = tag_end;
    while before_end > name_start
        && matches!(
            input[before_end - 1],
            b' ' | b'\t' | b'\n' | b'\r' | b'\x0C'
        )
    {
        before_end -= 1;
    }

    Some(TagInfo {
        name: name_start..position,
        is_close,
        is_self_closing: !is_close && before_end > name_start && input[before_end - 1] == b'/',
    })
}

/// Check if a tag name equals the given string (byte comparison, case-sensitive).
#[inline]
pub(crate) fn tag_name_eq(input: &[u8], range: &Range<usize>, tag: &str) -> bool {
    input[range.clone()] == *tag.as_bytes()
}

/// Check if a tag name equals the given string (case-insensitive).
#[inline]
pub(crate) fn tag_name_eq_ignore_ascii_case(input: &[u8], range: &Range<usize>, tag: &str) -> bool {
    input[range.clone()].eq_ignore_ascii_case(tag.as_bytes())
}

/// Returns `true` if the tag name is a raw-text element
/// (script, style, textarea, title).
#[inline]
pub(crate) fn is_raw_text_tag(input: &[u8], range: &Range<usize>) -> bool {
    let name = &input[range.clone()];
    matches!(
        name.to_ascii_lowercase().as_slice(),
        b"script" | b"style" | b"textarea" | b"title"
    )
}

/// Convert a tag name byte range to a `&str` (no UTF-8 validation —
/// assumes the input is valid HTML/ASCII tag names).
#[inline]
pub(crate) fn tag_name_str<'a>(input: &'a [u8], range: &Range<usize>) -> &'a str {
    unsafe { std::str::from_utf8_unchecked(&input[range.clone()]) }
}
