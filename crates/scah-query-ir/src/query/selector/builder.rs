use super::is_css_whitespace;
use super::string_search::{AttributeCaseSensitivity, AttributeSelectionKind};
use crate::Reader;
use crate::query::compiler::SelectorParseError;

#[inline]
fn is_element_selector_boundary(byte: u8) -> bool {
    is_css_whitespace(byte) || matches!(byte, b'#' | b'.' | b'[' | b':' | b'>' | b'+' | b'~' | b'|')
}

#[inline]
fn is_attribute_selector_boundary(byte: u8) -> bool {
    is_css_whitespace(byte)
        || matches!(
            byte,
            b'"' | b'\'' | b'=' | b']' | b'~' | b'|' | b'^' | b'$' | b'*'
        )
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
    pub case_sensitivity: AttributeCaseSensitivity,
}

#[derive(Debug, PartialEq, Clone)]
pub enum LocalSelectorList<'query> {
    Static(&'query [ElementPredicate<'query>]),
    Owned(Box<[ElementPredicate<'query>]>),
}

impl<'query> LocalSelectorList<'query> {
    pub const fn from_static(selectors: &'query [ElementPredicate<'query>]) -> Self {
        Self::Static(selectors)
    }

    pub const fn as_slice(&self) -> &[ElementPredicate<'query>] {
        match self {
            Self::Static(selectors) => selectors,
            Self::Owned(selectors) => selectors,
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum LocalLogicalPredicate<'query> {
    Not(LocalSelectorList<'query>),
    Any(LocalSelectorList<'query>),
}

#[derive(Debug, Clone)]
pub enum LogicalPredicates<'query> {
    Static(&'query [LocalLogicalPredicate<'query>]),
    Owned(Box<[LocalLogicalPredicate<'query>]>),
}

impl<'query> PartialEq for LogicalPredicates<'query> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct AnPlusB {
    pub a: i32,
    pub b: i32,
}

impl AnPlusB {
    pub const fn matches(self, index: u32) -> bool {
        let index = index as i64;
        let a = self.a as i64;
        let b = self.b as i64;
        if a == 0 {
            return b > 0 && index == b;
        }
        let delta = index - b;
        if a > 0 {
            delta >= 0 && delta % a == 0
        } else {
            delta <= 0 && delta % (-a) == 0
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum StructuralPredicate<'query> {
    Root,
    Scope,
    FirstChild,
    NthChild(AnPlusB),
    FirstOfType,
    NthOfType(AnPlusB),
    NthChildOf(AnPlusB, LocalSelectorList<'query>),
}

#[derive(Debug, Clone)]
pub struct StructuralMatchContext<'query> {
    pub child_index: u32,
    pub type_index: u32,
    pub filtered_child_indices: smallvec::SmallVec<[(&'query LocalSelectorList<'query>, u32); 8]>,
    pub is_root: bool,
}

impl PartialEq for StructuralMatchContext<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.child_index == other.child_index
            && self.type_index == other.type_index
            && self.is_root == other.is_root
            && self.filtered_child_indices.len() == other.filtered_child_indices.len()
            && self
                .filtered_child_indices
                .iter()
                .zip(&other.filtered_child_indices)
                .all(
                    |(&(left_filter, left_index), &(right_filter, right_index))| {
                        std::ptr::eq(left_filter, right_filter) && left_index == right_index
                    },
                )
    }
}

impl Eq for StructuralMatchContext<'_> {}

#[derive(Debug, Clone)]
pub enum StructuralPredicates<'query> {
    Static(&'query [StructuralPredicate<'query>]),
    Owned(Box<[StructuralPredicate<'query>]>),
}

impl<'query> PartialEq for StructuralPredicates<'query> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<'query> StructuralPredicates<'query> {
    pub const fn from_static(predicates: &'query [StructuralPredicate<'query>]) -> Self {
        Self::Static(predicates)
    }

    pub const fn as_slice(&self) -> &[StructuralPredicate<'query>] {
        match self {
            Self::Static(predicates) => predicates,
            Self::Owned(predicates) => predicates,
        }
    }
}

impl<'query> Default for StructuralPredicates<'query> {
    fn default() -> Self {
        Self::Static(&[])
    }
}

impl<'query> From<Vec<StructuralPredicate<'query>>> for StructuralPredicates<'query> {
    fn from(value: Vec<StructuralPredicate<'query>>) -> Self {
        Self::Owned(value.into_boxed_slice())
    }
}

impl<'query> LogicalPredicates<'query> {
    pub const fn from_static(predicates: &'query [LocalLogicalPredicate<'query>]) -> Self {
        Self::Static(predicates)
    }

    pub const fn as_slice(&self) -> &[LocalLogicalPredicate<'query>] {
        match self {
            Self::Static(predicates) => predicates,
            Self::Owned(predicates) => predicates,
        }
    }
}

impl<'query> Default for LogicalPredicates<'query> {
    fn default() -> Self {
        Self::Static(&[])
    }
}

impl<'query> From<Vec<LocalLogicalPredicate<'query>>> for LogicalPredicates<'query> {
    fn from(value: Vec<LocalLogicalPredicate<'query>>) -> Self {
        Self::Owned(value.into_boxed_slice())
    }
}

impl<'query> AttributeSelection<'query> {
    pub const fn new_const(
        name: &'query str,
        value: Option<&'query str>,
        kind: AttributeSelectionKind,
        case_sensitivity: AttributeCaseSensitivity,
    ) -> Self {
        Self {
            name,
            value,
            kind,
            case_sensitivity,
        }
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

    pub const fn as_slice(&self) -> &[AttributeSelection<'query>] {
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

    pub const fn as_slice(&self) -> &[&'query str] {
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
    fn push(
        &mut self,
        content_inside_quotes: &'query str,
        position: usize,
    ) -> Result<(), SelectorParseError> {
        if self.name.is_none() {
            self.name = Some(content_inside_quotes);
            Ok(())
        } else if self.value.is_none() {
            self.value = Some(content_inside_quotes);
            Ok(())
        } else {
            Err(SelectorParseError::new(
                "attribute selector has multiple values",
                position,
            ))
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
        let mut equal = false;
        let mut operator_requires_equal = false;
        let mut case_sensitivity = AttributeCaseSensitivity::Default;
        let mut saw_modifier = false;

        let mut kv = KeyValueAttributeSelection {
            name: None,
            selection_kind: AttributeSelectionKind::Presence,
            value: None,
        };

        while let Some(token) = SelectionAttributeToken::next(reader)? {
            // A match operator (`~`, `|`, `^`, `$`, `*`) must be immediately
            // followed by `=`. Anything else (`[class~]`, `[id^]`, ...) is a
            // malformed selector.
            if operator_requires_equal && !matches!(token, SelectionAttributeToken::Equal) {
                return Err(SelectorParseError::new(
                    "attribute match operator requires '='",
                    reader.get_position(),
                ));
            }

            match token {
                SelectionAttributeToken::String(string_value) => {
                    if kv.value.is_none()
                        && kv.name.is_some()
                        && !equal
                        && (string_value.eq_ignore_ascii_case("i")
                            || string_value.eq_ignore_ascii_case("s"))
                    {
                        return Err(SelectorParseError::new(
                            "attribute value modifiers require a value comparison",
                            reader.get_position(),
                        ));
                    } else if kv.value.is_some() && !saw_modifier {
                        case_sensitivity = if string_value.eq_ignore_ascii_case("i") {
                            AttributeCaseSensitivity::AsciiInsensitive
                        } else if string_value.eq_ignore_ascii_case("s") {
                            AttributeCaseSensitivity::Sensitive
                        } else {
                            return Err(SelectorParseError::new(
                                "attribute value modifier must be 'i' or 's'",
                                reader.get_position(),
                            ));
                        };
                        saw_modifier = true;
                    } else if saw_modifier {
                        return Err(SelectorParseError::new(
                            "attribute selector has multiple value modifiers",
                            reader.get_position(),
                        ));
                    } else {
                        kv.push(string_value, reader.get_position())?;
                    }
                }

                SelectionAttributeToken::QuotedString(string_value) => {
                    if kv.value.is_some() {
                        return Err(SelectorParseError::new(
                            "attribute value modifier must be an unquoted identifier",
                            reader.get_position(),
                        ));
                    }
                    kv.push(string_value, reader.get_position())?;
                }

                SelectionAttributeToken::StringMatchSelector(equal_selector) => {
                    kv.selection_kind = equal_selector;
                    operator_requires_equal = true;
                }

                SelectionAttributeToken::Equal => {
                    operator_requires_equal = false;
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

        if operator_requires_equal {
            return Err(SelectorParseError::new(
                "attribute match operator requires '='",
                reader.get_position(),
            ));
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

        if equal && kv.value.is_none() {
            return Err(SelectorParseError::new(
                "attribute selector is missing a value",
                reader.get_position(),
            ));
        }

        kv.refresh_equal();

        Ok(AttributeSelection {
            name: kv.name.unwrap(),
            value: kv.value,
            kind: kv.selection_kind,
            case_sensitivity,
        })
    }
}

enum SelectionKeyWords<'query> {
    String(&'query str),
    Universal,
    SimplePseudo(&'query str),
    FunctionalPseudo(&'query str),
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
            && (matches!(token, b'>' | b'+' | b'~' | b'|') || is_css_whitespace(token))
        {
            return None;
        }

        match reader.next()? {
            b'*' if start_pos + 1 == reader.get_position() => Some(Self::Universal),
            b':' => {
                let name_start = reader.get_position();
                while let Some(byte) = reader.peek() {
                    if byte.is_ascii_alphabetic() || byte == b'-' {
                        reader.skip();
                    } else {
                        break;
                    }
                }
                let name = reader.slice(name_start..reader.get_position());
                if reader.peek() == Some(b'(') {
                    reader.skip();
                    Some(Self::FunctionalPseudo(name))
                } else {
                    Some(Self::SimplePseudo(name))
                }
            }
            b'#' => Some(Self::ID),
            b'.' => Some(Self::Class),
            b'"' => Some(Self::Quote),
            b'\'' => Some(Self::Quote),
            b'[' => Some(Self::OpenAttribute),
            b']' => Some(Self::CloseAttribute),
            _ => {
                while let Some(byte) = reader.peek() {
                    if is_element_selector_boundary(byte) {
                        break;
                    }
                    reader.skip();
                }
                Some(Self::String(reader.slice(start_pos..reader.get_position())))
            }
        }
    }
}

enum SelectionAttributeToken<'a> {
    String(&'a str),
    QuotedString(&'a str),
    Equal,
    StringMatchSelector(AttributeSelectionKind),
}

impl<'a> SelectionAttributeToken<'a> {
    pub fn next(reader: &mut Reader<'a>) -> Result<Option<Self>, SelectorParseError> {
        while let Some(b) = reader.peek() {
            if !is_css_whitespace(b) {
                break;
            }
            reader.skip();
        }

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
            b'"' | b'\'' => {
                let quote = token;
                let content_start = reader.get_position();

                reader.next_until_unescaped(quote, b'\\');

                if reader.peek() != Some(quote) {
                    return Err(SelectorParseError::new(
                        "attribute selector has an unclosed quoted value",
                        reader.get_position(),
                    ));
                }

                let value = reader.slice(content_start..reader.get_position());
                reader.skip(); // consume closing quote

                Some(Self::QuotedString(value))
            }
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
                while let Some(byte) = reader.peek() {
                    if is_attribute_selector_boundary(byte) {
                        break;
                    }
                    reader.skip();
                }
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
    pub logical: LogicalPredicates<'a>,
    pub structural: StructuralPredicates<'a>,
}

impl<'a> ElementPredicate<'a> {
    pub const fn new_const(
        name: Option<&'a str>,
        id: Option<&'a str>,
        classes: ClassSelections<'a>,
        attributes: AttributeSelections<'a>,
        logical: LogicalPredicates<'a>,
        structural: StructuralPredicates<'a>,
    ) -> Self {
        Self {
            name,
            id,
            classes,
            attributes,
            logical,
            structural,
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
            logical: LogicalPredicates::default(),
            structural: StructuralPredicates::default(),
        };
        let mut universal = false;
        let mut logical = Vec::new();
        let mut structural = Vec::new();

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
                (Option::None, SelectionKeyWords::Universal) => universal = true,
                (Some(SelectionKeyWords::Universal), SelectionKeyWords::String(_)) => {
                    return Err(SelectorParseError::new(
                        "universal selector must be the only type selector",
                        reader.get_position(),
                    ));
                }
                (_, SelectionKeyWords::Universal) => {
                    return Err(SelectorParseError::new(
                        "universal selector must start a compound selector",
                        reader.get_position().saturating_sub(1),
                    ));
                }
                (_, SelectionKeyWords::FunctionalPseudo(name)) => {
                    if name.is_empty() {
                        return Err(SelectorParseError::new(
                            "illegal selector token",
                            reader.get_position(),
                        ));
                    }
                    let is_nth_child = name.eq_ignore_ascii_case("nth-child");
                    let is_nth_of_type = name.eq_ignore_ascii_case("nth-of-type");
                    if is_nth_child || is_nth_of_type {
                        let argument = read_balanced_function_argument(reader)?;
                        let (formula_source, filter_source) = split_nth_filter(argument)?;
                        let formula = parse_an_plus_b(formula_source)?;
                        structural.push(if is_nth_child {
                            if let Some(filter_source) = filter_source {
                                StructuralPredicate::NthChildOf(
                                    formula,
                                    parse_local_selector_list(filter_source)?,
                                )
                            } else {
                                StructuralPredicate::NthChild(formula)
                            }
                        } else {
                            if filter_source.is_some() {
                                return Err(SelectorParseError::new(
                                    "filtered nth-of-type is not supported",
                                    reader.get_position(),
                                ));
                            }
                            StructuralPredicate::NthOfType(formula)
                        });
                    } else {
                        let argument = read_balanced_function_argument(reader)?;
                        let predicate = if name.eq_ignore_ascii_case("not") {
                            LocalLogicalPredicate::Not(parse_local_selector_list(argument)?)
                        } else if name.eq_ignore_ascii_case("is")
                            || name.eq_ignore_ascii_case("where")
                        {
                            LocalLogicalPredicate::Any(parse_forgiving_local_selector_list(
                                argument,
                            ))
                        } else {
                            return Err(SelectorParseError::new(
                                "unsupported pseudo-class",
                                reader.get_position().saturating_sub(name.len() + 2),
                            ));
                        };
                        logical.push(predicate);
                    }
                }
                (_, SelectionKeyWords::SimplePseudo(name)) => {
                    if name.is_empty() {
                        return Err(SelectorParseError::new(
                            "illegal selector token",
                            reader.get_position(),
                        ));
                    }
                    structural.push(if name.eq_ignore_ascii_case("first-child") {
                        StructuralPredicate::FirstChild
                    } else if name.eq_ignore_ascii_case("first-of-type") {
                        StructuralPredicate::FirstOfType
                    } else if name.eq_ignore_ascii_case("root") {
                        StructuralPredicate::Root
                    } else if name.eq_ignore_ascii_case("scope") {
                        StructuralPredicate::Scope
                    } else {
                        return Err(SelectorParseError::new(
                            "unsupported pseudo-class",
                            reader.get_position().saturating_sub(name.len() + 1),
                        ));
                    });
                }
                (Some(SelectionKeyWords::ID), SelectionKeyWords::String(id_name)) => {
                    if !is_valid_selector_name(id_name) {
                        return Err(SelectorParseError::new(
                            "missing id string",
                            reader.get_position().saturating_sub(id_name.len()),
                        ));
                    }
                    if element.id.is_some() {
                        return Err(SelectorParseError::new(
                            "selector has multiple IDs",
                            reader.get_position().saturating_sub(id_name.len()),
                        ));
                    }
                    element.id = Some(*id_name);
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
                && element.attributes.as_slice().is_empty()
                && logical.is_empty()
                && structural.is_empty()
                && !universal =>
            {
                Err(SelectorParseError::new(
                    "missing selector element",
                    reader.get_position(),
                ))
            }
            _ => {
                element.logical = LogicalPredicates::from(logical);
                element.structural = StructuralPredicates::from(structural);
                Ok(element)
            }
        }
    }
}

fn read_balanced_function_argument<'query>(
    reader: &mut Reader<'query>,
) -> Result<&'query str, SelectorParseError> {
    let start = reader.get_position();
    let mut depth = 1usize;
    while let Some(byte) = reader.next() {
        match byte {
            b'"' | b'\'' => {
                let quote = byte;
                reader.next_until_unescaped(quote, b'\\');
                if reader.peek() == Some(quote) {
                    reader.skip();
                } else {
                    return Err(SelectorParseError::new(
                        "pseudo-class has an unclosed quoted value",
                        reader.get_position(),
                    ));
                }
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(reader.slice(start..reader.get_position() - 1));
                }
            }
            _ => {}
        }
    }
    Err(SelectorParseError::new(
        "pseudo-class has an unclosed ')'",
        reader.get_position(),
    ))
}

fn parse_local_selector_list<'query>(
    source: &'query str,
) -> Result<LocalSelectorList<'query>, SelectorParseError> {
    parse_local_selector_list_parts(source, false)
}

fn parse_forgiving_local_selector_list<'query>(source: &'query str) -> LocalSelectorList<'query> {
    parse_local_selector_list_parts(source, true)
        .expect("forgiving selector-list parsing cannot fail")
}

fn parse_local_selector_list_parts<'query>(
    source: &'query str,
    forgiving: bool,
) -> Result<LocalSelectorList<'query>, SelectorParseError> {
    let bytes = source.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active_quote {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(&source[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&source[start..]);
    let mut selectors = Vec::with_capacity(parts.len());
    for part in parts {
        match parse_local_selector(part) {
            Ok(selector) => selectors.push(selector),
            Err(_) if forgiving => {}
            Err(error) => return Err(error),
        }
    }
    Ok(LocalSelectorList::Owned(selectors.into_boxed_slice()))
}

fn parse_local_selector(source: &str) -> Result<ElementPredicate<'_>, SelectorParseError> {
    let source = source.trim();
    if source.is_empty() {
        return Err(SelectorParseError::new(
            "pseudo-class selector list has an empty alternative",
            0,
        ));
    }

    let mut reader = Reader::new(source);
    let selector = ElementPredicate::try_from(&mut reader)?;
    if !reader.eof() {
        return Err(SelectorParseError::new(
            "combinators are not supported inside local pseudo-classes",
            reader.get_position(),
        ));
    }
    if selector.requires_structural() {
        return Err(SelectorParseError::new(
            "structural pseudo-classes are not supported inside local selector lists",
            0,
        ));
    }
    Ok(selector)
}

fn is_css_whitespace_char(character: char) -> bool {
    character.is_ascii() && is_css_whitespace(character as u8)
}

fn parse_an_plus_b(source: &str) -> Result<AnPlusB, SelectorParseError> {
    let source = source.trim_matches(is_css_whitespace_char);

    if source.eq_ignore_ascii_case("odd") {
        return Ok(AnPlusB { a: 2, b: 1 });
    }
    if source.eq_ignore_ascii_case("even") {
        return Ok(AnPlusB { a: 2, b: 0 });
    }
    if let Ok(b) = source.parse::<i32>() {
        return Ok(AnPlusB { a: 0, b });
    }

    let Some(n_index) = source
        .bytes()
        .position(|byte| byte.eq_ignore_ascii_case(&b'n'))
    else {
        return Err(SelectorParseError::new("invalid An+B formula", 0));
    };
    let coefficient = &source[..n_index];
    let a = match coefficient {
        "" | "+" => 1,
        "-" => -1,
        value => value
            .parse::<i32>()
            .map_err(|_| SelectorParseError::new("invalid An+B coefficient", 0))?,
    };

    let remainder = source[n_index + 1..].trim_matches(is_css_whitespace_char);
    let b = if remainder.is_empty() {
        0
    } else {
        let sign = remainder.as_bytes()[0];
        if !matches!(sign, b'+' | b'-') {
            return Err(SelectorParseError::new("invalid An+B offset", n_index + 1));
        }
        let digits = remainder[1..].trim_start_matches(is_css_whitespace_char);
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(SelectorParseError::new("invalid An+B offset", n_index + 1));
        }
        let magnitude = digits
            .parse::<i64>()
            .map_err(|_| SelectorParseError::new("invalid An+B offset", n_index + 1))?;
        let signed = if sign == b'-' { -magnitude } else { magnitude };
        i32::try_from(signed)
            .map_err(|_| SelectorParseError::new("invalid An+B offset", n_index + 1))?
    };
    Ok(AnPlusB { a, b })
}

fn split_nth_filter(source: &str) -> Result<(&str, Option<&str>), SelectorParseError> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(q) = quote {
            if byte == q && (index == 0 || bytes[index - 1] != b'\\') {
                quote = None;
            }
        } else if byte == b'\'' || byte == b'"' {
            quote = Some(byte);
        } else if byte == b'(' || byte == b'[' {
            depth += 1;
        } else if byte == b')' || byte == b']' {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && byte.is_ascii_whitespace() {
            let rest = source[index..].trim_start();
            let rest_bytes = rest.as_bytes();
            if rest_bytes
                .get(..2)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"of"))
                && rest_bytes.get(2).is_some_and(u8::is_ascii_whitespace)
            {
                let formula = source[..index].trim();
                let filter = rest[2..].trim();
                if formula.is_empty() || filter.is_empty() {
                    return Err(SelectorParseError::new(
                        "invalid filtered An+B formula",
                        index,
                    ));
                }
                return Ok((formula, Some(filter)));
            }
        }
        index += 1;
    }
    Ok((source.trim(), None))
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
    use crate::Transition;

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
                logical: LogicalPredicates::from_static(&[]),
                structural: StructuralPredicates::from_static(&[]),
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
                    kind: AttributeSelectionKind::Exact,
                    case_sensitivity: AttributeCaseSensitivity::Default
                }]),
                logical: LogicalPredicates::from_static(&[]),
                structural: StructuralPredicates::from_static(&[]),
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
                        kind: AttributeSelectionKind::WhitespaceSeparated,
                        case_sensitivity: AttributeCaseSensitivity::Default
                    },
                    AttributeSelection {
                        name: "selected",
                        value: Some("true"),
                        kind: AttributeSelectionKind::Exact,
                        case_sensitivity: AttributeCaseSensitivity::Default
                    }
                ]),
                logical: LogicalPredicates::from_static(&[]),
                structural: StructuralPredicates::from_static(&[]),
            }
        );
    }

    #[test]
    fn test_duplicate_ids_are_rejected() {
        let mut reader = Reader::new("element#id.class[selected=true]#id#notid");
        let result = ElementPredicate::try_from(&mut reader);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().message(), "selector has multiple IDs");
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
                logical: LogicalPredicates::from_static(&[]),
                structural: StructuralPredicates::from_static(&[]),
            }
        );
    }

    #[test]
    fn selector_attribute_value_allows_escaped_double_quote() {
        let mut reader = Reader::new(r#"a[title="hello \"world\""]"#);
        let element = ElementPredicate::from(&mut reader);

        assert_eq!(element.attributes.as_slice().len(), 1);
        assert_eq!(element.attributes.as_slice()[0].name, "title");
        assert_eq!(
            element.attributes.as_slice()[0].value,
            Some(r#"hello \"world\""#)
        );
    }

    #[test]
    fn quoted_attribute_value_allows_equals() {
        let mut reader = Reader::new(r#"[data-x="a=b"]"#);
        let element = ElementPredicate::from(&mut reader);
        let attr = &element.attributes.as_slice()[0];
        assert_eq!(attr.name, "data-x");
        assert_eq!(attr.value, Some("a=b"));
    }

    #[test]
    fn quoted_attribute_value_allows_operator_characters() {
        for (selector, expected_value) in [
            (r#"[data-x="a*b"]"#, "a*b"),
            (r#"[data-x="a~b"]"#, "a~b"),
            (r#"[data-x="a|b"]"#, "a|b"),
            (r#"[data-x="a^b"]"#, "a^b"),
            (r#"[data-x="a$b"]"#, "a$b"),
        ] {
            let mut reader = Reader::new(selector);
            let element = ElementPredicate::from(&mut reader);
            let attr = &element.attributes.as_slice()[0];
            assert_eq!(attr.value, Some(expected_value), "{selector} should parse");
        }
    }

    #[test]
    fn quoted_attribute_value_allows_closing_bracket_character() {
        let mut reader = Reader::new(r#"[data-x="a]b"]"#);
        let element = ElementPredicate::from(&mut reader);
        let attr = &element.attributes.as_slice()[0];
        assert_eq!(attr.name, "data-x");
        assert_eq!(attr.value, Some("a]b"));
    }

    #[test]
    fn quoted_attribute_value_allows_url_query_string() {
        let mut reader = Reader::new(r#"[href="https://example.com/search?q=test"]"#);
        let element = ElementPredicate::from(&mut reader);
        let attr = &element.attributes.as_slice()[0];
        assert_eq!(attr.name, "href");
        assert_eq!(attr.value, Some("https://example.com/search?q=test"));
    }

    // ── CSS whitespace around attribute selector operators ──────

    #[test]
    fn attribute_selector_with_tab_around_equals_parses() {
        let mut reader = Reader::new("[data-x\t=\t\"value\"]");
        let element = ElementPredicate::from(&mut reader);
        let attr = &element.attributes.as_slice()[0];
        assert_eq!(attr.name, "data-x");
        assert_eq!(attr.value, Some("value"));
    }

    #[test]
    fn attribute_selector_with_newline_around_equals_parses() {
        let mut reader = Reader::new("[data-x\n=\n\"value\"]");
        let element = ElementPredicate::from(&mut reader);
        let attr = &element.attributes.as_slice()[0];
        assert_eq!(attr.name, "data-x");
        assert_eq!(attr.value, Some("value"));
    }

    #[test]
    fn attribute_selector_with_cr_around_equals_parses() {
        let mut reader = Reader::new("[data-x\r=\r\"value\"]");
        let element = ElementPredicate::from(&mut reader);
        let attr = &element.attributes.as_slice()[0];
        assert_eq!(attr.name, "data-x");
        assert_eq!(attr.value, Some("value"));
    }

    #[test]
    fn attribute_selector_with_form_feed_around_equals_parses() {
        let mut reader = Reader::new("[data-x\u{000C}=\u{000C}\"value\"]");
        let element = ElementPredicate::from(&mut reader);
        let attr = &element.attributes.as_slice()[0];
        assert_eq!(attr.name, "data-x");
        assert_eq!(attr.value, Some("value"));
    }

    #[test]
    fn attribute_selector_unquoted_value_with_css_whitespace_around_operator_parses() {
        // Unquoted value with tab around `=`.
        let mut reader = Reader::new("[data-x\t=\tvalue]");
        let element = ElementPredicate::from(&mut reader);
        let attr = &element.attributes.as_slice()[0];
        assert_eq!(attr.name, "data-x");
        assert_eq!(attr.value, Some("value"));
    }

    #[test]
    fn whitespace_inside_quoted_attribute_value_is_preserved() {
        let mut reader = Reader::new("[data-x=\"a   b\"]");
        let element = ElementPredicate::from(&mut reader);
        let attr = &element.attributes.as_slice()[0];
        assert_eq!(attr.name, "data-x");
        assert_eq!(attr.value, Some("a   b"));
    }

    #[test]
    fn uppercase_attribute_modifiers_are_accepted() {
        for (selector, expected) in [
            (
                r#"[data-x="FOO" I]"#,
                AttributeCaseSensitivity::AsciiInsensitive,
            ),
            (r#"[data-x="FOO" S]"#, AttributeCaseSensitivity::Sensitive),
        ] {
            let mut reader = Reader::new(selector);
            let element = ElementPredicate::try_from(&mut reader).unwrap();
            assert_eq!(element.attributes.as_slice()[0].case_sensitivity, expected);
        }
    }

    #[test]
    fn unquoted_i_and_s_attribute_values_are_not_mistaken_for_modifiers() {
        for (selector, expected_value, expected_kind, expected_case) in [
            (
                "[x=i]",
                "i",
                AttributeSelectionKind::Exact,
                AttributeCaseSensitivity::Default,
            ),
            (
                "[x=s]",
                "s",
                AttributeSelectionKind::Exact,
                AttributeCaseSensitivity::Default,
            ),
            (
                "[x=i i]",
                "i",
                AttributeSelectionKind::Exact,
                AttributeCaseSensitivity::AsciiInsensitive,
            ),
            (
                "[x=s i]",
                "s",
                AttributeSelectionKind::Exact,
                AttributeCaseSensitivity::AsciiInsensitive,
            ),
            (
                "[x~=i]",
                "i",
                AttributeSelectionKind::WhitespaceSeparated,
                AttributeCaseSensitivity::Default,
            ),
        ] {
            let mut reader = Reader::new(selector);
            let element = ElementPredicate::try_from(&mut reader).unwrap();
            let attribute = &element.attributes.as_slice()[0];
            assert_eq!(attribute.value, Some(expected_value), "{selector}");
            assert_eq!(attribute.kind, expected_kind, "{selector}");
            assert_eq!(attribute.case_sensitivity, expected_case, "{selector}");
        }
    }

    #[test]
    fn quoted_attribute_modifiers_are_rejected() {
        for selector in [r#"[data-x="FOO" "i"]"#, r#"[data-x="FOO" 's']"#] {
            let mut reader = Reader::new(selector);
            let error = ElementPredicate::try_from(&mut reader).unwrap_err();
            assert_eq!(
                error.message(),
                "attribute value modifier must be an unquoted identifier"
            );
        }
    }

    #[test]
    fn an_plus_b_accepts_css_whitespace_and_ascii_case_variants() {
        for (source, expected) in [
            ("3n + 1", AnPlusB { a: 3, b: 1 }),
            ("-n+ 6", AnPlusB { a: -1, b: 6 }),
            ("ODD", AnPlusB { a: 2, b: 1 }),
            ("2N+1", AnPlusB { a: 2, b: 1 }),
        ] {
            assert_eq!(parse_an_plus_b(source), Ok(expected), "{source}");
        }
    }

    #[test]
    fn an_plus_b_rejects_whitespace_that_changes_tokens() {
        for source in ["3 n", "+ 2n", "+ 2", "n 2"] {
            assert!(parse_an_plus_b(source).is_err(), "{source}");
        }
    }

    #[test]
    fn pseudo_class_names_are_ascii_case_insensitive() {
        for selector in [
            "li:FIRST-CHILD",
            "li:First-Of-Type",
            "li:NTH-CHILD(2n+1)",
            "li:nth-OF-type(2)",
            "div:NOT(.ad)",
            "div:Is(.card)",
            "div:WHERE(.card)",
            ":ROOT",
            ":SCOPE > a",
        ] {
            Transition::generate_transition_paths_from_string(selector)
                .unwrap_or_else(|error| panic!("{selector}: {error}"));
        }
    }

    #[test]
    fn structural_pseudos_in_local_selector_lists_are_rejected() {
        for selector in ["li:not(:first-child)", "li:nth-child(2 of :first-child)"] {
            let mut reader = Reader::new(selector);
            let error = ElementPredicate::try_from(&mut reader).unwrap_err();
            assert_eq!(
                error.message(),
                "structural pseudo-classes are not supported inside local selector lists"
            );
        }
    }

    #[test]
    fn is_and_where_discard_unsupported_alternatives() {
        for selector in [
            "div:is(.card, :has(a), :first-child)",
            "div:where(.card, :has(a), :nth-child(2))",
        ] {
            let mut reader = Reader::new(selector);
            let element = ElementPredicate::try_from(&mut reader).unwrap();
            let LocalLogicalPredicate::Any(alternatives) = &element.logical.as_slice()[0] else {
                panic!("{selector} did not compile to an any predicate");
            };
            assert_eq!(alternatives.as_slice().len(), 1, "{selector}");
            assert_eq!(
                alternatives.as_slice()[0].classes.as_slice(),
                &["card"],
                "{selector}"
            );
        }
    }

    #[test]
    fn is_with_only_unsupported_alternatives_matches_nothing() {
        let mut reader = Reader::new("div:is(:has(a), :first-child)");
        let element = ElementPredicate::try_from(&mut reader).unwrap();
        let LocalLogicalPredicate::Any(alternatives) = &element.logical.as_slice()[0] else {
            panic!("selector did not compile to an any predicate");
        };
        assert!(alternatives.as_slice().is_empty());
    }

    #[test]
    fn malformed_unicode_nth_arguments_return_errors() {
        for selector in ["li:nth-child(2 中)", "li:nth-of-type(2 中)"] {
            let mut reader = Reader::new(selector);
            assert!(ElementPredicate::try_from(&mut reader).is_err());
        }
    }
}
