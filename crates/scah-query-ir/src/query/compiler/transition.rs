use crate::Reader;
use crate::query::compiler::SelectorParseError;
use crate::query::selector::{Combinator, ElementPredicate, IElement, Lexer};

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
        let mut attribute_names = Vec::new();
        for attribute in predicate.attributes.as_slice() {
            if attribute.name.eq_ignore_ascii_case("id")
                || attribute.name.eq_ignore_ascii_case("class")
                || attribute_names
                    .iter()
                    .any(|name: &&str| name.eq_ignore_ascii_case(attribute.name))
            {
                continue;
            }
            attribute_names.push(attribute.name);
        }

        Self {
            name: predicate.name,
            name_hash: predicate.name.map_or(0, ascii_case_insensitive_hash),
            needs_id: predicate.id.is_some()
                || predicate
                    .attributes
                    .as_slice()
                    .iter()
                    .any(|attribute| attribute.name.eq_ignore_ascii_case("id")),
            needs_class: !predicate.classes.as_slice().is_empty()
                || predicate
                    .attributes
                    .as_slice()
                    .iter()
                    .any(|attribute| attribute.name.eq_ignore_ascii_case("class")),
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

        let attributes = predicate.attributes.as_slice();
        let needs_id = predicate.id.is_some() || has_attribute_named(attributes, "id");
        let needs_class =
            !predicate.classes.as_slice().is_empty() || has_attribute_named(attributes, "class");
        if self.needs_id != needs_id || self.needs_class != needs_class {
            return false;
        }

        let metadata_names = self.attribute_names.as_slice();
        let mut metadata_index = 0;
        let mut attribute_index = 0;
        while attribute_index < attributes.len() {
            let attribute_name = attributes[attribute_index].name;
            if !is_metadata_attribute(attribute_name)
                && !has_previous_attribute(attributes, attribute_index, attribute_name)
            {
                if metadata_index >= metadata_names.len()
                    || !const_ascii_case_insensitive_eq(
                        metadata_names[metadata_index],
                        attribute_name,
                    )
                {
                    return false;
                }
                metadata_index += 1;
            }
            attribute_index += 1;
        }

        metadata_index == metadata_names.len()
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
        let reader = &mut Reader::new(query);
        let mut states = Vec::new();
        while let Some((combinator, element)) = Lexer::try_next(reader)? {
            states.push(Self::new(combinator, element));
        }

        if states.is_empty() {
            return Err(SelectorParseError::new("empty selector", 0));
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

        self.guard.evaluate(last_depth, current_depth) && self.predicate.matches_element(element)
    }

    #[allow(clippy::needless_lifetimes)]
    pub fn back<'html>(&self, _element: &'html str, current_depth: u16, last_depth: u16) -> bool {
        last_depth == current_depth
    }
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
                    },
                    AttributeSelection {
                        name: "HREF",
                        value: None,
                        kind: AttributeSelectionKind::Presence,
                    },
                    AttributeSelection {
                        name: "class",
                        value: None,
                        kind: AttributeSelectionKind::Presence,
                    },
                ]),
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
            },
        );

        transition.set_predicate(ElementPredicate {
            name: Some("div"),
            id: Some("hero"),
            classes: ClassSelections::from_static(&[]),
            attributes: AttributeSelections::from_static(&[]),
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
            }]),
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
}
