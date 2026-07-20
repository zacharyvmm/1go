//! Owned query builder and compiled query with selector tape ownership.
//!
//! Foreign callers supply temporary string pointers; this type owns them until
//! `build`, then packs selectors into a stable tape backing the compiled query.

use scah::{
    Query, QueryBuilder, QuerySection, QuerySectionId, Save, SelectionKind, SelectorParseError,
    Transition, TransitionId,
};
use std::mem::ManuallyDrop;
use std::sync::Arc;

#[derive(Debug, PartialEq, Clone)]
struct PendingSection {
    selector: String,
    save: Save,
    kind: SelectionKind,
    parent: Option<QuerySectionId>,
    next_sibling: Option<QuerySectionId>,
}

/// Pending query tree that owns selector strings until [`OwnedQueryBuilder::build`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OwnedQueryBuilder {
    sections: Vec<PendingSection>,
}

/// Compiled query whose selector string slices borrow [`OwnedQuery::selector_tape`].
///
/// # Safety invariant
///
/// - `query.queries[*].source` and transition predicates borrow bytes inside
///   `selector_tape`.
/// - `selector_tape` is an `Arc<[u8]>` so its allocation never moves after
///   construction (unlike a growable `Vec`).
/// - `query` must never outlive `selector_tape`.
/// - [`Drop`] drops `query` first, then `selector_tape`, preserving that order.
pub(crate) struct OwnedQuery {
    query: ManuallyDrop<Query<'static>>,
    /// Selector backing storage; must outlive `query`.
    #[allow(dead_code)]
    selector_tape: Arc<[u8]>,
}

impl Drop for OwnedQuery {
    fn drop(&mut self) {
        // SAFETY: drop the query (and its borrows into the tape) before the tape.
        unsafe {
            ManuallyDrop::drop(&mut self.query);
        }
    }
}

impl OwnedQuery {
    #[inline]
    pub(crate) fn query(&self) -> &Query<'static> {
        &self.query
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn selector_tape(&self) -> &Arc<[u8]> {
        &self.selector_tape
    }
}

impl OwnedQueryBuilder {
    pub(crate) fn new_all(selector: String, save: Save) -> Self {
        Self {
            sections: vec![PendingSection {
                selector,
                save,
                kind: SelectionKind::All,
                parent: None,
                next_sibling: None,
            }],
        }
    }

    pub(crate) fn new_first(selector: String, save: Save) -> Self {
        Self {
            sections: vec![PendingSection {
                selector,
                save,
                kind: SelectionKind::First,
                parent: None,
                next_sibling: None,
            }],
        }
    }

    pub(crate) fn all_mut(&mut self, selector: String, save: Save) {
        let parent_index = QuerySectionId(self.sections.len() - 1);
        self.sections.push(PendingSection {
            selector,
            save,
            kind: SelectionKind::All,
            parent: Some(parent_index),
            next_sibling: None,
        });
    }

    pub(crate) fn first_mut(&mut self, selector: String, save: Save) {
        let parent_index = QuerySectionId(self.sections.len() - 1);
        self.sections.push(PendingSection {
            selector,
            save,
            kind: SelectionKind::First,
            parent: Some(parent_index),
            next_sibling: None,
        });
    }

    #[cfg(test)]
    pub(crate) fn all(mut self, selector: String, save: Save) -> Self {
        self.all_mut(selector, save);
        self
    }

    #[cfg(test)]
    pub(crate) fn first(mut self, selector: String, save: Save) -> Self {
        self.first_mut(selector, save);
        self
    }

    /// Append a cloned child tree under `parent`.
    ///
    /// Returns `Err(())` when `parent` is not a valid section id.
    pub(crate) fn append(
        &mut self,
        parent: QuerySectionId,
        other: &OwnedQueryBuilder,
    ) -> Result<(), ()> {
        if parent.index() >= self.sections.len() {
            return Err(());
        }

        let mut other = other.clone();
        let selection_length = self.sections.len();

        let mut last_sibling: Option<QuerySectionId> = {
            if parent.index() + 1 == selection_length {
                None
            } else {
                let mut sibling_index = QuerySectionId(parent.index() + 1);
                while self.sections[sibling_index.index()].next_sibling.is_some() {
                    sibling_index = self.sections[sibling_index.index()].next_sibling.unwrap();
                }
                Some(sibling_index)
            }
        };

        for index in 0..other.sections.len() {
            let query = &mut other.sections[index];
            if let Some(next_sibling) = query.next_sibling {
                query.next_sibling = Some(QuerySectionId(next_sibling.index() + selection_length));
            }

            if let Some(idx) = query.parent {
                query.parent = Some(QuerySectionId(idx.index() + selection_length));
            } else {
                query.parent = Some(parent);

                let current_index = QuerySectionId(selection_length + index);
                last_sibling = match last_sibling {
                    Some(sibling) => {
                        if sibling.index() < selection_length {
                            self.sections[sibling.index()].next_sibling = Some(current_index);
                        } else {
                            other.sections[sibling.index() - selection_length].next_sibling =
                                Some(current_index);
                        }
                        Some(current_index)
                    }
                    None => Some(current_index),
                };
            }
        }
        self.sections.append(&mut other.sections);
        Ok(())
    }

    pub(crate) fn current_section(&self) -> Option<QuerySectionId> {
        if self.sections.is_empty() {
            None
        } else {
            Some(QuerySectionId(self.sections.len() - 1))
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.sections.len()
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// Compile pending sections into an [`OwnedQuery`].
    ///
    /// Invalid selectors are reported here (not at construction time).
    pub(crate) fn build(&self) -> Result<OwnedQuery, SelectorParseError> {
        self.clone().build_owned()
    }

    fn build_owned(self) -> Result<OwnedQuery, SelectorParseError> {
        let string_tape_size: usize = self.sections.iter().map(|q| q.selector.len()).sum();
        let mut string_tape = Vec::with_capacity(string_tape_size);
        for section in &self.sections {
            string_tape.extend_from_slice(section.selector.as_bytes());
        }

        // Freeze the tape *before* creating any string slices so borrows cannot
        // be invalidated by `Vec` reallocation (e.g. `into_boxed_slice` shrink).
        let selector_tape: Arc<[u8]> = Arc::from(string_tape);

        let mut queries = Vec::with_capacity(self.sections.len());
        let mut states = Vec::with_capacity(self.sections.len() * 2);
        let mut offset = 0usize;

        for section in self.sections {
            let len = section.selector.len();
            // SAFETY: bytes were copied from valid UTF-8 `String`s. The slice
            // points into `selector_tape`, which is retained in `OwnedQuery`
            // and outlives the query (enforced by Drop order).
            let source = unsafe {
                let raw_slice = std::slice::from_raw_parts(selector_tape.as_ptr().add(offset), len);
                str::from_utf8_unchecked(raw_slice)
            };
            offset += len;

            let mut string_states = Transition::generate_transitions_from_string(source)?;
            let range = {
                let start = states.len();
                states.append(&mut string_states);
                let end = states.len();
                TransitionId(start)..TransitionId(end)
            };

            queries.push(QuerySection {
                source,
                range,
                save: section.save,
                kind: section.kind,
                parent: section.parent,
                next_sibling: section.next_sibling,
            });
        }

        let query = QueryBuilder {
            states,
            selection: queries,
        }
        .build();

        // SAFETY: `query` borrows only from `selector_tape`. We store both in
        // `OwnedQuery` and drop `query` before the tape, so extending the
        // lifetime to `'static` is sound for as long as `OwnedQuery` exists.
        let query: Query<'static> = unsafe { std::mem::transmute(query) };

        Ok(OwnedQuery {
            query: ManuallyDrop::new(query),
            selector_tape,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use scah::{
        AttributeSelections, ClassSelections, Combinator, ElementPredicate, Query, QuerySectionId,
        Save, SelectionKind, TransitionId,
    };

    #[test]
    fn test_owned_query_selector() {
        let q = OwnedQueryBuilder::new_all("div".into(), Save::all())
            .first("a".into(), Save::all())
            .all("a".into(), Save::none());

        assert_eq!(
            q.sections,
            vec![
                PendingSection {
                    selector: "div".into(),
                    save: Save::all(),
                    kind: SelectionKind::All,
                    parent: None,
                    next_sibling: None,
                },
                PendingSection {
                    selector: "a".into(),
                    save: Save::all(),
                    kind: SelectionKind::First,
                    parent: Some(QuerySectionId(0)),
                    next_sibling: None,
                },
                PendingSection {
                    selector: "a".into(),
                    save: Save::none(),
                    kind: SelectionKind::All,
                    parent: Some(QuerySectionId(1)),
                    next_sibling: None,
                }
            ]
        );
    }

    #[test]
    fn test_owned_query_branches() {
        let mut root = OwnedQueryBuilder::new_all("div".into(), Save::all())
            .first("a".into(), Save::all())
            .all("a".into(), Save::none());
        let parent = root.current_section().unwrap();

        let branch_a = OwnedQueryBuilder::new_all("span".into(), Save::all());
        let branch_b = OwnedQueryBuilder::new_first("section".into(), Save::all())
            .all("figure".into(), Save::none());

        root.append(parent, &branch_a).unwrap();
        root.append(parent, &branch_b).unwrap();

        // Child builders remain usable after append.
        assert_eq!(branch_a.len(), 1);
        assert_eq!(branch_b.len(), 2);

        assert_eq!(
            root.sections,
            vec![
                PendingSection {
                    selector: "div".into(),
                    save: Save::all(),
                    kind: SelectionKind::All,
                    parent: None,
                    next_sibling: None,
                },
                PendingSection {
                    selector: "a".into(),
                    save: Save::all(),
                    kind: SelectionKind::First,
                    parent: Some(QuerySectionId(0)),
                    next_sibling: None,
                },
                PendingSection {
                    selector: "a".into(),
                    save: Save::none(),
                    kind: SelectionKind::All,
                    parent: Some(QuerySectionId(1)),
                    next_sibling: None,
                },
                PendingSection {
                    selector: "span".into(),
                    save: Save::all(),
                    kind: SelectionKind::All,
                    parent: Some(QuerySectionId(2)),
                    next_sibling: Some(QuerySectionId(4)),
                },
                PendingSection {
                    selector: "section".into(),
                    save: Save::all(),
                    kind: SelectionKind::First,
                    parent: Some(QuerySectionId(2)),
                    next_sibling: None,
                },
                PendingSection {
                    selector: "figure".into(),
                    save: Save::none(),
                    kind: SelectionKind::All,
                    parent: Some(QuerySectionId(4)),
                    next_sibling: None,
                },
            ]
        );
    }

    #[test]
    fn invalid_parent_is_rejected() {
        let mut root = OwnedQueryBuilder::new_all("div".into(), Save::all());
        let child = OwnedQueryBuilder::new_all("span".into(), Save::all());
        assert!(root.append(QuerySectionId(5), &child).is_err());
        assert_eq!(root.len(), 1);
    }

    #[test]
    fn invalid_selector_reported_at_build() {
        let q = OwnedQueryBuilder::new_all("".into(), Save::all());
        assert!(q.build().is_err());
    }

    #[test]
    fn build_does_not_consume_builder() {
        let builder = OwnedQueryBuilder::new_all("div".into(), Save::all());
        let owned = builder.build().unwrap();
        assert_eq!(builder.len(), 1);
        assert_eq!(owned.query().queries[0].source, "div");
    }

    #[test]
    fn cloned_builders_mutate_independently() {
        let mut a = OwnedQueryBuilder::new_all("div".into(), Save::all());
        let mut b = a.clone();
        a.all_mut("a".into(), Save::all());
        b.first_mut("span".into(), Save::none());
        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 2);
        assert_eq!(a.sections[1].selector, "a");
        assert_eq!(b.sections[1].selector, "span");
    }

    #[test]
    fn selector_dropped_before_query_use() {
        let selector = String::from("div");
        let builder = OwnedQueryBuilder::new_all(selector, Save::all());
        // selector moved into builder; building owns a tape copy.
        let owned = builder.build().unwrap();
        assert_eq!(owned.query().queries[0].source, "div");
        assert_eq!(owned.selector_tape().as_ref(), b"div" as &[u8]);
    }

    #[test]
    fn test_tape_slicing() {
        let q = OwnedQueryBuilder::new_all(String::from("div"), Save::all())
            .first(String::from("a"), Save::all())
            .all(String::from("a"), Save::none());

        let owned = q.build().unwrap();
        assert_eq!(owned.selector_tape().as_ref(), b"divaa" as &[u8]);

        let range = owned.selector_tape().as_ptr_range();
        assert!(range.contains(&owned.query().queries[0].source.as_ptr()));
        assert!(range.contains(&owned.query().queries[1].source.as_ptr()));
        assert!(range.contains(&owned.query().queries[2].source.as_ptr()));

        assert_eq!(
            owned.query(),
            &Query {
                states: vec![
                    Transition::new(
                        Combinator::Descendant,
                        ElementPredicate {
                            name: Some("div"),
                            id: None,
                            classes: ClassSelections::from_static(&[]),
                            attributes: AttributeSelections::from_static(&[])
                        }
                    ),
                    Transition::new(
                        Combinator::Descendant,
                        ElementPredicate {
                            name: Some("a"),
                            id: None,
                            classes: ClassSelections::from_static(&[]),
                            attributes: AttributeSelections::from_static(&[])
                        }
                    ),
                    Transition::new(
                        Combinator::Descendant,
                        ElementPredicate {
                            name: Some("a"),
                            id: None,
                            classes: ClassSelections::from_static(&[]),
                            attributes: AttributeSelections::from_static(&[])
                        }
                    ),
                ]
                .into_boxed_slice(),
                queries: vec![
                    QuerySection::new(
                        "div",
                        Save::all(),
                        SelectionKind::All,
                        TransitionId(0)..TransitionId(1),
                        None,
                    ),
                    QuerySection::new(
                        "a",
                        Save::all(),
                        SelectionKind::First,
                        TransitionId(1)..TransitionId(2),
                        Some(QuerySectionId(0)),
                    ),
                    QuerySection::new(
                        "a",
                        Save::none(),
                        SelectionKind::All,
                        TransitionId(2)..TransitionId(3),
                        Some(QuerySectionId(1)),
                    ),
                ]
                .into_boxed_slice(),
                exit_at_section_end: None,
            }
        );
    }

    #[test]
    fn test_early_exit() {
        let owned = OwnedQueryBuilder::new_all("a".into(), Save::all())
            .build()
            .unwrap();
        assert_eq!(owned.query().exit_at_section_end, None);

        let owned = OwnedQueryBuilder::new_first("a".into(), Save::all())
            .build()
            .unwrap();
        assert_eq!(owned.query().exit_at_section_end, Some(QuerySectionId(0)));

        let owned = OwnedQueryBuilder::new_first("p".into(), Save::none())
            .first("a".into(), Save::none())
            .build()
            .unwrap();
        assert_eq!(owned.query().exit_at_section_end, Some(QuerySectionId(1)));
    }

    #[test]
    fn sequential_appends_to_same_parent() {
        let mut root = OwnedQueryBuilder::new_all("main".into(), Save::all());
        let parent = root.current_section().unwrap();
        root.append(parent, &OwnedQueryBuilder::new_all("a".into(), Save::all()))
            .unwrap();
        root.append(parent, &OwnedQueryBuilder::new_all("b".into(), Save::all()))
            .unwrap();
        root.append(parent, &OwnedQueryBuilder::new_all("c".into(), Save::all()))
            .unwrap();

        assert_eq!(root.sections[1].next_sibling, Some(QuerySectionId(2)));
        assert_eq!(root.sections[2].next_sibling, Some(QuerySectionId(3)));
        assert_eq!(root.sections[3].next_sibling, None);
        assert_eq!(root.sections[1].parent, Some(QuerySectionId(0)));
        assert_eq!(root.sections[2].parent, Some(QuerySectionId(0)));
        assert_eq!(root.sections[3].parent, Some(QuerySectionId(0)));
    }
}
