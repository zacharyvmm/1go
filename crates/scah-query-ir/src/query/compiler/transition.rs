use crate::Reader;
use crate::query::compiler::SelectorParseError;
use crate::query::selector::{Combinator, ElementPredicate, IElement, Lexer};

#[derive(PartialEq, Debug, Clone)]
pub struct Transition<'query> {
    pub guard: Combinator,
    pub predicate: ElementPredicate<'query>,
}

impl<'query> Transition<'query> {
    pub fn new(guard: Combinator, predicate: ElementPredicate<'query>) -> Self {
        Self { guard, predicate }
    }

    pub const fn new_const(guard: Combinator, predicate: ElementPredicate<'query>) -> Self {
        Self { guard, predicate }
    }

    pub fn generate_transitions_from_string(
        query: &'query str,
    ) -> Result<Vec<Self>, SelectorParseError> {
        let reader = &mut Reader::new(query);
        let mut states = Vec::new();
        let mut seen_selector = false;
        while let Some((combinator, element)) = Lexer::try_next(reader, seen_selector)? {
            states.push(Self::new(combinator, element));
            seen_selector = true;
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

#[cfg(test)]
mod tests {
    use crate::query::selector::{Attribute, AttributeSelections, ClassSelections, IElement};

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

    #[test]
    fn test_fsm_next_sibling_same_depth() {
        let adjacent = Transition::new(
            Combinator::NextSibling,
            ElementPredicate {
                name: Some("p"),
                id: None,
                classes: ClassSelections::from_static(&[]),
                attributes: AttributeSelections::from_static(&[]),
            },
        );
        let subsequent = Transition::new(
            Combinator::SubsequentSibling,
            ElementPredicate {
                name: Some("p"),
                id: None,
                classes: ClassSelections::from_static(&[]),
                attributes: AttributeSelections::from_static(&[]),
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
