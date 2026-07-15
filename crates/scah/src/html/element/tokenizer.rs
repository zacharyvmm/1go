use crate::Reader;

#[derive(Debug, PartialEq)]
pub enum ElementAttributeToken<'a> {
    String(&'a str),
    Equal,
}

const DOUBLEQUOTE: u8 = b'"';
const SINGLEQUOTE: u8 = b'\'';
const EQUAL: u8 = b'=';
const BACKSLASH: u8 = b'\\';
const END_OF_ELEMENT: u8 = b'>';

/// HTML whitespace: space, tab, line feed, form feed (U+000C), carriage return.
const WHITESPACE: [u8; 5] = [b' ', b'\t', b'\n', 0x0C, b'\r'];

/// Characters that terminate an unquoted attribute name/value token: HTML
/// whitespace, quotes, `=`, and the end-of-tag `>`.
const WORD_BOUNDARIES: [u8; 9] = [
    b' ',
    b'\t',
    b'\n',
    0x0C,
    b'\r',
    DOUBLEQUOTE,
    SINGLEQUOTE,
    EQUAL,
    END_OF_ELEMENT,
];

impl<'a> ElementAttributeToken<'a> {
    /// Read the next attribute token.
    ///
    /// `in_value` must be `true` when the caller is expecting an unquoted
    /// attribute *value* (i.e. the previous token was `=`). In HTML a `/` is
    /// only the self-closing marker in tag/attribute-*name* position; inside an
    /// unquoted value every `/` is a literal character (e.g. `data=/foo/`), so
    /// the trailing-solidus handling below is skipped when `in_value` is set.
    pub fn next(reader: &mut Reader<'a>, in_value: bool) -> Option<Self> {
        reader.next_while_list(&WHITESPACE);

        let start_pos = reader.get_position();

        match reader.next()? {
            DOUBLEQUOTE => {
                let star_position = reader.get_position();
                reader.next_until_unescaped(DOUBLEQUOTE, BACKSLASH);
                let content_inside_quotes = reader.slice(star_position..reader.get_position());
                reader.skip();

                Some(Self::String(content_inside_quotes))
            }
            SINGLEQUOTE => {
                let star_position = reader.get_position();
                reader.next_until_unescaped(SINGLEQUOTE, BACKSLASH);
                let content_inside_quotes = reader.slice(star_position..reader.get_position());
                reader.skip();

                Some(Self::String(content_inside_quotes))
            }
            EQUAL => Some(Self::Equal),
            END_OF_ELEMENT => None,
            _ => {
                // Find end of word
                reader.next_until_list(&WORD_BOUNDARIES);
                let word = reader.slice(start_pos..reader.get_position());

                // A trailing solidus immediately before `>` is the HTML
                // self-closing marker (`<hr/>`, `<input disabled/>`), but only
                // in name position. It must not glue to the tag name nor leak
                // as an attribute. A `/` in value position (`data=/foo/`) or
                // anywhere else is left untouched.
                if !in_value
                    && reader.peek() == Some(END_OF_ELEMENT)
                    && let Some(stripped) = word.strip_suffix('/')
                {
                    if stripped.is_empty() {
                        // Bare `/>`; consume the `>` and end the tag.
                        reader.skip();
                        return None;
                    }
                    return Some(Self::String(stripped));
                }

                Some(Self::String(word))
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

        let mut next_iter = ElementAttributeToken::next(&mut reader, false);
        assert!(next_iter.is_some());

        let mut next_value = next_iter.unwrap();

        assert_eq!(next_value, ElementAttributeToken::String("key"));

        next_iter = ElementAttributeToken::next(&mut reader, false);
        assert!(next_iter.is_some());

        next_value = next_iter.unwrap();
        assert_eq!(next_value, ElementAttributeToken::Equal);

        next_iter = ElementAttributeToken::next(&mut reader, true);
        assert!(next_iter.is_some());

        next_value = next_iter.unwrap();
        assert_eq!(next_value, ElementAttributeToken::String("value"));
    }

    // TOKENIZER / FSM attribute robustness tests
    // TODO: key="value's" <-- `'` should be part of the string
    // TODO: k'ey="value" <-- `'` should be part of the string
    // TODO: key="v"alue" <-- parsed as `key="v"` and `alue"` which is equal to true.
}
