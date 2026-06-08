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
            // Strip trailing solidus from tag name: <hr/> -> "hr"
            let name = attribute.key;
            self.name = if name.ends_with('/') {
                &name[..name.len() - 1]
            } else {
                name
            };
        } else if self.class.is_none() && attribute.key == "class" && attribute.value.is_some() {
            self.class = attribute.value;
        } else if self.id.is_none() && attribute.key == "id" && attribute.value.is_some() {
            self.id = attribute.value;
        } else {
            attribute_tape.push(attribute);
        }
    }

    pub fn is_self_closing(&self) -> bool {
        if is_html_void_element(self.name) {
            return true;
        }
        // In HTML mode, only void elements are self-closing;
        // trailing solidus is ignored for non-void elements.
        // For XHTML/XML mode, `trailing_solidus` would close arbitrary elements.
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

        // Detect trailing solidus: if the last "attribute" is a lone "/"
        // treat it as a trailing solidus, not as an attribute (HTML-compatible behavior).
        if let Some(k) = key {
            if k == "/" {
                // Trailing solidus — skip it, don't add as attribute.
                // Also remove any "/" attribute that may have been pushed with a value.
                let tape_len = attribute_tape.len();
                if tape_len > start_len {
                    let last = &attribute_tape[tape_len - 1];
                    if last.key == "/" {
                        attribute_tape.truncate(tape_len - 1);
                    }
                }
            } else {
                self.add_to_element(
                    Attribute {
                        key: k,
                        value: None,
                    },
                    attribute_tape,
                );
            }
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
                attribute_tape.push(Attribute { key, value });
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

#[inline]
fn is_html_void_element(name: &str) -> bool {
    match name.len() {
        2 => name.eq_ignore_ascii_case("br") || name.eq_ignore_ascii_case("hr"),
        3 => {
            name.eq_ignore_ascii_case("img")
                || name.eq_ignore_ascii_case("col")
                || name.eq_ignore_ascii_case("wbr")
        }
        4 => {
            name.eq_ignore_ascii_case("area")
                || name.eq_ignore_ascii_case("base")
                || name.eq_ignore_ascii_case("link")
                || name.eq_ignore_ascii_case("meta")
        }
        5 => {
            name.eq_ignore_ascii_case("embed")
                || name.eq_ignore_ascii_case("input")
                || name.eq_ignore_ascii_case("param")
                || name.eq_ignore_ascii_case("track")
        }
        6 => name.eq_ignore_ascii_case("source"),
        _ => false,
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

impl<'a> XHtmlTag<'a> {
    pub fn from(reader: &mut Reader<'a>) -> Option<Self> {
        reader.next_while_list(&[b' ', b'\n', b'\r', b'\t', b'<']);
        if let Some(character) = reader.peek() {
            if character == b'/' {
                let start = reader.get_position() + 1;
                reader.next_until(b'>');

                let end = reader.get_position();
                reader.skip();

                return Some(Self::Close(reader.slice(start..end).trim()));
            } else if character == b'!' {
                // HTML comments: <!-- ... -->
                // Check for HTML comment: <!--
                if reader.source_bytes().get(reader.get_position() + 1) == Some(&b'-')
                    && reader.source_bytes().get(reader.get_position() + 2) == Some(&b'-')
                {
                    // Consume the '!' and both '-' characters
                    reader.skip(); // skip '!'
                    reader.skip(); // skip first '-'
                    reader.skip(); // skip second '-'
                    loop {
                        reader.next_until(b'-');
                        if reader.peek() == Some(b'-') {
                            reader.skip();
                            if reader.peek() == Some(b'>') {
                                reader.skip();
                                break;
                            }
                        } else if reader.peek().is_none() {
                            // EOF inside comment — stop
                            break;
                        } else {
                            reader.skip();
                        }
                    }
                } else {
                    // Other SGML markup like <!DOCTYPE ...>
                    reader.next_until(b'>');
                    reader.skip();
                }
                return None;
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

    // --- Edge Case #2: slash / self-closing ---

    #[test]
    fn test_void_element_with_trailing_slash() {
        // <hr /> should parse as self-closing void element, / not an attribute
        let mut reader = Reader::new("hr />");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);
        assert_eq!(element.name, "hr");
        assert!(element.is_self_closing());
        assert!(element.attributes.is_empty());
    }

    #[test]
    fn test_void_element_without_slash() {
        let mut reader = Reader::new("hr>");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);
        assert_eq!(element.name, "hr");
        assert!(element.is_self_closing());
    }

    #[test]
    fn test_input_with_trailing_slash() {
        // <input disabled /> — disabled attr, / is not an attribute
        let mut reader = Reader::new("input disabled />");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);
        assert_eq!(element.name, "input");
        assert!(element.is_self_closing());
        // disabled should be an attribute, / should NOT
        assert_eq!(element.attributes.len(), 1);
        assert_eq!(element.attributes[0].key, "disabled");
    }

    #[test]
    fn test_non_void_element_with_slash_is_not_self_closing() {
        // <div />after — in HTML, div is NOT self-closing
        // The slash is just ignored
        let mut reader = Reader::new("div />");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);
        assert_eq!(element.name, "div");
        assert!(!element.is_self_closing());
        assert!(element.attributes.is_empty());
    }

    // --- Edge Case #3: tag whitespace ---

    #[test]
    fn test_tag_with_newline_between_attributes() {
        let mut reader = Reader::new("a\n  href=\"x\"\n  class=\"link\">");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);
        assert_eq!(element.name, "a");
        assert_eq!(element.class, Some("link"));
        assert_eq!(element.attributes.len(), 1);
        assert_eq!(element.attributes[0].key, "href");
        assert_eq!(element.attributes[0].value, Some("x"));
    }

    #[test]
    fn test_tag_with_tab_between_attributes() {
        let mut reader = Reader::new("a\thref=x>");
        let mut element = XHtmlElement::default();
        let mut attributes = vec![];
        element.from(&mut reader, &mut attributes);
        assert_eq!(element.name, "a");
        assert_eq!(element.attributes[0].key, "href");
        assert_eq!(element.attributes[0].value, Some("x"));
    }

    // --- Edge Case #7: comments with > inside ---

    #[test]
    fn test_comment_with_gt_inside() {
        // <!-- a > b --><a>real</a>
        // The > inside the comment should not terminate the comment
        let mut reader = Reader::new("<!-- a > b -->");
        let tag = XHtmlTag::from(&mut reader);
        assert!(tag.is_none());
        // Reader should be at end of comment, after -->
        assert_eq!(reader.get_position(), 14);
    }

    #[test]
    fn test_comment_with_nested_markup() {
        // <!-- <div><span></span></div> -->
        let mut reader = Reader::new("<!-- <div><span></span></div> -->");
        let tag = XHtmlTag::from(&mut reader);
        assert!(tag.is_none());
    }

    #[test]
    fn test_doctype_is_skipped() {
        let mut reader = Reader::new("!DOCTYPE html>");
        let tag = XHtmlTag::from(&mut reader);
        assert!(tag.is_none());
    }
}
