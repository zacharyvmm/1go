//! HTML data-state character-reference decoding.

#[path = "entities_table.rs"]
mod entities_table;

use entities_table::{MAX_NAME_LEN, NAMED_ENTITIES};

/// Return whether `source` might need character-reference decoding.
#[inline]
pub(crate) fn contains_ampersand(source: &str) -> bool {
    source.as_bytes().contains(&b'&')
}

/// Decode HTML character references in `source` into `out`.
///
/// Decoded UTF-8 bytes are appended to `out`. Callers should use
/// [`contains_ampersand`] to fast-path fragments that cannot contain a
/// character reference.
pub(crate) fn decode_character_references(source: &str, out: &mut Vec<u8>) {
    let bytes = source.as_bytes();
    let mut copied_through = 0;

    while let Some(relative_ampersand) = bytes[copied_through..]
        .iter()
        .position(|&byte| byte == b'&')
    {
        let ampersand = copied_through + relative_ampersand;
        out.extend_from_slice(&bytes[copied_through..ampersand]);

        match decode_after_ampersand(&bytes[ampersand + 1..], out) {
            Some(consumed) => copied_through = ampersand + 1 + consumed,
            None => {
                out.push(b'&');
                copied_through = ampersand + 1;
            }
        }
    }

    out.extend_from_slice(&bytes[copied_through..]);
}

/// Decode the bytes following an ampersand and return their consumed length.
fn decode_after_ampersand(source: &[u8], out: &mut Vec<u8>) -> Option<usize> {
    if source.first() == Some(&b'#') {
        decode_numeric(source, out)
    } else {
        decode_named(source, out)
    }
}

fn decode_numeric(source: &[u8], out: &mut Vec<u8>) -> Option<usize> {
    let mut cursor = 1;
    let radix = if matches!(source.get(cursor), Some(b'x' | b'X')) {
        cursor += 1;
        16
    } else {
        10
    };
    let digits_start = cursor;
    let mut value = 0_u64;

    while let Some(&byte) = source.get(cursor) {
        let Some(digit) = digit_value(byte, radix) else {
            break;
        };
        value = value
            .saturating_mul(u64::from(radix))
            .saturating_add(u64::from(digit));
        cursor += 1;
    }

    if cursor == digits_start {
        return None;
    }
    if source.get(cursor) == Some(&b';') {
        cursor += 1;
    }

    append_code_point(normalize_numeric_reference(value), out);
    Some(cursor)
}

#[inline]
fn digit_value(byte: u8, radix: u32) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some(u32::from(byte - b'0')),
        b'a'..=b'f' if radix == 16 => Some(u32::from(byte - b'a') + 10),
        b'A'..=b'F' if radix == 16 => Some(u32::from(byte - b'A') + 10),
        _ => None,
    }
}

fn decode_named(source: &[u8], out: &mut Vec<u8>) -> Option<usize> {
    let mut name_end = 0;
    while name_end < source.len()
        && name_end < MAX_NAME_LEN
        && source[name_end].is_ascii_alphanumeric()
    {
        name_end += 1;
    }

    if name_end == 0 {
        return None;
    }

    let mut candidate_end = if name_end < MAX_NAME_LEN && source.get(name_end) == Some(&b';') {
        name_end + 1
    } else {
        name_end
    };

    while candidate_end > 0 {
        let candidate = &source[..candidate_end];
        if let Ok(index) =
            NAMED_ENTITIES.binary_search_by(|(name, _)| name.as_bytes().cmp(candidate))
        {
            out.extend_from_slice(NAMED_ENTITIES[index].1.as_bytes());
            return Some(candidate_end);
        }
        candidate_end -= 1;
    }

    None
}

fn normalize_numeric_reference(value: u64) -> char {
    let value = match value {
        0 | 0xD800..=0xDFFF | 0x110000.. => return '\u{FFFD}',
        0x80 => 0x20AC,
        0x82 => 0x201A,
        0x83 => 0x0192,
        0x84 => 0x201E,
        0x85 => 0x2026,
        0x86 => 0x2020,
        0x87 => 0x2021,
        0x88 => 0x02C6,
        0x89 => 0x2030,
        0x8A => 0x0160,
        0x8B => 0x2039,
        0x8C => 0x0152,
        0x8E => 0x017D,
        0x91 => 0x2018,
        0x92 => 0x2019,
        0x93 => 0x201C,
        0x94 => 0x201D,
        0x95 => 0x2022,
        0x96 => 0x2013,
        0x97 => 0x2014,
        0x98 => 0x02DC,
        0x99 => 0x2122,
        0x9A => 0x0161,
        0x9B => 0x203A,
        0x9C => 0x0153,
        0x9E => 0x017E,
        0x9F => 0x0178,
        value => value,
    };

    // The invalid scalar values returned above have already been handled.
    char::from_u32(value as u32).unwrap_or('\u{FFFD}')
}

#[inline]
fn append_code_point(character: char, out: &mut Vec<u8>) {
    let mut encoded = [0; 4];
    out.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
}

#[cfg(test)]
mod tests {
    use super::{contains_ampersand, decode_character_references};

    fn decode(source: &str) -> String {
        let mut output = Vec::new();
        decode_character_references(source, &mut output);
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn detects_ampersands() {
        assert!(!contains_ampersand("plain text"));
        assert!(contains_ampersand("one &amp; two"));
    }

    #[test]
    fn decodes_named_references_with_semicolons() {
        assert_eq!(
            decode("&amp;&nbsp;&lt;&gt;&quot;&copy;&NotEqualTilde;"),
            "&\u{00A0}<>\"©\u{2242}\u{0338}"
        );
    }

    #[test]
    fn decodes_legacy_named_references_without_semicolons_in_data_state() {
        assert_eq!(decode("&amp"), "&");
        assert_eq!(decode("&copy test"), "© test");
        assert_eq!(decode("&ampersand"), "&ersand");
    }

    #[test]
    fn decodes_decimal_and_hex_numeric_references() {
        assert_eq!(decode("&#65; &#65 &#x41; &#X41"), "A A A A");
    }

    #[test]
    fn replaces_invalid_numeric_scalar_values() {
        assert_eq!(
            decode("&#0; &#xD800; &#x110000;"),
            "\u{FFFD} \u{FFFD} \u{FFFD}"
        );
    }

    #[test]
    fn remaps_windows_1252_numeric_references() {
        let remapped = [
            (0x80, '\u{20AC}'),
            (0x82, '\u{201A}'),
            (0x83, '\u{0192}'),
            (0x84, '\u{201E}'),
            (0x85, '\u{2026}'),
            (0x86, '\u{2020}'),
            (0x87, '\u{2021}'),
            (0x88, '\u{02C6}'),
            (0x89, '\u{2030}'),
            (0x8A, '\u{0160}'),
            (0x8B, '\u{2039}'),
            (0x8C, '\u{0152}'),
            (0x8E, '\u{017D}'),
            (0x91, '\u{2018}'),
            (0x92, '\u{2019}'),
            (0x93, '\u{201C}'),
            (0x94, '\u{201D}'),
            (0x95, '\u{2022}'),
            (0x96, '\u{2013}'),
            (0x97, '\u{2014}'),
            (0x98, '\u{02DC}'),
            (0x99, '\u{2122}'),
            (0x9A, '\u{0161}'),
            (0x9B, '\u{203A}'),
            (0x9C, '\u{0153}'),
            (0x9E, '\u{017E}'),
            (0x9F, '\u{0178}'),
        ];
        for (source, expected) in remapped {
            assert_eq!(decode(&format!("&#{source};")), expected.to_string());
        }
        for unchanged in [0x81_u32, 0x8D, 0x8F, 0x90, 0x9D] {
            assert_eq!(
                decode(&format!("&#{unchanged};")),
                char::from_u32(unchanged).unwrap().to_string()
            );
        }
    }

    #[test]
    fn preserves_malformed_references_without_skipping_input() {
        assert_eq!(
            decode("&unknown; & &#; &#x; &#X; end"),
            "&unknown; & &#; &#x; &#X; end"
        );
    }

    #[test]
    fn appends_to_existing_output() {
        let mut output = b"prefix: ".to_vec();
        decode_character_references("&lt;x&gt;", &mut output);
        assert_eq!(output, b"prefix: <x>");
    }
}
