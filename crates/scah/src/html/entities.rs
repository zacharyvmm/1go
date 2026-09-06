//! HTML data-state character-reference decoding.

#[path = "entities_table.rs"]
mod entities_table;

use entities_table::{
    ENTITY_NAME_ENDS, ENTITY_NAMES, ENTITY_VALUE_ENDS, ENTITY_VALUES, MAX_NAME_LEN,
    NAMED_ENTITY_COUNT,
};

// Derive a small search index from the generated, sorted names. Searching
// only names with the same initial byte avoids unrelated table comparisons.
const SEARCH_INDEX: ([u16; 129], usize) = build_search_index();

const fn build_search_index() -> ([u16; 129], usize) {
    let mut starts = [0; 129];
    let mut index = 0;
    let mut initial = 0;
    let mut longest_legacy_name = 0;
    while initial < starts.len() {
        while index < NAMED_ENTITY_COUNT {
            let start = if index == 0 {
                0
            } else {
                ENTITY_NAME_ENDS[index - 1] as usize
            };
            if ENTITY_NAMES[start] as usize >= initial {
                break;
            }
            let end = ENTITY_NAME_ENDS[index] as usize;
            if ENTITY_NAMES[end - 1] != b';' && end - start > longest_legacy_name {
                longest_legacy_name = end - start;
            }
            index += 1;
        }
        starts[initial] = index as u16;
        initial += 1;
    }
    (starts, longest_legacy_name)
}

#[inline]
fn entity_name(index: usize) -> &'static [u8] {
    let start = if index == 0 {
        0
    } else {
        ENTITY_NAME_ENDS[index - 1] as usize
    };
    let end = ENTITY_NAME_ENDS[index] as usize;
    &ENTITY_NAMES[start..end]
}

#[inline]
fn entity_value(index: usize) -> &'static [u8] {
    let start = if index == 0 {
        0
    } else {
        ENTITY_VALUE_ENDS[index - 1] as usize
    };
    let end = ENTITY_VALUE_ENDS[index] as usize;
    &ENTITY_VALUES[start..end]
}

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
#[cold]
#[inline(never)]
pub(crate) fn decode_character_references(source: &str, out: &mut Vec<u8>) {
    decode_character_references_impl::<false>(source, out);
}

/// Decode character references after applying HTML source-newline normalization.
///
/// Only CR and CRLF bytes copied from `source` become LF. A CR produced by a
/// numeric character reference remains CR because character references are
/// resolved after source preprocessing.
#[cold]
#[inline(never)]
pub(crate) fn decode_character_references_normalizing_source_newlines(
    source: &str,
    out: &mut Vec<u8>,
) {
    decode_character_references_impl::<true>(source, out);
}

fn decode_character_references_impl<const NORMALIZE_SOURCE_NEWLINES: bool>(
    source: &str,
    out: &mut Vec<u8>,
) {
    let bytes = source.as_bytes();
    let mut copied_through = 0;

    while let Some(relative_ampersand) = bytes[copied_through..]
        .iter()
        .position(|&byte| byte == b'&')
    {
        let ampersand = copied_through + relative_ampersand;
        append_source::<NORMALIZE_SOURCE_NEWLINES>(&bytes[copied_through..ampersand], out);

        match decode_after_ampersand(&bytes[ampersand + 1..], out) {
            Some(consumed) => copied_through = ampersand + 1 + consumed,
            None => {
                out.push(b'&');
                copied_through = ampersand + 1;
            }
        }
    }

    append_source::<NORMALIZE_SOURCE_NEWLINES>(&bytes[copied_through..], out);
}

#[inline]
fn append_source<const NORMALIZE_SOURCE_NEWLINES: bool>(source: &[u8], out: &mut Vec<u8>) {
    if !NORMALIZE_SOURCE_NEWLINES || !source.contains(&b'\r') {
        out.extend_from_slice(source);
        return;
    }

    let mut copied_through = 0;
    while let Some(relative_cr) = source[copied_through..]
        .iter()
        .position(|&byte| byte == b'\r')
    {
        let cr = copied_through + relative_cr;
        out.extend_from_slice(&source[copied_through..cr]);
        out.push(b'\n');
        copied_through = cr + 1;
        if source.get(copied_through) == Some(&b'\n') {
            copied_through += 1;
        }
    }
    out.extend_from_slice(&source[copied_through..]);
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
    // Escaped HTML most often uses these five complete spellings. Resolve
    // them before the full-table search. Requiring the semicolon leaves
    // longest-prefix and legacy semicolon-less matching to the general path.
    let common: Option<(&[u8], usize)> = match source {
        [b'a', b'm', b'p', b';', ..] => Some((b"&", 4)),
        [b'l', b't', b';', ..] => Some((b"<", 3)),
        [b'g', b't', b';', ..] => Some((b">", 3)),
        [b'q', b'u', b'o', b't', b';', ..] => Some((b"\"", 5)),
        [b'n', b'b', b's', b'p', b';', ..] => Some(("\u{00A0}".as_bytes(), 5)),
        _ => None,
    };
    if let Some((value, consumed)) = common {
        out.extend_from_slice(value);
        return Some(consumed);
    }

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

    let candidate_end = if name_end < MAX_NAME_LEN && source.get(name_end) == Some(&b';') {
        name_end + 1
    } else {
        name_end
    };

    if let Ok(index) = binary_search_entity_name(&source[..candidate_end]) {
        out.extend_from_slice(entity_value(index));
        return Some(candidate_end);
    }

    // A shorter match cannot include the terminal semicolon. Only legacy
    // semicolon-less spellings can succeed, and none exceed this length.
    let mut candidate_end = (candidate_end - 1).min(SEARCH_INDEX.1);
    while candidate_end > 0 {
        let candidate = &source[..candidate_end];
        if let Ok(index) = binary_search_entity_name(candidate) {
            out.extend_from_slice(entity_value(index));
            return Some(candidate_end);
        }
        candidate_end -= 1;
    }

    None
}

#[inline]
fn binary_search_entity_name(candidate: &[u8]) -> Result<usize, usize> {
    let initial = candidate[0] as usize;
    let mut left = SEARCH_INDEX.0[initial] as usize;
    let mut right = SEARCH_INDEX.0[initial + 1] as usize;
    while left < right {
        let mid = left + (right - left) / 2;
        match entity_name(mid).cmp(candidate) {
            std::cmp::Ordering::Less => left = mid + 1,
            std::cmp::Ordering::Greater => right = mid,
            std::cmp::Ordering::Equal => return Ok(mid),
        }
    }
    Err(left)
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
    use super::{
        contains_ampersand, decode_character_references,
        decode_character_references_normalizing_source_newlines,
    };

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
    fn source_newline_normalization_preserves_reference_cr() {
        let mut output = Vec::new();
        decode_character_references_normalizing_source_newlines(
            "\r\n&amp;\r&#13;&#10;",
            &mut output,
        );
        assert_eq!(String::from_utf8(output).unwrap(), "\n&\n\r\n");
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
    fn every_named_reference_matches_the_generated_value() {
        for index in 0..super::NAMED_ENTITY_COUNT {
            let name = std::str::from_utf8(super::entity_name(index)).unwrap();
            let value = std::str::from_utf8(super::entity_value(index)).unwrap();
            assert_eq!(decode(&format!("&{name}")), value, "{name}");
            assert_eq!(
                decode(&format!("&{name}!é")),
                format!("{value}!é"),
                "{name}"
            );
        }
        assert_eq!(
            decode("&amp;&amper;&AMP;&Amp;&notin;&notit;"),
            "&&er;&&Amp;∉¬it;"
        );
    }

    #[test]
    fn named_lookup_preserves_longest_prefix_for_extended_and_truncated_names() {
        for index in 0..super::NAMED_ENTITY_COUNT {
            let name = std::str::from_utf8(super::entity_name(index)).unwrap();
            for source in [
                format!("{name}letters;"),
                format!("{}invalid;", name.trim_end_matches(';')),
                name[..name.len() - 1].to_owned(),
            ] {
                let expected = (0..super::NAMED_ENTITY_COUNT)
                    .filter(|&candidate| {
                        source.as_bytes().starts_with(super::entity_name(candidate))
                    })
                    .max_by_key(|&candidate| super::entity_name(candidate).len());
                let mut output = Vec::new();
                let consumed = super::decode_named(source.as_bytes(), &mut output);
                assert_eq!(
                    consumed,
                    expected.map(|candidate| super::entity_name(candidate).len()),
                    "{source}"
                );
                assert_eq!(
                    output.as_slice(),
                    expected.map(super::entity_value).unwrap_or_default(),
                    "{source}"
                );
            }
        }
    }

    #[test]
    fn appends_to_existing_output() {
        let mut output = b"prefix: ".to_vec();
        decode_character_references("&lt;x&gt;", &mut output);
        assert_eq!(output, b"prefix: <x>");
    }
}
