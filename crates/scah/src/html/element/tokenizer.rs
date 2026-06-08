use crate::Reader;
use scah_reader::{BoundaryHit, BoundaryKind};

#[derive(Debug, PartialEq)]
pub enum ElementAttributeToken<'a> {
    String(&'a str),
    Equal,
}

const DOUBLEQUOTE: u8 = b'"';
const SINGLEQUOTE: u8 = b'\'';
const EQUAL: u8 = b'=';
const END_OF_ELEMENT: u8 = b'>';
const UNQUOTED_BOUNDARIES: [u8; 9] = [
    b' ',
    b'\t',
    b'\n',
    b'\r',
    b'\x0C',
    DOUBLEQUOTE,
    SINGLEQUOTE,
    EQUAL,
    END_OF_ELEMENT,
];

/// Precomputed 256-byte LUT: `UNQUOTED_BOUNDARY_LUT[b as usize]` is `true`
/// iff `b` is one of the 9 boundary characters.  Eliminates the linear
/// scan of `UNQUOTED_BOUNDARIES.contains(b)` in the hot path — replaces
/// ~4 comparisons/byte with a single L1-cache lookup.
static UNQUOTED_BOUNDARY_LUT: [bool; 256] = {
    let mut lut = [false; 256];
    let mut i = 0;
    while i < UNQUOTED_BOUNDARIES.len() {
        lut[UNQUOTED_BOUNDARIES[i] as usize] = true;
        i += 1;
    }
    lut
};

impl<'a> ElementAttributeToken<'a> {
    pub fn next(reader: &mut Reader<'a>) -> Option<Self> {
        reader.skip_whitespace();

        let start_pos = reader.get_position();

        let first_byte = reader.peek()?;

        match first_byte {
            DOUBLEQUOTE => {
                reader.skip(); // skip opening quote
                let content_start = reader.get_position();
                // Use SIMD-accelerated search for the closing quote
                reader.next_until(DOUBLEQUOTE);
                let content = reader.slice(content_start..reader.get_position());
                reader.skip(); // skip closing quote
                Some(Self::String(content))
            }
            SINGLEQUOTE => {
                reader.skip(); // skip opening quote
                let content_start = reader.get_position();
                reader.next_until(SINGLEQUOTE);
                let content = reader.slice(content_start..reader.get_position());
                reader.skip(); // skip closing quote
                Some(Self::String(content))
            }
            EQUAL => {
                reader.skip();
                Some(Self::Equal)
            }
            END_OF_ELEMENT => {
                reader.skip(); // consume '>' so the parser doesn't re-read it
                None
            }
            _ if reader.has_simd_attribute_boundary() => {
                Self::scan_unquoted_token_simd(reader, start_pos)
            }
            _ => Self::scan_unquoted_token_scalar(reader, start_pos),
        }
    }

    /// Scan an unquoted attribute name or value using the LUT-accelerated fast path.
    ///
    /// Replaces the `next_until_list` / `contains()` byte-at-a-time linear scan
    /// with a single 256-byte LUT lookup per byte — ~4× faster for typical
    /// attribute tokens (3–15 bytes).
    #[inline]
    fn scan_unquoted_token_scalar(reader: &mut Reader<'a>, start_pos: usize) -> Option<Self> {
        let source = reader.source_bytes();
        let len = source.len();
        let mut pos = start_pos;
        while pos < len && !UNQUOTED_BOUNDARY_LUT[source[pos] as usize] {
            pos += 1;
        }
        reader.set_position(pos);
        if start_pos >= pos {
            None
        } else {
            Some(Self::String(reader.slice(start_pos..pos)))
        }
    }

    /// Scan an unquoted attribute name or value using SIMD boundary detection.
    /// Returns the token spanning from `start_pos` to the first boundary character.
    #[inline]
    fn scan_unquoted_token_simd(reader: &mut Reader<'a>, start_pos: usize) -> Option<Self> {
        // Use SIMD-accelerated boundary finding from the reader.
        // This processes 32 bytes at a time via AVX2 (or falls back to scalar).
        match reader.find_attribute_boundary() {
            Some(hit) => {
                let BoundaryHit { position, kind } = hit;
                // Position the reader at the boundary character
                reader.set_position(position);

                let token = reader.slice(start_pos..position);

                match kind {
                    BoundaryKind::Gt => {
                        // '>' terminates the element - don't consume it.
                        // The caller (XHtmlTag/XHtmlElement) will handle it.
                        if token.is_empty() {
                            None
                        } else {
                            Some(Self::String(token))
                        }
                    }
                    BoundaryKind::Equals => {
                        // '=' is a separate token; don't consume it here.
                        // Return the string before it, then the next call will get Equal.
                        if token.is_empty() {
                            // The token starts with '=', so return Equal directly
                            reader.skip(); // consume '='
                            Some(Self::Equal)
                        } else {
                            Some(Self::String(token))
                        }
                    }
                    _ => {
                        // Quote or whitespace: the token ends here.
                        if token.is_empty() {
                            // Boundary at current position (e.g., leading whitespace
                            // that wasn't skipped, or a quote). Re-dispatch.
                            Self::next(reader)
                        } else {
                            Some(Self::String(token))
                        }
                    }
                }
            }
            None => {
                // No boundary found - the rest of input is the token
                let end = reader.source_bytes().len();
                reader.set_position(end);
                if start_pos >= end {
                    None
                } else {
                    Some(Self::String(reader.slice(start_pos..end)))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_attribute_iterator() {
        let string: String = String::from("key=\"value\"");

        let mut reader = Reader::new(&string);

        let mut next_iter = ElementAttributeToken::next(&mut reader);
        assert!(next_iter.is_some());

        let mut next_value = next_iter.unwrap();

        assert_eq!(next_value, ElementAttributeToken::String("key"));

        next_iter = ElementAttributeToken::next(&mut reader);
        assert!(next_iter.is_some());

        next_value = next_iter.unwrap();
        assert_eq!(next_value, ElementAttributeToken::Equal);

        next_iter = ElementAttributeToken::next(&mut reader);
        assert!(next_iter.is_some());

        next_value = next_iter.unwrap();
        assert_eq!(next_value, ElementAttributeToken::String("value"));
    }

    #[test]
    fn unquoted_attribute_value() {
        let mut reader = Reader::new("key=value");
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::String("key"))
        );
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::Equal)
        );
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::String("value"))
        );
    }

    #[test]
    fn multiple_unquoted_attributes() {
        let mut reader = Reader::new("key1 val1 key2=val2");
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::String("key1"))
        );
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::String("val1"))
        );
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::String("key2"))
        );
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::Equal)
        );
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::String("val2"))
        );
    }

    #[test]
    fn single_quoted_attribute() {
        let mut reader = Reader::new("key='value'");
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::String("key"))
        );
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::Equal)
        );
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::String("value"))
        );
    }

    #[test]
    fn terminates_at_gt() {
        let mut reader = Reader::new("key>");
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::String("key"))
        );
        // Next call should return None because we're at '>'
        assert_eq!(ElementAttributeToken::next(&mut reader), None);
    }

    #[test]
    fn empty_at_gt() {
        let mut reader = Reader::new(">");
        assert_eq!(ElementAttributeToken::next(&mut reader), None);
    }

    #[test]
    fn whitespace_separated_tokens() {
        let mut reader = Reader::new("  key  =  \"value\"  ");
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::String("key"))
        );
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::Equal)
        );
        assert_eq!(
            ElementAttributeToken::next(&mut reader),
            Some(ElementAttributeToken::String("value"))
        );
    }

    // TOKENIZER / FSM attribute robustness tests
    // TODO: key="value's" <-- `'` should be part of the string
    // TODO: k'ey="value" <-- `'` should be part of the string
    // TODO: key="v"alue" <-- parsed as `key="v"` and `alue"` which is equal to true.
}
