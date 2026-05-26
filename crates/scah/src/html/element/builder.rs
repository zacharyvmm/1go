use super::tokenizer::ElementAttributeToken;
use crate::Reader;
use crate::html::tape::CompactAttrEntry;
use scah_query_ir::{Attribute, IElement};

/// A key-value pair representing an HTML element attribute.
///
/// Both `key` and `value` are zero-copy `&str` references into the
/// original HTML source.
///
/// # Example
///
/// ```rust
/// use scah::{Query, Save, parse};
///
/// let html = r#"<a href="https://example.com" target="_blank">Link</a>"#;
/// let queries = &[Query::all("a", Save::all())
///     .expect("valid selector")
///     .build()];
/// let store = parse(html, queries);
///
/// let a = store.get("a").unwrap().next().unwrap();
/// let attrs = a.attributes(&store).unwrap();
/// assert_eq!(attrs[0].key, "href");
/// assert_eq!(attrs[0].value, Some("https://example.com"));
/// assert_eq!(attrs[1].key, "target");
/// assert_eq!(attrs[1].value, Some("_blank"));
/// ```
/// An HTML element as parsed from the token stream.
///
/// This is the *parser-level* representation used during streaming.
/// Once an element is matched by a query, its data is copied into an
/// [`Element`](crate::Element) inside the [`Store`](crate::Store).
#[derive(Debug, PartialEq, Clone, Default)]
pub struct XHtmlElement<'html> {
    /// The tag name (e.g. `"div"`, `"a"`, `"section"`).
    pub name: &'html str,
    /// The value of the `id` attribute, if present.
    pub id: Option<&'html str>,
    /// The value of the `class` attribute, if present.
    pub class: Option<&'html str>,
    /// Slice of additional attributes (excludes `id` and `class`).
    pub attributes: &'html [Attribute<'html>],
}

#[derive(Debug, PartialEq)]
pub enum XHtmlTag<'html> {
    Open,
    Close(&'html str),
}

impl<'html> XHtmlElement<'html> {
    fn add_to_element(
        &mut self,
        attribute: Attribute<'html>,
        attribute_tape: &mut Vec<Attribute<'html>>,
    ) {
        if self.name.is_empty() && attribute.value.is_none() {
            self.name = attribute.key;
        } else if self.class.is_none() && attribute.key == "class" && attribute.value.is_some() {
            self.class = attribute.value;
        } else if self.id.is_none() && attribute.key == "id" && attribute.value.is_some() {
            self.id = attribute.value;
        } else {
            attribute_tape.push(attribute);
        }
    }

    pub fn is_self_closing(&self) -> bool {
        // Use SWAR-based tag name check for all HTML void elements.
        // This compares 4 bytes at a time using bit tricks instead of逐个 character comparison.
        if scah_reader::simd::is_self_closing_tag(self.name.as_bytes()) {
            return true;
        }
        if let Some(last_attribute) = self.attributes.last() {
            return last_attribute.key == "\\";
        }

        false
    }

    pub fn clear(&mut self) {
        self.name = "";
        self.id = None;
        self.class = None;
        self.attributes = &[];
    }

    /*
     * When a Element is parsed all the Attributes are added to a Tape
     * If the Element was not saved, then we need to delete these Attributes
     */
    pub fn remove_attributes(&self, attribute_tape: &mut Vec<Attribute<'html>>) {
        if self.attributes.is_empty() {
            return;
        }
        let tape_ptr = attribute_tape.as_ptr();
        let attr_range_ptr = self.attributes.as_ptr();
        let idx = unsafe { attr_range_ptr.offset_from_unsigned(tape_ptr) };

        attribute_tape.truncate(idx);
    }

    pub fn from(&mut self, reader: &mut Reader<'html>, attribute_tape: &mut Vec<Attribute<'html>>) {
        let mut assign = false;
        let mut key = None;
        let start_len = attribute_tape.len();

        while let Some(token) = ElementAttributeToken::next(reader) {
            match token {
                ElementAttributeToken::String(string_value) => match key {
                    None => {
                        debug_assert!(!assign);
                        key = Some(string_value);
                    }
                    Some(k) => {
                        if assign {
                            self.add_to_element(
                                Attribute {
                                    key: k,
                                    value: Some(string_value),
                                },
                                attribute_tape,
                            );
                            key = None;
                        } else {
                            self.add_to_element(
                                Attribute {
                                    key: k,
                                    value: None,
                                },
                                attribute_tape,
                            );
                            key = Some(string_value)
                        }
                        assign = false;
                    }
                },

                ElementAttributeToken::Equal => {
                    assign = true;
                }
            }
        }

        if let Some(attribute) = key {
            self.add_to_element(
                Attribute {
                    key: attribute,
                    value: None,
                },
                attribute_tape,
            );
        }

        // Since we are
        //  1) assigning after adding the Attributes
        //  and 2) either transforming it into a Range in Store or removing them
        //  their is no risk when doing this unsafely
        self.attributes = unsafe {
            std::slice::from_raw_parts(
                attribute_tape.as_ptr().add(start_len),
                attribute_tape.len() - start_len,
            )
        };
    }

    /// Build an XHtmlElement from pre-tokenized tape entries.
    ///
    /// This slices attributes directly from the compact attribute entries
    /// in the tape without re-scanning through the Reader+tokenizer.
    ///
    /// # Arguments
    /// * `tape_entry` - The tape entry for this tag
    /// * `source` - The HTML source bytes
    /// * `compact_attrs` - Slice of pre-tokenized compact attribute entries
    /// * `attr_range` - Range of attribute indices for this tag
    /// * `attribute_tape` - Mutable reference to store the resulting Attribute entries
    pub fn from_tape(
        &mut self,
        tape_entry: &crate::html::tape::TapeEntry,
        source: &'html [u8],
        compact_attrs: &[CompactAttrEntry],
        attr_range: std::ops::Range<usize>,
        attribute_tape: &mut Vec<Attribute<'html>>,
    ) {
        let start_len = attribute_tape.len();
        let tag_slice = tape_entry.slice(source);
        let tag_bytes = tag_slice.as_bytes();

        // Parse tag name from the tag slice
        // Skip '<' and optional '/'
        let mut name_start = 1;
        if name_start < tag_bytes.len() && tag_bytes[name_start] == b'/' {
            name_start += 1;
        }

        // Find end of tag name
        let mut name_end = name_start;
        while name_end < tag_bytes.len() {
            match tag_bytes[name_end] {
                b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/' => break,
                _ => name_end += 1,
            }
        }

        // Set the tag name (adjust offsets relative to source)
        let name_offset = tape_entry.offset as usize + name_start;
        let name_len = name_end - name_start;
        if name_len > 0 {
            self.name = unsafe {
                std::str::from_utf8_unchecked(&source[name_offset..name_offset + name_len])
            };
        }

        // Process compact attributes
        for &compact_attr in &compact_attrs[attr_range] {
            // Copy fields to avoid issues with packed struct references
            let key_offset = compact_attr.key_offset as usize;
            let key_length = compact_attr.key_length as usize;
            let key: &str = unsafe {
                std::str::from_utf8_unchecked(&source[key_offset..key_offset + key_length])
            };
            let value: Option<&str> = if compact_attr.has_value() {
                let val_offset = compact_attr.value_offset as usize;
                let val_length = compact_attr.value_length as usize;
                Some(unsafe {
                    std::str::from_utf8_unchecked(&source[val_offset..val_offset + val_length])
                })
            } else {
                None
            };

            // Handle id and class specially
            if key == "class" && value.is_some() {
                self.class = value;
            } else if key == "id" && value.is_some() {
                self.id = value;
            } else {
                attribute_tape.push(Attribute {
                    key,
                    value,
                });
            }
        }

        // Build the attributes slice
        // Since we are
        //  1) assigning after adding the Attributes
        //  and 2) either transforming it into a Range in Store or removing them
        //  there is no risk when doing this unsafely
        self.attributes = unsafe {
            std::slice::from_raw_parts(
                attribute_tape.as_ptr().add(start_len),
                attribute_tape.len() - start_len,
            )
        };
    }
}

impl<'html> IElement<'html> for XHtmlElement<'html> {
    fn name(&self) -> &'html str {
        self.name
    }

    fn id(&self) -> Option<&'html str> {
        self.id
    }

    fn class(&self) -> Option<&'html str> {
        self.class
    }

    fn attributes(&self) -> &[Attribute<'html>] {
        self.attributes
    }
}

// Table-driven dispatch for tag parsing.
// Indexed by the first byte after '<' (+ whitespace skip), this replaces
// a two-branch if-else chain with a single table lookup.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum TagDispatch {
    Open,
    Close,
    Comment,
}

/// Lookup table: maps byte value → tag dispatch action.
/// Default is `Open`; '/' maps to `Close`; '!' maps to `Comment`.
const TAG_DISPATCH_TABLE: [TagDispatch; 256] = {
    let mut table = [TagDispatch::Open; 256];
    table[b'/' as usize] = TagDispatch::Close;
    table[b'!' as usize] = TagDispatch::Comment;
    table
};

impl<'a> XHtmlTag<'a> {
    pub fn from(reader: &mut Reader<'a>) -> Option<Self> {
        reader.next_while_list(&[b' ', b'\n', b'\r', b'\t', b'<']);
        if let Some(character) = reader.peek() {
            // Table-driven dispatch: single indexed load instead of two comparisons.
            match TAG_DISPATCH_TABLE[character as usize] {
                TagDispatch::Close => {
                    let start = reader.get_position() + 1;
                    reader.next_until(b'>');

                    let end = reader.get_position();
                    reader.skip();

                    return Some(Self::Close(reader.slice(start..end).trim()));
                }
                TagDispatch::Comment => {
                    reader.next_until(b'>');
                    reader.skip();
                    return None;
                }
                TagDispatch::Open => {
                    // Fall through to return Open below
                }
            }
        }
        Some(Self::Open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_no_quote_and_value_with_quote() {
        let mut reader = Reader::new("p key=\"value\"");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);
        assert_eq!(element.name, "p");

        assert_eq!(
            element.attributes[0],
            Attribute {
                key: "key",
                value: Some("value")
            }
        );
    }

    #[test]
    fn test_key_no_quote_and_value_no_quote() {
        let mut reader = Reader::new("p key=value");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);

        assert_eq!(element.name, "p");

        assert_eq!(element.attributes.len(), 1);

        assert_eq!(
            element.attributes[0],
            Attribute {
                key: "key",
                value: Some("value")
            }
        );
    }

    #[test]
    fn test_key_with_quote_and_value_with_quote() {
        let mut reader = Reader::new("p \"key\"=\"value\"");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);

        assert_eq!(element.name, "p");

        assert_eq!(
            element.attributes[0],
            Attribute {
                key: "key",
                value: Some("value")
            }
        );
    }

    #[test]
    fn test_multiple_key_value_pairs() {
        let mut reader = Reader::new("p key=\"value\" \"key1\"=value1 \"key2\"=\"value2\" keey");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);

        assert_eq!(element.name, "p");

        assert_eq!(
            element.attributes[0],
            Attribute {
                key: "key",
                value: Some("value")
            }
        );
        assert_eq!(
            element.attributes[1],
            Attribute {
                key: "key1",
                value: Some("value1")
            }
        );
        assert_eq!(
            element.attributes[2],
            Attribute {
                key: "key2",
                value: Some("value2")
            }
        );
        assert_eq!(
            element.attributes[3],
            Attribute {
                key: "keey",
                value: None
            }
        );
    }

    #[test]
    fn test_key_with_quote_and_no_value() {
        let mut reader = Reader::new("p \"key\"");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);

        assert_eq!(element.name, "p");

        assert_eq!(
            element.attributes[0],
            Attribute {
                key: "key",
                value: None
            }
        );
    }

    #[test]
    fn test_key_no_quote_and_no_value() {
        let mut reader = Reader::new("p key");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);

        assert_eq!(element.name, "p");

        assert_eq!(
            element.attributes[0],
            Attribute {
                key: "key",
                value: None
            }
        );
    }

    #[test]
    #[ignore = "Known issue: Escapes are not handled"]
    fn test_key_no_quote_and_escaped_space_value() {
        let mut reader = Reader::new("p key = hello\\ world");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);

        assert_eq!(element.name, "p");

        assert_eq!(
            element.attributes[0],
            Attribute {
                key: "key",
                value: Some("hello\\ world")
            }
        );
    }

    #[test]
    fn test_long_key_with_spaces() {
        let mut reader = Reader::new("p \"long key with spaces\"=\"value\"");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);

        assert_eq!(element.name, "p");

        assert_eq!(
            element.attributes[0],
            Attribute {
                key: "long key with spaces",
                value: Some("value")
            }
        );
    }

    #[test]
    fn test_long_key_with_spaces_and_different_quote_inside() {
        let mut reader = Reader::new("p \"long key's with spaces\"=\"value\"");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);

        assert_eq!(element.name, "p");

        assert_eq!(
            element.attributes[0],
            Attribute {
                key: "long key's with spaces",
                value: Some("value")
            }
        );
    }

    #[test]
    #[ignore = "Known issue: Escapes are not handled"]
    fn test_long_key_with_spaces_and_real_same_quote_inside() {
        let mut reader = Reader::new(r#"p "long key\"s with spaces"="value""#);
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);

        assert_eq!(element.name, "p");

        assert_eq!(
            element.attributes[0],
            Attribute {
                key: r#"long key\"s with spaces"#,
                value: Some("value")
            }
        );
    }

    #[test]
    #[ignore = "Known issue: Escapes are not handled"]
    fn test_long_key_and_value_with_spaces_and_real_same_quote_inside() {
        let mut reader = Reader::new(
            r#"p "long key\"s with spaces"="value\"s of an other person \\\\\\ \\\\\ \ \  \"""#,
        );
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);

        assert_eq!(element.name, "p");

        assert_eq!(
            element.attributes[0],
            Attribute {
                key: r#"long key\"s with spaces"#,
                value: Some(r#"value\"s of an other person \\\\\\ \\\\\ \ \  \""#)
            }
        );
    }

    #[test]
    fn test_valid_anchor_tag_attributes() {
        let mut reader = Reader::new(
            "a target=\"_blank\" href=\"/my_cv.pdf\" class=\"px-7 py-3\" hello-world=hello-world",
        );
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);

        assert_eq!(element.name, "a");

        assert_eq!(
            element.attributes[0],
            Attribute {
                key: "target",
                value: Some("_blank")
            }
        );

        assert_eq!(
            element.attributes[1],
            Attribute {
                key: "href",
                value: Some("/my_cv.pdf")
            }
        );

        assert_eq!(element.class, Some("px-7 py-3"));

        assert_eq!(
            element.attributes[2],
            Attribute {
                key: "hello-world",
                value: Some("hello-world")
            }
        );
    }

    #[test]
    fn test_complex_open_tag() {
        let mut reader = Reader::new(
            r#"a href="https://developer.mozilla.org/en-US/docs/Web/HTML/Attributes/crossorigin" title="The crossorigin attribute, valid on the <audio>, <img>, <link>, <script>, and <video> elements, provides support for CORS, defining how the element handles cross-origin requests, thereby enabling the configuration of the CORS requests for the element's fetched data. Depending on the element, the attribute can be a CORS settings attribute.""#,
        );

        let tag = XHtmlTag::from(&mut reader);
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);

        assert_eq!(tag, Some(XHtmlTag::Open));

        assert_eq!(
            element,
            XHtmlElement {
                name: "a",
                id: None,
                class: None,
                attributes: &[
                    Attribute {
                        key: "href",
                        value: Some(
                            "https://developer.mozilla.org/en-US/docs/Web/HTML/Attributes/crossorigin"
                        )
                    },
                    Attribute {
                        key: "title",
                        value: Some(
                            "The crossorigin attribute, valid on the <audio>, <img>, <link>, <script>, and <video> elements, provides support for CORS, defining how the element handles cross-origin requests, thereby enabling the configuration of the CORS requests for the element's fetched data. Depending on the element, the attribute can be a CORS settings attribute."
                        )
                    }
                ],
            }
        );
    }

    #[test]
    fn test_xhtml_tag_open() {
        let mut reader = Reader::new("p key=\"value\"");
        let tag = XHtmlTag::from(&mut reader);
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);

        assert_eq!(tag, Some(XHtmlTag::Open));

        assert_eq!(
            element,
            XHtmlElement {
                name: "p",
                id: None,
                class: None,
                attributes: &[Attribute {
                    key: "key",
                    value: Some("value")
                }],
            }
        );
    }

    #[test]
    fn test_xhtml_tag_close() {
        let mut reader = Reader::new("/p>");
        let tag = XHtmlTag::from(&mut reader);

        assert_eq!(tag, Some(XHtmlTag::Close("p")));
    }

    #[test]
    fn test_xhtml_tag_close_weird_formatting() {
        let mut reader = Reader::new("  /   p   >");
        let tag = XHtmlTag::from(&mut reader);

        assert_eq!(tag, Some(XHtmlTag::Close("p")));
    }

    #[test]
    fn test_parsing_comment() {
        let mut reader = Reader::new("<!-- These 3 links will be selected by the selector -->");
        let tag = XHtmlTag::from(&mut reader);

        assert!(tag.is_none())
    }

    #[test]
    fn test_parsing_mutiline_comment() {
        let mut reader = Reader::new(
            r#"
            <!-- These 3 links will be selected by the selector -->
        "#,
        );
        let tag = XHtmlTag::from(&mut reader);

        assert!(tag.is_none())
    }
}
