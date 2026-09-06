use crate::__private::PredicateMetadata;
use crate::ElementPredicate;
use smallvec::SmallVec;

const INLINE_ATTRIBUTE_KEYS: usize = 4;

/// Attribute fields required by the currently active query frontier.
///
/// `all` is used for viable save points because Scah's current result contract
/// preserves every attribute. Intermediate selector transitions can request
/// only the fields they actually inspect.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct AttributeInterest<'query> {
    all: bool,
    id: bool,
    class: bool,
    keys: SmallVec<[&'query str; 4]>,
}

impl<'query> AttributeInterest<'query> {
    #[inline]
    pub fn clear(&mut self) {
        self.all = false;
        self.id = false;
        self.class = false;
        self.keys.clear();
    }

    #[inline]
    pub fn require_all(&mut self) {
        self.all = true;
        self.keys.clear();
    }

    pub fn add_predicate(&mut self, predicate: &ElementPredicate<'query>) {
        self.add_metadata(&PredicateMetadata::compile(predicate));
    }

    pub fn add_metadata(&mut self, metadata: &PredicateMetadata<'query>) {
        if self.all {
            return;
        }

        self.id |= metadata.needs_id();
        self.class |= metadata.needs_class();
        for &key in metadata.attribute_names() {
            if !self
                .keys
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(key))
            {
                if self.keys.len() == INLINE_ATTRIBUTE_KEYS {
                    self.require_all();
                    return;
                }
                self.keys.push(key);
            }
        }
    }

    pub fn merge(&mut self, other: &Self) {
        if self.all || other.is_empty() {
            return;
        }
        if other.all {
            self.require_all();
            return;
        }

        self.id |= other.id;
        self.class |= other.class;
        for &key in &other.keys {
            if !self
                .keys
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(key))
            {
                if self.keys.len() == INLINE_ATTRIBUTE_KEYS {
                    self.require_all();
                    return;
                }
                self.keys.push(key);
            }
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        !self.all && !self.id && !self.class && self.keys.is_empty()
    }

    #[inline]
    pub fn includes_id(&self) -> bool {
        self.all || self.id
    }

    #[inline]
    pub fn includes_class(&self) -> bool {
        self.all || self.class
    }

    #[inline]
    pub fn includes_attribute(&self, key: &str) -> bool {
        self.all
            || self
                .keys
                .iter()
                .any(|required| required.eq_ignore_ascii_case(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AttributeSelection, AttributeSelectionKind, AttributeSelections, ClassSelections};

    #[test]
    fn merges_dedicated_and_generic_attribute_requirements() {
        let mut interest = AttributeInterest::default();
        interest.add_predicate(&ElementPredicate {
            name: Some("a"),
            id: Some("hero"),
            classes: ClassSelections::from_static(&["promoted"]),
            attributes: AttributeSelections::from(vec![
                AttributeSelection {
                    name: "href",
                    value: None,
                    kind: AttributeSelectionKind::Presence,
                    case_sensitivity: crate::AttributeCaseSensitivity::Default,
                },
                AttributeSelection {
                    name: "HREF",
                    value: None,
                    kind: AttributeSelectionKind::Presence,
                    case_sensitivity: crate::AttributeCaseSensitivity::Default,
                },
            ]),
            logical: Default::default(),
            structural: Default::default(),
        });

        assert!(interest.includes_id());
        assert!(interest.includes_class());
        assert!(interest.includes_attribute("href"));
        assert!(!interest.includes_attribute("rel"));
        assert_eq!(interest.keys.len(), 1);
    }

    #[test]
    fn all_interest_supersedes_selected_keys() {
        let mut interest = AttributeInterest::default();
        interest.add_predicate(&ElementPredicate {
            name: None,
            id: None,
            classes: ClassSelections::default(),
            attributes: AttributeSelections::from(vec![AttributeSelection {
                name: "href",
                value: None,
                kind: AttributeSelectionKind::Presence,
                case_sensitivity: crate::AttributeCaseSensitivity::Default,
            }]),
            logical: Default::default(),
            structural: Default::default(),
        });
        interest.require_all();

        assert!(interest.includes_id());
        assert!(interest.includes_class());
        assert!(interest.includes_attribute("anything"));
        assert!(interest.keys.is_empty());
    }

    #[test]
    fn excess_selected_keys_fall_back_without_spilling() {
        let mut interest = AttributeInterest::default();
        let predicate = ElementPredicate {
            name: Some("a"),
            id: None,
            classes: ClassSelections::default(),
            attributes: AttributeSelections::from(
                ["href", "target", "rel", "download", "data-id"]
                    .into_iter()
                    .map(|name| AttributeSelection {
                        name,
                        value: None,
                        kind: AttributeSelectionKind::Presence,
                        case_sensitivity: crate::AttributeCaseSensitivity::Default,
                    })
                    .collect::<Vec<_>>(),
            ),
            logical: Default::default(),
            structural: Default::default(),
        };
        let metadata = PredicateMetadata::compile(&predicate);
        interest.add_metadata(&metadata);

        assert!(interest.all);
        assert!(interest.keys.is_empty());
        assert!(!interest.keys.spilled());
    }

    #[test]
    fn merge_deduplicates_compiled_interest() {
        let mut left = AttributeInterest::default();
        left.add_predicate(&ElementPredicate {
            name: None,
            id: Some("hero"),
            classes: ClassSelections::default(),
            attributes: AttributeSelections::from(vec![AttributeSelection {
                name: "href",
                value: None,
                kind: AttributeSelectionKind::Presence,
                case_sensitivity: crate::AttributeCaseSensitivity::Default,
            }]),
            logical: Default::default(),
            structural: Default::default(),
        });
        let mut right = AttributeInterest::default();
        right.add_predicate(&ElementPredicate {
            name: None,
            id: None,
            classes: ClassSelections::from_static(&["promoted"]),
            attributes: AttributeSelections::from(vec![AttributeSelection {
                name: "HREF",
                value: None,
                kind: AttributeSelectionKind::Presence,
                case_sensitivity: crate::AttributeCaseSensitivity::Default,
            }]),
            logical: Default::default(),
            structural: Default::default(),
        });

        left.merge(&right);

        assert!(left.includes_id());
        assert!(left.includes_class());
        assert_eq!(left.keys.as_slice(), &["href"]);
    }
}
