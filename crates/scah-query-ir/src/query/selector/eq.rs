use super::builder::{Attribute, AttributeSelection, ElementPredicate, IElement};
use super::string_search::AttributeSelectionKind;

impl<'a> AttributeSelection<'a> {
    pub fn matches_attribute(&self, other: &Attribute<'_>) -> bool {
        if !self.name.eq_ignore_ascii_case(other.key) {
            return false;
        }

        if self.value.is_none() || self.kind == AttributeSelectionKind::Presence {
            return true;
        }

        if other.value.is_none() {
            return false;
        }

        self.kind.find(self.value.unwrap(), other.value.unwrap())
    }
}

impl<'a> ElementPredicate<'a> {
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
        if let Some(name) = self.name
            && name != other.name()
        {
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
            // Check the dedicated id/class fields first, since they are stored
            // separately from the generic attribute tape.
            match selector_attribute.name {
                "id" => match other.id() {
                    Some(id) => selector_attribute.matches_attribute(&Attribute {
                        key: "id",
                        value: Some(id),
                    }),
                    None => false,
                },
                "class" => match other.class() {
                    Some(class) => selector_attribute.matches_attribute(&Attribute {
                        key: "class",
                        value: Some(class),
                    }),
                    None => false,
                },
                _ => other
                    .attributes()
                    .iter()
                    .any(|xhtml_attribute| selector_attribute.matches_attribute(xhtml_attribute)),
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
                    kind: AttributeSelectionKind::Exact
                }])
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
                }])
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
                attributes: AttributeSelections::from_static(&[])
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
                attributes: AttributeSelections::from_static(&[])
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
                attributes: AttributeSelections::from_static(&[])
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
        };
        let selector_two = ElementPredicate {
            name: Some("a"),
            id: None,
            classes: ClassSelections::from_static(&["exit", "blue"]),
            attributes: AttributeSelections::from_static(&[]),
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

    // --- Edge Case #4: [id] and [class] attribute selectors ---

    #[test]
    fn test_id_attribute_selector_presence() {
        // [id] should match elements that have an id
        let selector = ElementPredicate {
            name: Some("div"),
            id: None,
            classes: ClassSelections::from_static(&[]),
            attributes: AttributeSelections::from(vec![AttributeSelection {
                name: "id",
                value: None,
                kind: AttributeSelectionKind::Presence,
            }]),
        };
        assert!(selector.matches_element(&FakeElement {
            name: "div",
            id: Some("myid"),
            class: None,
            attributes: &[],
        }));
        assert!(!selector.matches_element(&FakeElement {
            name: "div",
            id: None,
            class: None,
            attributes: &[],
        }));
    }

    #[test]
    fn test_id_attribute_selector_exact() {
        // [id="x"] should match element with id="x"
        let selector = ElementPredicate {
            name: None,
            id: None,
            classes: ClassSelections::from_static(&[]),
            attributes: AttributeSelections::from(vec![AttributeSelection {
                name: "id",
                value: Some("target"),
                kind: AttributeSelectionKind::Exact,
            }]),
        };
        assert!(selector.matches_element(&FakeElement {
            name: "div",
            id: Some("target"),
            class: None,
            attributes: &[],
        }));
        assert!(!selector.matches_element(&FakeElement {
            name: "div",
            id: Some("other"),
            class: None,
            attributes: &[],
        }));
    }

    #[test]
    fn test_class_attribute_selector_presence() {
        // [class] should match elements that have a class
        let selector = ElementPredicate {
            name: None,
            id: None,
            classes: ClassSelections::from_static(&[]),
            attributes: AttributeSelections::from(vec![AttributeSelection {
                name: "class",
                value: None,
                kind: AttributeSelectionKind::Presence,
            }]),
        };
        assert!(selector.matches_element(&FakeElement {
            name: "div",
            id: None,
            class: Some("foo bar"),
            attributes: &[],
        }));
        assert!(!selector.matches_element(&FakeElement {
            name: "div",
            id: None,
            class: None,
            attributes: &[],
        }));
    }

    #[test]
    fn test_class_attribute_selector_contains() {
        // [class~="foo"] should match element with class containing "foo"
        let selector = ElementPredicate {
            name: None,
            id: None,
            classes: ClassSelections::from_static(&[]),
            attributes: AttributeSelections::from(vec![AttributeSelection {
                name: "class",
                value: Some("foo"),
                kind: AttributeSelectionKind::WhitespaceSeparated,
            }]),
        };
        assert!(selector.matches_element(&FakeElement {
            name: "div",
            id: None,
            class: Some("foo bar"),
            attributes: &[],
        }));
        assert!(!selector.matches_element(&FakeElement {
            name: "div",
            id: None,
            class: Some("baz qux"),
            attributes: &[],
        }));
    }
}
