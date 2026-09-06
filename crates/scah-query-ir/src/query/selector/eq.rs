use super::builder::{Attribute, AttributeSelection, ElementPredicate, IElement};
use super::string_search::AttributeSelectionKind;

impl<'a> AttributeSelection<'a> {
    pub fn matches_attribute(&self, other: &Attribute<'_>) -> bool {
        // Attribute names are case-insensitive in HTML.
        if !self.name.eq_ignore_ascii_case(other.key) {
            return false;
        }

        if self.value.is_none() || self.kind == AttributeSelectionKind::Presence {
            return true;
        }

        if other.value.is_none() {
            return false;
        }

        match self.case_sensitivity {
            super::string_search::AttributeCaseSensitivity::AsciiInsensitive => self
                .kind
                .find_ascii_insensitive(self.value.unwrap(), other.value.unwrap()),
            _ => self.kind.find(self.value.unwrap(), other.value.unwrap()),
        }
    }

    /// Match this selector against a value routed through one of the
    /// dedicated element fields (`id` / `class`), which are stored separately
    /// from the generic attribute list. A missing field never matches.
    fn matches_field(&self, field: Option<&str>) -> bool {
        let Some(value) = field else {
            return false;
        };

        if self.value.is_none() || self.kind == AttributeSelectionKind::Presence {
            return true;
        }

        match self.case_sensitivity {
            super::string_search::AttributeCaseSensitivity::AsciiInsensitive => {
                self.kind.find_ascii_insensitive(self.value.unwrap(), value)
            }
            _ => self.kind.find(self.value.unwrap(), value),
        }
    }
}

impl<'a> ElementPredicate<'a> {
    /// Whether the tag name alone can rule this predicate out.
    #[inline]
    pub fn matches_name(&self, name: &str) -> bool {
        self.name
            .is_none_or(|expected| expected.eq_ignore_ascii_case(name))
    }

    /// Whether evaluating this predicate needs `id`, `class`, or generic
    /// attributes in addition to the element name.
    #[inline]
    pub fn requires_attributes(&self) -> bool {
        self.id.is_some()
            || !self.classes.as_slice().is_empty()
            || !self.attributes.as_slice().is_empty()
            || self.logical.as_slice().iter().any(|logical| match logical {
                super::builder::LocalLogicalPredicate::Not(list)
                | super::builder::LocalLogicalPredicate::Any(list) => list
                    .as_slice()
                    .iter()
                    .any(ElementPredicate::requires_attributes),
            })
            || self
                .structural
                .as_slice()
                .iter()
                .any(|structural| match structural {
                    super::builder::StructuralPredicate::NthChildOf(_, filter) => filter
                        .as_slice()
                        .iter()
                        .any(ElementPredicate::requires_attributes),
                    _ => false,
                })
    }

    pub fn requires_structural(&self) -> bool {
        !self.structural.as_slice().is_empty()
            || self.logical.as_slice().iter().any(|logical| {
                let list = match logical {
                    super::builder::LocalLogicalPredicate::Not(list)
                    | super::builder::LocalLogicalPredicate::Any(list) => list.as_slice(),
                };
                list.iter().any(ElementPredicate::requires_structural)
            })
    }

    fn matches_classes(&self, element_classes: &str) -> bool {
        let selector_classes = self.classes.as_slice();
        match selector_classes.len() {
            0 => true,
            1 => element_classes
                .split_whitespace()
                .any(|word| word == selector_classes[0]),
            len if len <= u64::BITS as usize => {
                let mut matched = 0_u64;

                for word in element_classes.split_whitespace() {
                    for (index, selector_class) in selector_classes.iter().enumerate() {
                        if word == *selector_class {
                            matched |= 1 << index;
                        }
                    }
                }

                matched.count_ones() as usize == len
            }
            _ => {
                let mut matched = vec![false; selector_classes.len()];

                for word in element_classes.split_whitespace() {
                    for (index, selector_class) in selector_classes.iter().enumerate() {
                        if !matched[index] && word == *selector_class {
                            matched[index] = true;
                        }
                    }
                }

                matched.into_iter().all(std::convert::identity)
            }
        }
    }

    pub fn matches_element<'b, E: IElement<'b>>(&self, other: &E) -> bool {
        self.matches_element_with_context(other, None)
    }

    pub fn matches_element_with_context<'b, E: IElement<'b>>(
        &self,
        other: &E,
        structural: Option<super::builder::StructuralMatchContext>,
    ) -> bool {
        if !self.matches_name(other.name()) {
            return false;
        }

        if self.id.is_some() && self.id != other.id() {
            return false;
        }

        if !self.classes.as_slice().is_empty() {
            let Some(element_classes) = other.class() else {
                return false;
            };

            if !self.matches_classes(element_classes) {
                return false;
            }
        }

        self.attributes.as_slice().iter().all(|selector_attribute| {
            // `id` and `class` live in dedicated element fields, not the
            // generic attribute list, so route `[id]`/`[class]` selectors
            // there. Attribute names are case-insensitive in HTML. A rare
            // valueless `id`/`class` that landed in the attribute list is
            // still matched via the fallback scan.
            if selector_attribute.name.eq_ignore_ascii_case("id") {
                selector_attribute.matches_field(other.id())
                    || other
                        .attributes()
                        .iter()
                        .any(|attribute| selector_attribute.matches_attribute(attribute))
            } else if selector_attribute.name.eq_ignore_ascii_case("class") {
                selector_attribute.matches_field(other.class())
                    || other
                        .attributes()
                        .iter()
                        .any(|attribute| selector_attribute.matches_attribute(attribute))
            } else {
                other
                    .attributes()
                    .iter()
                    .any(|xhtml_attribute| selector_attribute.matches_attribute(xhtml_attribute))
            }
        }) && self.logical.as_slice().iter().all(|logical| match logical {
            super::builder::LocalLogicalPredicate::Not(list) => !list
                .as_slice()
                .iter()
                .any(|predicate| predicate.matches_element(other)),
            super::builder::LocalLogicalPredicate::Any(list) => list
                .as_slice()
                .iter()
                .any(|predicate| predicate.matches_element(other)),
        }) && self.structural.as_slice().iter().all(|predicate| {
            let Some(context) = structural else {
                return false;
            };
            match predicate {
                super::builder::StructuralPredicate::Root => context.is_root,
                super::builder::StructuralPredicate::Scope => context.is_root,
                super::builder::StructuralPredicate::FirstChild => context.child_index == 1,
                super::builder::StructuralPredicate::NthChild(formula) => {
                    formula.matches(context.child_index)
                }
                super::builder::StructuralPredicate::FirstOfType => context.type_index == 1,
                super::builder::StructuralPredicate::NthOfType(formula) => {
                    formula.matches(context.type_index)
                }
                super::builder::StructuralPredicate::NthChildOf(formula, filter) => {
                    let key = filter as *const _ as usize;
                    let matched_slot =
                        (0..8).find(|&slot| context.filtered_child_keys[slot] == key);
                    matched_slot
                        .is_some_and(|slot| formula.matches(context.filtered_child_indices[slot]))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AttributeSelections, ClassSelections};

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
    fn test_attribute_selection_comparison() {
        assert!(
            AttributeSelection {
                name: "hello",
                value: Some("World"),
                kind: AttributeSelectionKind::Exact,
                case_sensitivity: crate::AttributeCaseSensitivity::Default,
            }
            .matches_attribute(&Attribute {
                key: "hello",
                value: Some("World")
            })
        );
    }

    #[test]
    fn test_element_selection_comparison() {
        assert!(
            ElementPredicate {
                name: Some("hello"),
                id: Some("id"),
                classes: ClassSelections::from_static(&["world"]),
                attributes: AttributeSelections::from(vec![AttributeSelection {
                    name: "selected",
                    value: Some("true"),
                    kind: AttributeSelectionKind::Exact,
                    case_sensitivity: crate::AttributeCaseSensitivity::Default
                }]),
                logical: crate::LogicalPredicates::from_static(&[]),
                structural: crate::StructuralPredicates::from_static(&[]),
            }
            .matches_element(&FakeElement {
                name: "hello",
                id: Some("id"),
                class: Some("hello world"),
                attributes: &[
                    Attribute {
                        key: "key1",
                        value: Some("value1")
                    },
                    Attribute {
                        key: "key2",
                        value: Some("value2")
                    },
                    Attribute {
                        key: "selected",
                        value: Some("true")
                    },
                ]
            })
        );
    }

    #[test]
    fn test_realistic_search() {
        assert!(
            ElementPredicate {
                name: Some("a"),
                id: None,
                classes: ClassSelections::from_static(&["underline-green"]),
                attributes: AttributeSelections::from(vec![AttributeSelection {
                    name: "href",
                    value: None,
                    kind: AttributeSelectionKind::Presence,
                    case_sensitivity: crate::AttributeCaseSensitivity::Default
                }]),
                logical: crate::LogicalPredicates::from_static(&[]),
                structural: crate::StructuralPredicates::from_static(&[]),
            }
            .matches_element(&FakeElement {
                name: "a",
                id: Some("search-link"),
                class: Some("text-white underline-green p-4"),
                attributes: &[
                    Attribute {
                        key: "key1",
                        value: Some("value1")
                    },
                    Attribute {
                        key: "href",
                        value: Some("/search")
                    },
                    Attribute {
                        key: "selected",
                        value: Some("true")
                    },
                ]
            })
        );
    }

    #[test]
    fn test_multiple_class_selection_comparison() {
        assert!(
            ElementPredicate {
                name: Some("a"),
                id: None,
                classes: ClassSelections::from_static(&["blue", "exit"]),
                attributes: AttributeSelections::from_static(&[]),
                logical: crate::LogicalPredicates::from_static(&[]),
                structural: crate::StructuralPredicates::from_static(&[])
            }
            .matches_element(&FakeElement {
                name: "a",
                id: None,
                class: Some("blue large exit"),
                attributes: &[],
            })
        );
    }

    #[test]
    fn test_multiple_class_selection_comparison_is_order_independent() {
        assert!(
            ElementPredicate {
                name: Some("a"),
                id: None,
                classes: ClassSelections::from_static(&["exit", "blue"]),
                attributes: AttributeSelections::from_static(&[]),
                logical: crate::LogicalPredicates::from_static(&[]),
                structural: crate::StructuralPredicates::from_static(&[])
            }
            .matches_element(&FakeElement {
                name: "a",
                id: None,
                class: Some("blue large exit"),
                attributes: &[],
            })
        );
    }

    #[test]
    fn test_multiple_class_selection_comparison_requires_all_classes() {
        assert!(
            !ElementPredicate {
                name: Some("a"),
                id: None,
                classes: ClassSelections::from_static(&["blue", "exit", "missing"]),
                attributes: AttributeSelections::from_static(&[]),
                logical: crate::LogicalPredicates::from_static(&[]),
                structural: crate::StructuralPredicates::from_static(&[])
            }
            .matches_element(&FakeElement {
                name: "a",
                id: None,
                class: Some("blue large exit"),
                attributes: &[],
            })
        );
    }

    #[test]
    fn test_class_matching_is_order_independent_for_selector_and_element() {
        let selector_one = ElementPredicate {
            name: Some("a"),
            id: None,
            classes: ClassSelections::from_static(&["blue", "exit"]),
            attributes: AttributeSelections::from_static(&[]),
            logical: crate::LogicalPredicates::from_static(&[]),
            structural: crate::StructuralPredicates::from_static(&[]),
        };
        let selector_two = ElementPredicate {
            name: Some("a"),
            id: None,
            classes: ClassSelections::from_static(&["exit", "blue"]),
            attributes: AttributeSelections::from_static(&[]),
            logical: crate::LogicalPredicates::from_static(&[]),
            structural: crate::StructuralPredicates::from_static(&[]),
        };

        let element_one = FakeElement {
            name: "a",
            id: None,
            class: Some("blue exit"),
            attributes: &[],
        };
        let element_two = FakeElement {
            name: "a",
            id: None,
            class: Some("exit blue"),
            attributes: &[],
        };

        assert!(selector_one.matches_element(&element_one));
        assert!(selector_one.matches_element(&element_two));
        assert!(selector_two.matches_element(&element_one));
        assert!(selector_two.matches_element(&element_two));
    }
}
