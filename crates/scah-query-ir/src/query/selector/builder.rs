use super::string_search::AttributeSelectionKind;
use crate::Reader;
use crate::query::compiler::SelectorParseError;

#[derive(Debug, PartialEq, Clone, Copy)]
enum QuoteKind {
    DoubleQuoted,
    SingleQuoted,
}

#[derive(Debug, PartialEq, Clone)]
pub struct Attribute<'html> {
    pub key: &'html str,
    pub value: Option<&'html str>,
}

#[derive(Debug, PartialEq, Clone)]
pub struct AttributeSelection<'query> {
    pub name: &'query str,
    pub value: Option<&'query str>,
    pub kind: AttributeSelectionKind,
}

impl<'query> AttributeSelection<'query> {
    pub const fn new_const(
        name: &'query str,
        value: Option<&'query str>,
        kind: AttributeSelectionKind,
    ) -> Self {
        Self { name, value, kind }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum AttributeSelections<'query> {
    Static(&'query [AttributeSelection<'query>]),
    Owned(Box<[AttributeSelection<'query>]>),
}

impl<'query> AttributeSelections<'query> {
    pub const fn from_static(attributes: &'query [AttributeSelection<'query>]) -> Self {
        Self::Static(attributes)
    }

    pub fn as_slice(&self) -> &[AttributeSelection<'query>] {
        match self {
            Self::Static(attributes) => attributes,
            Self::Owned(attributes) => attributes,
        }
    }
}

impl<'query> Default for AttributeSelections<'query> {
    fn default() -> Self {
        Self::Static(&[])
    }
}

impl<'query> From<Vec<AttributeSelection<'query>>> for AttributeSelections<'query> {
    fn from(value: Vec<AttributeSelection<'query>>) -> Self {
        Self::Owned(value.into_boxed_slice())
    }
}

#[derive(Debug, Clone)]
pub enum ClassSelections<'query> {
    Static(&'query [&'query str]),
    Owned(Box<[&'query str]>),
}

impl<'query> ClassSelections<'query> {
    pub const fn from_static(classes: &'query [&'query str]) -> Self {
        Self::Static(classes)
    }

    pub fn as_slice(&self) -> &[&'query str] {
        match self {
            Self::Static(classes) => classes,
            Self::Owned(classes) => classes,
        }
    }
}

impl<'query> Default for ClassSelections<'query> {
    fn default() -> Self {
        Self::Static(&[])
    }
}

impl<'query> From<Vec<&'query str>> for ClassSelections<'query> {
    fn from(value: Vec<&'query str>) -> Self {
        Self::Owned(value.into_boxed_slice())
    }
}

impl<'query> PartialEq for ClassSelections<'query> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

/// Element Interface
pub trait IElement<'html> {
    fn name(&self) -> &'html str;
    fn id(&self) -> Option<&'html str>;
    fn class(&self) -> Option<&'html str>;
    fn attributes(&self) -> &[Attribute<'html>];
}

struct KeyValueAttributeSelection<'query> {
    name: Option<&'query str>,
    selection_kind: AttributeSelectionKind,
    value: Option<&'query str>,
}

impl<'query> KeyValueAttributeSelection<'query> {
    fn push(&mut self, content_inside_quotes: &'query str) {
        if self.name.is_none() {
            self.name = Some(content_inside_quotes);
        } else if self.value.is_none() {
            self.value = Some(content_inside_quotes);
        } else {
            unreachable!();
        }
    }

    fn refresh_equal(&mut self) {
        if self.selection_kind == AttributeSelectionKind::Presence && self.value.is_some() {
            self.selection_kind = AttributeSelectionKind::Exact;
        }
    }
}

impl<'query> From<&mut Reader<'query>> for AttributeSelection<'query> {
    fn from(reader: &mut Reader<'query>) -> Self {
        Self::try_from(reader).unwrap()
    }
}

impl<'query> AttributeSelection<'query> {
    fn try_from(reader: &mut Reader<'query>) -> Result<Self, SelectorParseError> {
        let mut position = reader.get_position();

        let mut opened_quote: Option<QuoteKind> = None;
        let mut equal = false;

        let mut kv = KeyValueAttributeSelection {
            name: None,
            selection_kind: AttributeSelectionKind::Presence,
            value: None,
        };

        while let Some(token) = SelectionAttributeToken::next(reader)? {
            match token {
                SelectionAttributeToken::Quote(kind) => {
                    if opened_quote.is_none() {
                        opened_quote = Some(kind);
                        position = reader.get_position();
                        continue;
                    }

                    if let Some(quote_kind) = &opened_quote
                        && *quote_kind != kind
                    {
                        continue;
                    }

                    opened_quote = None;

                    const SIZE_OF_QUOTE: usize = 1;
                    let end_position = reader.get_position() - SIZE_OF_QUOTE;
                    let content_inside_quotes = reader.slice(position..end_position);

                    kv.push(content_inside_quotes);
                }

                SelectionAttributeToken::String(string_value) => {
                    if opened_quote.is_some() {
                        continue;
                    }

                    kv.push(string_value);
                }

                SelectionAttributeToken::StringMatchSelector(equal_selector) => {
                    kv.selection_kind = equal_selector;
                }

                SelectionAttributeToken::Equal => {
                    if kv.name.is_none() {
                        return Err(SelectorParseError::new(
                            "attribute selector is missing a key",
                            reader.get_position(),
                        ));
                    }
                    if kv.value.is_some() {
                        return Err(SelectorParseError::new(
                            "attribute selector has multiple values",
                            reader.get_position(),
                        ));
                    }
                    if equal {
                        return Err(SelectorParseError::new(
                            "attribute selector has multiple '=' tokens",
                            reader.get_position(),
                        ));
                    }
                    equal = true;
                }
            }
        }

        if kv.name.is_none() {
            return Err(SelectorParseError::new(
                "attribute selector is missing a key",
                reader.get_position(),
            ));
        }
        if !is_valid_attribute_name(kv.name.unwrap()) {
            return Err(SelectorParseError::new(
                "attribute selector key is invalid",
                reader.get_position(),
            ));
        }

        if opened_quote.is_some() {
            return Err(SelectorParseError::new(
                "attribute selector has an unclosed quoted value",
                reader.get_position(),
            ));
        }

        if equal && kv.value.is_none() {
            return Err(SelectorParseError::new(
                "attribute selector is missing a value",
                reader.get_position(),
            ));
        }

        // If a match selector operator (~, ^, $, *, |) was seen without a
        // following `=`, that's an error. Example: [attr~] or [attr*].
        if !equal && kv.selection_kind != AttributeSelectionKind::Presence {
            return Err(SelectorParseError::new(
                "attribute selector is missing '=' after match operator",
                reader.get_position(),
            ));
        }

        kv.refresh_equal();

        Ok(AttributeSelection {
            name: kv.name.unwrap(),
            value: kv.value,
            kind: kv.selection_kind,
        })
    }
}

enum SelectionKeyWords<'query> {
    String(&'query str),
    ID,
    Class,
    Quote,
    OpenAttribute,
    CloseAttribute,
}

impl<'a> SelectionKeyWords<'a> {
    pub fn next(reader: &mut Reader<'a>) -> Option<Self> {
        let start_pos = reader.get_position();

        if let Some(token) = reader.peek()
            && matches!(token, b'>' | b' ' | b'+' | b'~' | b'|')
        {
            return None;
        }

        match reader.next()? {
            b'#' => Some(Self::ID),
            b'.' => Some(Self::Class),
            b'"' => Some(Self::Quote),
            b'\'' => Some(Self::Quote),
            b'[' => Some(Self::OpenAttribute),
            b']' => Some(Self::CloseAttribute),
            _ => {
                reader.next_until_list(&[b' ', b'#', b'.', b'[']);
                Some(Self::String(reader.slice(start_pos..reader.get_position())))
            }
        }
    }
}

enum SelectionAttributeToken<'a> {
    String(&'a str),
    Quote(QuoteKind),
    Equal,
    StringMatchSelector(AttributeSelectionKind),
}

impl<'a> SelectionAttributeToken<'a> {
    pub fn next(reader: &mut Reader<'a>) -> Result<Option<Self>, SelectorParseError> {
        reader.next_while(b' ');

        let start_pos = reader.get_position();

        let token = match reader.next() {
            None => {
                return Err(SelectorParseError::new(
                    "attribute selector is missing a closing ']'",
                    reader.get_position(),
                ));
            }
            Some(token) => token,
        };

        Ok(match token {
            b'"' => Some(Self::Quote(QuoteKind::DoubleQuoted)),
            b'\'' => Some(Self::Quote(QuoteKind::SingleQuoted)),
            b'=' => Some(Self::Equal),
            b'~' => Some(Self::StringMatchSelector(
                AttributeSelectionKind::WhitespaceSeparated,
            )),
            b'|' => Some(Self::StringMatchSelector(
                AttributeSelectionKind::HyphenSeparated,
            )),
            b'^' => Some(Self::StringMatchSelector(AttributeSelectionKind::Prefix)),
            b'$' => Some(Self::StringMatchSelector(AttributeSelectionKind::Suffix)),
            b'*' => Some(Self::StringMatchSelector(AttributeSelectionKind::Substring)),
            b']' => None,
            _ => {
                reader.next_until_list(&[
                    b' ', b'"', b'\'', b'=', b']', b'~', b'|', b'^', b'$', b'*',
                ]);
                Some(Self::String(reader.slice(start_pos..reader.get_position())))
            }
        })
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ElementPredicate<'a> {
    pub name: Option<&'a str>,
    pub id: Option<&'a str>,
    pub classes: ClassSelections<'a>,
    pub attributes: AttributeSelections<'a>,
}

impl<'a> ElementPredicate<'a> {
    pub const fn new_const(
        name: Option<&'a str>,
        id: Option<&'a str>,
        classes: ClassSelections<'a>,
        attributes: AttributeSelections<'a>,
    ) -> Self {
        Self {
            name,
            id,
            classes,
            attributes,
        }
    }

    fn push_class(&mut self, class_name: &'a str) {
        let mut classes = self.classes.as_slice().to_vec();
        classes.push(class_name);
        self.classes = ClassSelections::from(classes);
    }

    fn try_parse_attribute(&mut self, reader: &mut Reader<'a>) -> Result<(), SelectorParseError> {
        let attribute = AttributeSelection::try_from(reader)?;
        let mut attributes = self.attributes.as_slice().to_vec();
        attributes.push(attribute);
        self.attributes = AttributeSelections::from(attributes);
        Ok(())
    }

    pub fn try_from(reader: &mut Reader<'a>) -> Result<Self, SelectorParseError> {
        let mut element = Self {
            name: None,
            id: None,
            classes: ClassSelections::default(),
            attributes: AttributeSelections::default(),
        };

        let mut previous: Option<SelectionKeyWords> = None;

        while let Some(word) = SelectionKeyWords::next(reader) {
            match (previous, &word) {
                (Option::None, SelectionKeyWords::String(name)) => {
                    if !is_valid_selector_name(name) {
                        return Err(SelectorParseError::new(
                            "illegal selector token",
                            reader.get_position().saturating_sub(name.len()),
                        ));
                    }
                    if element.name.is_some() {
                        return Err(SelectorParseError::new(
                            "selector has multiple element names",
                            reader.get_position().saturating_sub(name.len()),
                        ));
                    }
                    element.name = Some(*name);
                }
                (Some(SelectionKeyWords::ID), SelectionKeyWords::String(id_name)) => {
                    if !is_valid_selector_name(id_name) {
                        return Err(SelectorParseError::new(
                            "missing id string",
                            reader.get_position().saturating_sub(id_name.len()),
                        ));
                    }
                    if element.id.is_none() {
                        element.id = Some(*id_name);
                    }
                }
                (Some(SelectionKeyWords::Class), SelectionKeyWords::String(class_name)) => {
                    if !is_valid_selector_name(class_name) {
                        return Err(SelectorParseError::new(
                            "missing class string",
                            reader.get_position().saturating_sub(class_name.len()),
                        ));
                    }
                    element.push_class(class_name);
                }
                (_, SelectionKeyWords::OpenAttribute) => element.try_parse_attribute(reader)?,

                (Some(SelectionKeyWords::ID), _) => {
                    return Err(SelectorParseError::new(
                        "missing id string",
                        reader.get_position(),
                    ));
                }
                (Some(SelectionKeyWords::Class), _) => {
                    return Err(SelectorParseError::new(
                        "missing class string",
                        reader.get_position(),
                    ));
                }

                (_, _) => (),
            }

            previous = Some(word);
        }

        match previous {
            Some(SelectionKeyWords::ID) => Err(SelectorParseError::new(
                "missing id string",
                reader.get_position(),
            )),
            Some(SelectionKeyWords::Class) => Err(SelectorParseError::new(
                "missing class string",
                reader.get_position(),
            )),
            _ if element.name.is_none()
                && element.id.is_none()
                && element.classes.as_slice().is_empty()
                && element.attributes.as_slice().is_empty() =>
            {
                Err(SelectorParseError::new(
                    "missing selector element",
                    reader.get_position(),
                ))
            }
            _ => Ok(element),
        }
    }
}

impl<'a> From<&mut Reader<'a>> for ElementPredicate<'a> {
    fn from(reader: &mut Reader<'a>) -> Self {
        Self::try_from(reader).unwrap()
    }
}

fn is_valid_selector_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn is_valid_attribute_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_alphabetic() || first == b'_' => (),
        _ => return false,
    }

    bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_element_selection() {
        let mut reader = Reader::new("element#id.class");
        let element = ElementPredicate::from(&mut reader);

        assert_eq!(
            element,
            ElementPredicate {
                name: Some("element"),
                id: Some("id"),
                classes: ClassSelections::from_static(&["class"]),
                attributes: AttributeSelections::from_static(&[]),
            }
        );
    }

    #[test]
    fn test_fully_detailed_element_selection() {
        let mut reader = Reader::new("element#id.class[selected=true]");

        let element = ElementPredicate::from(&mut reader);

        assert_eq!(
            element,
            ElementPredicate {
                name: Some("element"),
                id: Some("id"),
                classes: ClassSelections::from_static(&["class"]),
                attributes: AttributeSelections::from(vec![AttributeSelection {
                    name: "selected",
                    value: Some("true"),
                    kind: AttributeSelectionKind::Exact
                }]),
            }
        );
    }

    #[test]
    fn test_two_fully_detailed_element_selection() {
        let mut reader = Reader::new("element#id.class[href~=\"_blank\"][selected=true]");

        let element = ElementPredicate::from(&mut reader);

        assert_eq!(
            element,
            ElementPredicate {
                name: Some("element"),
                id: Some("id"),
                classes: ClassSelections::from_static(&["class"]),
                attributes: AttributeSelections::from(vec![
                    AttributeSelection {
                        name: "href",
                        value: Some("_blank"),
                        kind: AttributeSelectionKind::WhitespaceSeparated
                    },
                    AttributeSelection {
                        name: "selected",
                        value: Some("true"),
                        kind: AttributeSelectionKind::Exact
                    }
                ]),
            }
        );
    }

    #[test]
    fn test_handle_duplicates_in_element_definition() {
        let mut reader = Reader::new("element#id.class[selected=true]#id#notid");
        let element = ElementPredicate::from(&mut reader);

        assert_eq!(
            element,
            ElementPredicate {
                name: Some("element"),
                id: Some("id"),
                classes: ClassSelections::from_static(&["class"]),
                attributes: AttributeSelections::from(vec![AttributeSelection {
                    name: "selected",
                    value: Some("true"),
                    kind: AttributeSelectionKind::Exact
                }]),
            }
        );
    }

    #[test]
    fn test_multiple_classes_are_preserved() {
        let mut reader = Reader::new("a.blue.exit");
        let element = ElementPredicate::from(&mut reader);

        assert_eq!(
            element,
            ElementPredicate {
                name: Some("a"),
                id: None,
                classes: ClassSelections::from_static(&["blue", "exit"]),
                attributes: AttributeSelections::from_static(&[]),
            }
        );
    }
}
