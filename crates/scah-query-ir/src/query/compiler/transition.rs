use crate::Reader;
use crate::query::compiler::SelectorParseError;
use crate::query::selector::{
    Combinator, ElementPredicate, IElement, Lexer, StructuralMatchContext,
};

#[inline]
pub const fn ascii_case_insensitive_hash(value: &str) -> u64 {
    let bytes = value.as_bytes();
    let mut index = 0;
    let mut hash = 0xcbf2_9ce4_8422_2325;
    while index < bytes.len() {
        let byte = bytes[index];
        let lower = if byte >= b'A' && byte <= b'Z' {
            byte + (b'a' - b'A')
        } else {
            byte
        };
        hash = (hash ^ lower as u64).wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum AttributeNames<'query> {
    Static(&'query [&'query str]),
    Owned(Box<[&'query str]>),
}

impl<'query> AttributeNames<'query> {
    pub const fn from_static(names: &'query [&'query str]) -> Self {
        Self::Static(names)
    }

    pub const fn as_slice(&self) -> &[&'query str] {
        match self {
            Self::Static(names) => names,
            Self::Owned(names) => names,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PredicateMetadata<'query> {
    name: Option<&'query str>,
    name_hash: u64,
    needs_id: bool,
    needs_class: bool,
    attribute_names: AttributeNames<'query>,
}

impl<'query> PredicateMetadata<'query> {
    pub fn compile(predicate: &ElementPredicate<'query>) -> Self {
        let name = predicate.name;
        let mut attribute_names = Vec::new();
        let mut needs_id = false;
        let mut needs_class = false;
        collect_metadata(
            predicate,
            &mut attribute_names,
            &mut needs_id,
            &mut needs_class,
        );
        needs_id |= predicate_needs_id(predicate);
        needs_class |= predicate_needs_class(predicate);

        Self {
            name,
            name_hash: name.map_or(0, ascii_case_insensitive_hash),
            needs_id,
            needs_class,
            attribute_names: AttributeNames::Owned(attribute_names.into_boxed_slice()),
        }
    }

    #[doc(hidden)]
    pub const fn new_const(
        name: Option<&'query str>,
        needs_id: bool,
        needs_class: bool,
        attribute_names: AttributeNames<'query>,
    ) -> Self {
        Self {
            name,
            name_hash: match name {
                Some(name) => ascii_case_insensitive_hash(name),
                None => 0,
            },
            needs_id,
            needs_class,
            attribute_names,
        }
    }

    const fn matches_predicate(&self, predicate: &ElementPredicate<'query>) -> bool {
        let names_match = match (self.name, predicate.name) {
            (Some(metadata_name), Some(predicate_name)) => {
                const_ascii_case_insensitive_eq(metadata_name, predicate_name)
            }
            (None, None) => true,
            _ => false,
        };
        let predicate_name_hash = match predicate.name {
            Some(name) => ascii_case_insensitive_hash(name),
            None => 0,
        };
        if !names_match || self.name_hash != predicate_name_hash {
            return false;
        }

        let needs_id = predicate_needs_id(predicate);
        let needs_class = predicate_needs_class(predicate);
        if self.needs_id != needs_id || self.needs_class != needs_class {
            return false;
        }

        let metadata_names = self.attribute_names.as_slice();
        let mut metadata_index = 0;
        metadata_matches_predicate(predicate, metadata_names, &mut metadata_index)
            && metadata_index == metadata_names.len()
    }

    #[inline]
    pub fn matches_name(&self, name: &str, name_hash: u64) -> bool {
        self.name.is_none_or(|expected| {
            expected.len() == name.len()
                && self.name_hash == name_hash
                && expected.eq_ignore_ascii_case(name)
        })
    }

    #[inline]
    pub fn needs_id(&self) -> bool {
        self.needs_id
    }

    #[inline]
    pub fn needs_class(&self) -> bool {
        self.needs_class
    }

    #[inline]
    pub fn attribute_names(&self) -> &[&'query str] {
        self.attribute_names.as_slice()
    }
}

fn collect_metadata<'query>(
    predicate: &ElementPredicate<'query>,
    attribute_names: &mut Vec<&'query str>,
    needs_id: &mut bool,
    needs_class: &mut bool,
) {
    *needs_id |= predicate.id.is_some();
    *needs_class |= !predicate.classes.as_slice().is_empty();
    for attribute in predicate.attributes.as_slice() {
        if attribute.name.eq_ignore_ascii_case("id") {
            *needs_id = true;
        } else if attribute.name.eq_ignore_ascii_case("class") {
            *needs_class = true;
        } else if !attribute_names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(attribute.name))
        {
            attribute_names.push(attribute.name);
        }
    }
    for logical in predicate.logical.as_slice() {
        let lists = match logical {
            crate::query::selector::LocalLogicalPredicate::Not(list)
            | crate::query::selector::LocalLogicalPredicate::Any(list) => list.as_slice(),
        };
        for local in lists {
            collect_metadata(local, attribute_names, needs_id, needs_class);
        }
    }
}

const fn predicate_needs_id(predicate: &ElementPredicate<'_>) -> bool {
    if predicate.id.is_some() || has_attribute_named(predicate.attributes.as_slice(), "id") {
        return true;
    }
    let logical = predicate.logical.as_slice();
    let mut index = 0;
    while index < logical.len() {
        let lists = match &logical[index] {
            crate::query::selector::LocalLogicalPredicate::Not(list)
            | crate::query::selector::LocalLogicalPredicate::Any(list) => list.as_slice(),
        };
        let mut local = 0;
        while local < lists.len() {
            if predicate_needs_id(&lists[local]) {
                return true;
            }
            local += 1;
        }
        index += 1;
    }
    false
}

const fn predicate_needs_class(predicate: &ElementPredicate<'_>) -> bool {
    if !predicate.classes.as_slice().is_empty()
        || has_attribute_named(predicate.attributes.as_slice(), "class")
    {
        return true;
    }
    let structural = predicate.structural.as_slice();
    let mut structural_index = 0;
    while structural_index < structural.len() {
        if let crate::query::selector::StructuralPredicate::NthChildOf(_, filter) =
            &structural[structural_index]
        {
            let filters = filter.as_slice();
            let mut filter_index = 0;
            while filter_index < filters.len() {
                if predicate_needs_class(&filters[filter_index]) {
                    return true;
                }
                filter_index += 1;
            }
        }
        structural_index += 1;
    }
    let logical = predicate.logical.as_slice();
    let mut index = 0;
    while index < logical.len() {
        let lists = match &logical[index] {
            crate::query::selector::LocalLogicalPredicate::Not(list)
            | crate::query::selector::LocalLogicalPredicate::Any(list) => list.as_slice(),
        };
        let mut local = 0;
        while local < lists.len() {
            if predicate_needs_class(&lists[local]) {
                return true;
            }
            local += 1;
        }
        index += 1;
    }
    false
}

const fn metadata_matches_predicate(
    predicate: &ElementPredicate<'_>,
    metadata_names: &[&str],
    metadata_index: &mut usize,
) -> bool {
    if !metadata_matches_attributes(
        predicate.attributes.as_slice(),
        metadata_names,
        metadata_index,
    ) {
        return false;
    }
    let logical = predicate.logical.as_slice();
    let mut index = 0;
    while index < logical.len() {
        let lists = match &logical[index] {
            crate::query::selector::LocalLogicalPredicate::Not(list)
            | crate::query::selector::LocalLogicalPredicate::Any(list) => list.as_slice(),
        };
        let mut local = 0;
        while local < lists.len() {
            if !metadata_matches_predicate(&lists[local], metadata_names, metadata_index) {
                return false;
            }
            local += 1;
        }
        index += 1;
    }
    true
}

const fn metadata_matches_attributes(
    attributes: &[crate::query::selector::AttributeSelection<'_>],
    metadata_names: &[&str],
    metadata_index: &mut usize,
) -> bool {
    let mut attribute_index = 0;
    while attribute_index < attributes.len() {
        let attribute_name = attributes[attribute_index].name;
        if !is_metadata_attribute(attribute_name)
            && !has_previous_attribute(attributes, attribute_index, attribute_name)
        {
            if *metadata_index >= metadata_names.len()
                || !const_ascii_case_insensitive_eq(metadata_names[*metadata_index], attribute_name)
            {
                return false;
            }
            *metadata_index += 1;
        }
        attribute_index += 1;
    }
    true
}

#[derive(PartialEq, Debug, Clone)]
pub struct Transition<'query> {
    pub guard: Combinator,
    predicate: ElementPredicate<'query>,
    metadata: PredicateMetadata<'query>,
}

impl<'query> Transition<'query> {
    pub fn new(guard: Combinator, predicate: ElementPredicate<'query>) -> Self {
        let metadata = PredicateMetadata::compile(&predicate);
        Self {
            guard,
            predicate,
            metadata,
        }
    }

    /// Constructs a transition for a generated static query.
    ///
    /// # Panics
    ///
    /// Panics when `metadata` does not describe `predicate`.
    #[doc(hidden)]
    pub const fn new_const(
        guard: Combinator,
        predicate: ElementPredicate<'query>,
        metadata: PredicateMetadata<'query>,
    ) -> Self {
        assert!(
            metadata.matches_predicate(&predicate),
            "predicate metadata does not match predicate"
        );
        Self {
            guard,
            predicate,
            metadata,
        }
    }

    #[inline]
    pub const fn predicate(&self) -> &ElementPredicate<'query> {
        &self.predicate
    }

    #[inline]
    pub const fn metadata(&self) -> &PredicateMetadata<'query> {
        &self.metadata
    }

    /// Replace the predicate and atomically refresh its compiled metadata.
    pub fn set_predicate(&mut self, predicate: ElementPredicate<'query>) {
        let metadata = PredicateMetadata::compile(&predicate);
        self.predicate = predicate;
        self.metadata = metadata;
    }

    pub fn generate_transitions_from_string(
        query: &'query str,
    ) -> Result<Vec<Self>, SelectorParseError> {
        let paths = Self::generate_transition_paths_from_string(query)?;
        if paths.len() != 1 {
            return Err(SelectorParseError::new(
                "selector list requires a query builder",
                0,
            ));
        }
        Ok(paths.into_iter().next().unwrap())
    }

    pub fn generate_transition_paths_from_string(
        query: &'query str,
    ) -> Result<Vec<Vec<Self>>, SelectorParseError> {
        if query.bytes().any(|byte| byte == 0x0b) {
            return Err(SelectorParseError::new("illegal selector token", 0));
        }
        let alternatives = split_selector_list(query)?;
        let mut paths = Vec::with_capacity(alternatives.len());
        for alternative in alternatives {
            paths.push(Self::generate_single_path(alternative)?);
        }
        Ok(paths)
    }

    fn generate_single_path(query: &'query str) -> Result<Vec<Self>, SelectorParseError> {
        if query.bytes().any(|byte| byte == 0x0b) {
            return Err(SelectorParseError::new("illegal selector token", 0));
        }
        let reader = &mut Reader::new(query);
        let mut states = Vec::new();
        let mut seen_selector = false;
        while let Some((combinator, element)) = Lexer::try_next(reader, seen_selector)? {
            states.push(Self::new(combinator, element));
            seen_selector = true;
        }

        if !reader.eof() {
            return Err(SelectorParseError::new(
                "illegal selector token",
                reader.get_position(),
            ));
        }

        if states.is_empty() {
            return Err(SelectorParseError::new("empty selector", 0));
        }

        // `:scope` is a zero-width anchor for nested sections. Once a path
        // continues past it, the existing cursor parent already is the scope;
        // retaining a synthetic element transition would incorrectly require
        // the scope to appear again in the stream.
        if states.len() > 1
            && states[0]
                .predicate
                .structural
                .as_slice()
                .iter()
                .all(|predicate| matches!(predicate, crate::StructuralPredicate::Scope))
            && states[0].predicate.name.is_none()
            && states[0].predicate.id.is_none()
            && states[0].predicate.classes.as_slice().is_empty()
            && states[0].predicate.attributes.as_slice().is_empty()
            && states[0].predicate.logical.as_slice().is_empty()
        {
            states.remove(0);
        }

        Ok(states)
    }

    pub fn next<'html, E: IElement<'html>>(
        &self,
        element: &E,
        current_depth: u16,
        last_depth: u16,
    ) -> bool {
        assert!(
            current_depth >= last_depth,
            "Current depth is smaller than last depth: {current_depth} >= {last_depth}"
        );

        self.next_with_context(element, current_depth, last_depth, None)
    }

    pub fn next_with_context<'html, E: IElement<'html>>(
        &self,
        element: &E,
        current_depth: u16,
        last_depth: u16,
        structural: Option<StructuralMatchContext>,
    ) -> bool {
        assert!(
            current_depth >= last_depth,
            "Current depth is smaller than last depth: {current_depth} >= {last_depth}"
        );
        self.guard.evaluate(last_depth, current_depth)
            && self
                .predicate
                .matches_element_with_context(element, structural)
    }

    #[allow(clippy::needless_lifetimes)]
    pub fn back<'html>(&self, _element: &'html str, current_depth: u16, last_depth: u16) -> bool {
        last_depth == current_depth
    }
}

fn split_selector_list<'query>(
    source: &'query str,
) -> Result<Vec<&'query str>, SelectorParseError> {
    let mut parts = Vec::new();
    let bytes = source.as_bytes();
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
            b'[' | b'(' => depth += 1,
            b']' | b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(source[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(source[start..].trim());
    if parts.iter().any(|part| part.is_empty()) {
        return Err(SelectorParseError::new(
            "selector list has an empty alternative",
            0,
        ));
    }
    Ok(parts)
}

const fn const_ascii_case_insensitive_eq(left: &str, right: &str) -> bool {
    let left_bytes = left.as_bytes();
    let right_bytes = right.as_bytes();
    if left_bytes.len() != right_bytes.len() {
        return false;
    }

    let mut index = 0;
    while index < left_bytes.len() {
        let left_byte = left_bytes[index];
        let right_byte = right_bytes[index];
        let left_lower = if left_byte.is_ascii_uppercase() {
            left_byte + (b'a' - b'A')
        } else {
            left_byte
        };
        let right_lower = if right_byte.is_ascii_uppercase() {
            right_byte + (b'a' - b'A')
        } else {
            right_byte
        };
        if left_lower != right_lower {
            return false;
        }
        index += 1;
    }

    true
}

const fn has_attribute_named(
    attributes: &[crate::query::selector::AttributeSelection<'_>],
    expected: &str,
) -> bool {
    let mut index = 0;
    while index < attributes.len() {
        if const_ascii_case_insensitive_eq(attributes[index].name, expected) {
            return true;
        }
        index += 1;
    }
    false
}

const fn is_metadata_attribute(name: &str) -> bool {
    const_ascii_case_insensitive_eq(name, "id") || const_ascii_case_insensitive_eq(name, "class")
}

const fn has_previous_attribute(
    attributes: &[crate::query::selector::AttributeSelection<'_>],
    end: usize,
    name: &str,
) -> bool {
    let mut index = 0;
    while index < end {
        if !is_metadata_attribute(attributes[index].name)
            && const_ascii_case_insensitive_eq(attributes[index].name, name)
        {
            return true;
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::query::selector::{
        Attribute, AttributeSelection, AttributeSelectionKind, AttributeSelections,
        ClassSelections, IElement,
    };

    use super::*;

    #[derive(Debug)]
    struct FakeElement<'a> {
        name: &'a str,
        id: Option<&'a str>,
        class: Option<&'a str>,
        attributes: &'a [Attribute<'a>],
    }

    impl<'a> IElement<'a> for FakeElement<'a> {
        fn name(&self) -> &'a str {
            self.name
        }

        fn id(&self) -> Option<&'a str> {
            self.id
        }

        fn class(&self) -> Option<&'a str> {
            self.class
        }

        fn attributes(&self) -> &[Attribute<'a>] {
            self.attributes
        }
    }

    #[test]
    fn predicate_metadata_compiles_unique_attribute_interests() {
        let transition = Transition::new(
            Combinator::Descendant,
            ElementPredicate {
                name: Some("ARTICLE"),
                id: Some("hero"),
                classes: ClassSelections::from_static(&["featured"]),
                attributes: AttributeSelections::from(vec![
                    AttributeSelection {
                        name: "href",
                        value: None,
                        kind: AttributeSelectionKind::Presence,
                        case_sensitivity: crate::query::selector::AttributeCaseSensitivity::Default,
                    },
                    AttributeSelection {
                        name: "HREF",
                        value: None,
                        kind: AttributeSelectionKind::Presence,
                        case_sensitivity: crate::query::selector::AttributeCaseSensitivity::Default,
                    },
                    AttributeSelection {
                        name: "class",
                        value: None,
                        kind: AttributeSelectionKind::Presence,
                        case_sensitivity: crate::query::selector::AttributeCaseSensitivity::Default,
                    },
                ]),
                logical: crate::LogicalPredicates::from_static(&[]),
                structural: crate::StructuralPredicates::from_static(&[]),
            },
        );

        assert!(
            transition
                .metadata()
                .matches_name("article", ascii_case_insensitive_hash("article"))
        );
        assert!(transition.metadata().needs_id());
        assert!(transition.metadata().needs_class());
        assert_eq!(transition.metadata().attribute_names(), &["href"]);
    }

    #[test]
    fn replacing_predicate_refreshes_metadata() {
        let mut transition = Transition::new(
            Combinator::Descendant,
            ElementPredicate {
                name: Some("a"),
                id: None,
                classes: ClassSelections::from_static(&[]),
                attributes: AttributeSelections::from_static(&[]),
                logical: crate::LogicalPredicates::from_static(&[]),
                structural: crate::StructuralPredicates::from_static(&[]),
            },
        );

        transition.set_predicate(ElementPredicate {
            name: Some("div"),
            id: Some("hero"),
            classes: ClassSelections::from_static(&[]),
            attributes: AttributeSelections::from_static(&[]),
            logical: crate::LogicalPredicates::from_static(&[]),
            structural: crate::StructuralPredicates::from_static(&[]),
        });

        assert_eq!(transition.predicate().name, Some("div"));
        assert!(
            transition
                .metadata()
                .matches_name("div", ascii_case_insensitive_hash("div"))
        );
        assert!(
            !transition
                .metadata()
                .matches_name("a", ascii_case_insensitive_hash("a"))
        );
        assert!(transition.metadata().needs_id());
    }

    #[test]
    #[should_panic(expected = "predicate metadata does not match predicate")]
    fn const_constructor_rejects_inconsistent_metadata() {
        let predicate = ElementPredicate {
            name: Some("a"),
            id: None,
            classes: ClassSelections::from_static(&[]),
            attributes: AttributeSelections::from_static(&[AttributeSelection {
                name: "href",
                value: None,
                kind: AttributeSelectionKind::Presence,
                case_sensitivity: crate::query::selector::AttributeCaseSensitivity::Default,
            }]),
            logical: crate::LogicalPredicates::from_static(&[]),
            structural: crate::StructuralPredicates::from_static(&[]),
        };
        let metadata =
            PredicateMetadata::new_const(Some("a"), false, false, AttributeNames::from_static(&[]));

        let _ = Transition::new_const(Combinator::Descendant, predicate, metadata);
    }

    #[test]
    fn test_fsm_next_descendant() {
        let state = Transition::new(
            Combinator::Descendant,
            ElementPredicate {
                name: Some("a"),
                id: None,
                classes: ClassSelections::from_static(&[]),
                attributes: AttributeSelections::from_static(&[]),
                logical: crate::LogicalPredicates::from_static(&[]),
                structural: crate::StructuralPredicates::from_static(&[]),
            },
        );
        assert!(state.next(
            &FakeElement {
                name: "a",
                id: None,
                class: None,
                attributes: &[],
            },
            4,
            1,
        ));
    }

    #[test]
    fn test_fsm_next_child() {
        let state = Transition::new(
            Combinator::Child,
            ElementPredicate {
                name: Some("a"),
                id: None,
                classes: ClassSelections::from_static(&[]),
                attributes: AttributeSelections::from_static(&[]),
                logical: crate::LogicalPredicates::from_static(&[]),
                structural: crate::StructuralPredicates::from_static(&[]),
            },
        );
        assert!(state.next(
            &FakeElement {
                name: "a",
                id: None,
                class: None,
                attributes: &[],
            },
            2,
            1,
        ));
    }

    #[test]
    fn test_fsm_next_child_failed() {
        let state = Transition::new(
            Combinator::Child,
            ElementPredicate {
                name: Some("a"),
                id: None,
                classes: ClassSelections::from_static(&[]),
                attributes: AttributeSelections::from_static(&[]),
                logical: crate::LogicalPredicates::from_static(&[]),
                structural: crate::StructuralPredicates::from_static(&[]),
            },
        );
        assert!(!state.next(
            &FakeElement {
                name: "a",
                id: None,
                class: None,
                attributes: &[],
            },
            4,
            1,
        ));
    }

    #[test]
    fn test_fsm_next_sibling_same_depth() {
        let adjacent = Transition::new(
            Combinator::NextSibling,
            ElementPredicate {
                name: Some("p"),
                id: None,
                classes: ClassSelections::from_static(&[]),
                attributes: AttributeSelections::from_static(&[]),
                logical: crate::LogicalPredicates::from_static(&[]),
                structural: crate::StructuralPredicates::from_static(&[]),
            },
        );
        let subsequent = Transition::new(
            Combinator::SubsequentSibling,
            ElementPredicate {
                name: Some("p"),
                id: None,
                classes: ClassSelections::from_static(&[]),
                attributes: AttributeSelections::from_static(&[]),
                logical: crate::LogicalPredicates::from_static(&[]),
                structural: crate::StructuralPredicates::from_static(&[]),
            },
        );
        let element = FakeElement {
            name: "p",
            id: None,
            class: None,
            attributes: &[],
        };

        assert!(adjacent.next(&element, 3, 3));
        assert!(subsequent.next(&element, 3, 3));
        assert!(!adjacent.next(&element, 4, 3));
        assert!(!subsequent.next(&element, 4, 3));
    }

    #[test]
    fn sibling_selectors_compile_to_expected_guards() {
        let states = Transition::generate_transitions_from_string("main > div ~ p > span").unwrap();
        let guards: Vec<_> = states.iter().map(|s| s.guard.clone()).collect();
        assert_eq!(
            guards,
            [
                Combinator::Descendant,
                Combinator::Child,
                Combinator::SubsequentSibling,
                Combinator::Child,
            ]
        );
    }
}
