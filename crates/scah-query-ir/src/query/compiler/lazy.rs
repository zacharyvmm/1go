use super::transition::Transition;
use crate::{Query, QueryBuilder, QuerySection, QuerySectionId, TransitionId};

use super::SelectorParseError;
use super::builder::{Save, SelectionKind};

#[derive(Debug, PartialEq, Clone)]
struct QueryString<S: AsRef<str>> {
    source: S,
    save: Save,
    kind: SelectionKind,

    parent: Option<QuerySectionId>,
    next_sibling: Option<QuerySectionId>,
}

pub struct LazyQuery {}

impl LazyQuery {
    pub fn all<S: AsRef<str>>(query: S, save: Save) -> LazyQueryBuilder<S> {
        LazyQueryBuilder {
            queries: vec![QueryString {
                source: query,
                save: save,
                kind: SelectionKind::All,

                parent: None,
                next_sibling: None,
            }],
        }
    }

    pub fn first<S: AsRef<str>>(query: S, save: Save) -> LazyQueryBuilder<S> {
        LazyQueryBuilder {
            queries: vec![QueryString {
                source: query,
                save: save,
                kind: SelectionKind::First,

                parent: None,
                next_sibling: None,
            }],
        }
    }
}

// NOTE: The disadvantages of this are worst errors for invalid queries,
//  since build is at the end, thus this probably only be used for Bindings
// Basicly it's used for owned Strings, but I'll make it work for `&str`
#[derive(Clone)]
pub struct LazyQueryBuilder<S: AsRef<str>> {
    queries: Vec<QueryString<S>>,
}

impl<S: AsRef<str>> LazyQueryBuilder<S> {
    pub fn all_mut(&mut self, query: S, save: Save) {
        let parent_index = QuerySectionId(self.queries.len() - 1);
        self.queries.push(QueryString {
            source: query,
            save: save,
            kind: SelectionKind::All,
            parent: Some(parent_index),
            next_sibling: None,
        });
    }
    pub fn first_mut(&mut self, query: S, save: Save) {
        let parent_index = QuerySectionId(self.queries.len() - 1);
        self.queries.push(QueryString {
            source: query,
            save: save,
            kind: SelectionKind::First,
            parent: Some(parent_index),
            next_sibling: None,
        });
    }

    pub fn all(mut self, query: S, save: Save) -> Self {
        self.all_mut(query, save);
        self
    }

    pub fn first(mut self, query: S, save: Save) -> Self {
        self.first_mut(query, save);
        self
    }

    pub fn append(&mut self, parent: QuerySectionId, mut other: Self) {
        let selection_length = self.queries.len();

        let mut last_sibling: Option<QuerySectionId> = {
            if parent.index() + 1 == selection_length {
                None
            } else {
                let mut sibling_index = QuerySectionId(parent.index() + 1);
                while self.queries[sibling_index.index()].next_sibling.is_some() {
                    sibling_index = self.queries[sibling_index.index()].next_sibling.unwrap();
                }

                Some(sibling_index)
            }
        };
        for index in 0..other.queries.len() {
            let query = &mut other.queries[index];
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
                            self.queries[sibling.index()].next_sibling = Some(current_index);
                        } else {
                            other.queries[sibling.index() - selection_length].next_sibling =
                                Some(current_index);
                        }
                        Some(current_index)
                    }
                    None => Some(current_index),
                };
            }
        }
        self.queries.append(&mut other.queries);
    }

    pub fn then_mut<F, I>(&mut self, func: F)
    where
        F: FnOnce(LazyQueryFactory) -> I,
        I: IntoIterator<Item = Self>,
    {
        let factory = LazyQueryFactory {};
        let children = func(factory);

        let current_index = QuerySectionId(self.queries.len() - 1);
        for child in children {
            self.append(current_index, child);
        }
    }

    pub fn then<F, I>(mut self, func: F) -> Self
    where
        F: FnOnce(LazyQueryFactory) -> I,
        I: IntoIterator<Item = Self>,
    {
        self.then_mut(func);
        self
    }

    pub fn len(&self) -> usize {
        self.queries.len()
    }

    /// # Safety
    /// This is for an internal abstraction for bidings.
    pub unsafe fn try_to_query<'a>(
        self,
    ) -> Result<(std::sync::Arc<Vec<u8>>, Query<'a>), SelectorParseError> {
        // I need to do this to unsafely get a slice from the String
        let string_tape_size = self.queries.iter().map(|q| q.source.as_ref().len()).sum();
        let mut string_tape = Vec::with_capacity(string_tape_size);

        let mut queries = Vec::with_capacity(self.queries.len());
        let mut states = Vec::with_capacity(self.queries.len() * 2);
        let mut alternatives = Vec::with_capacity(self.queries.len());

        for query in self.queries {
            let source = {
                let start = string_tape.len();
                string_tape.extend_from_slice(query.source.as_ref().as_bytes());
                let end = string_tape.len();

                unsafe {
                    let pointer = string_tape.as_ptr();

                    let raw_slice: &[u8] =
                        std::slice::from_raw_parts(pointer.add(start), end - start);
                    str::from_utf8_unchecked(raw_slice)
                }
            };

            let paths = if query.parent.is_some() {
                Transition::generate_scoped_transition_paths_from_string(source)?
            } else {
                Transition::generate_transition_paths_from_string(source)?
            };
            let mut section_alternatives = Vec::new();
            for mut path in paths {
                let start = states.len();
                states.append(&mut path);
                section_alternatives.push(TransitionId(start)..TransitionId(states.len()));
            }
            let range = section_alternatives.first().unwrap().start
                ..section_alternatives.last().unwrap().end;
            alternatives.push(section_alternatives);

            queries.push(QuerySection {
                source,
                range,

                save: query.save,
                kind: query.kind,

                parent: query.parent,
                next_sibling: query.next_sibling,
            });
        }

        Ok((
            std::sync::Arc::new(string_tape),
            QueryBuilder {
                states,
                selection: queries,
                alternatives,
            }
            .build(),
        ))
    }

    /// # Safety
    /// This is for an internal abstraction for bidings.
    pub unsafe fn to_query<'a>(self) -> (std::sync::Arc<Vec<u8>>, Query<'a>) {
        unsafe { self.try_to_query() }.unwrap_or_else(|err| panic!("invalid query selector: {err}"))
    }
}

pub struct LazyQueryFactory {}
impl LazyQueryFactory {
    pub fn all<S: AsRef<str>>(&self, query: S, save: Save) -> LazyQueryBuilder<S> {
        LazyQuery::all(query, save)
    }

    pub fn first<S: AsRef<str>>(&self, query: S, save: Save) -> LazyQueryBuilder<S> {
        LazyQuery::first(query, save)
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::Transition;
    use super::*;
    use crate::AttributeSelections;
    use crate::query::selector::{ClassSelections, Combinator, ElementPredicate};
    use crate::{Query, QuerySectionId, Save, SelectionKind, TransitionId};

    #[test]
    fn test_lazy_query_selector() {
        let q = LazyQuery::all("div", Save::all())
            .first("a", Save::all())
            .all("a", Save::none());

        assert_eq!(
            q.queries,
            vec![
                QueryString {
                    source: "div",
                    save: Save::all(),
                    kind: SelectionKind::All,
                    parent: None,
                    next_sibling: None,
                },
                QueryString {
                    source: "a",
                    save: Save::all(),
                    kind: SelectionKind::First,
                    parent: Some(QuerySectionId(0)),
                    next_sibling: None,
                },
                QueryString {
                    source: "a",
                    save: Save::none(),
                    kind: SelectionKind::All,
                    parent: Some(QuerySectionId(1)),
                    next_sibling: None,
                }
            ]
        );
    }

    #[test]
    fn test_lazy_query_selector_string() {
        let q = LazyQuery::all(String::from("div"), Save::all())
            .first(String::from("a"), Save::all())
            .all(String::from("a"), Save::none());

        assert_eq!(
            q.queries,
            vec![
                QueryString {
                    source: String::from("div"),
                    save: Save::all(),
                    kind: SelectionKind::All,
                    parent: None,
                    next_sibling: None,
                },
                QueryString {
                    source: String::from("a"),
                    save: Save::all(),
                    kind: SelectionKind::First,
                    parent: Some(QuerySectionId(0)),
                    next_sibling: None,
                },
                QueryString {
                    source: String::from("a"),
                    save: Save::none(),
                    kind: SelectionKind::All,
                    parent: Some(QuerySectionId(1)),
                    next_sibling: None,
                }
            ]
        );
    }

    #[test]
    fn test_lazy_query_selector_then() {
        let q = LazyQuery::all("div", Save::all())
            .first("a", Save::all())
            .all("a", Save::none())
            .then(|a| {
                [
                    a.all("span", Save::all()),
                    a.first("section", Save::all()).all("figure", Save::none()),
                ]
            });

        assert_eq!(
            q.queries,
            vec![
                QueryString {
                    source: "div",
                    save: Save::all(),
                    kind: SelectionKind::All,
                    parent: None,
                    next_sibling: None,
                },
                QueryString {
                    source: "a",
                    save: Save::all(),
                    kind: SelectionKind::First,
                    parent: Some(QuerySectionId(0)),
                    next_sibling: None,
                },
                QueryString {
                    source: "a",
                    save: Save::none(),
                    kind: SelectionKind::All,
                    parent: Some(QuerySectionId(1)),
                    next_sibling: None,
                },
                QueryString {
                    source: "span",
                    save: Save::all(),
                    kind: SelectionKind::All,
                    parent: Some(QuerySectionId(2)),
                    next_sibling: Some(QuerySectionId(4)),
                },
                QueryString {
                    source: "section",
                    save: Save::all(),
                    kind: SelectionKind::First,
                    parent: Some(QuerySectionId(2)),
                    next_sibling: None,
                },
                QueryString {
                    source: "figure",
                    save: Save::none(),
                    kind: SelectionKind::All,
                    parent: Some(QuerySectionId(4)),
                    next_sibling: None,
                },
            ]
        );
    }

    #[test]
    fn test_unsafe_slicing() {
        let q = LazyQuery::all(String::from("div"), Save::all())
            .first(String::from("a"), Save::all())
            .all(String::from("a"), Save::none());

        let (tape, query) = unsafe { q.to_query() };

        assert_eq!(*tape, b"divaa");
        // std::mem::drop(tape);

        let range = tape.as_ptr_range();
        assert!(range.contains(&query.queries[0].source.as_ptr()));
        assert!(range.contains(&query.queries[1].source.as_ptr()));
        assert!(range.contains(&query.queries[2].source.as_ptr()));

        assert_eq!(
            query,
            Query {
                states: vec![
                    Transition::new(
                        Combinator::Descendant,
                        ElementPredicate {
                            name: Some("div"),
                            id: None,
                            classes: ClassSelections::from_static(&[]),
                            attributes: AttributeSelections::from_static(&[]),
                            logical: crate::LogicalPredicates::from_static(&[]),
                            structural: crate::StructuralPredicates::from_static(&[]),
                        }
                    ),
                    Transition::new(
                        Combinator::Descendant,
                        ElementPredicate {
                            name: Some("a"),
                            id: None,
                            classes: ClassSelections::from_static(&[]),
                            attributes: AttributeSelections::from_static(&[]),
                            logical: crate::LogicalPredicates::from_static(&[]),
                            structural: crate::StructuralPredicates::from_static(&[]),
                        }
                    ),
                    Transition::new(
                        Combinator::Descendant,
                        ElementPredicate {
                            name: Some("a"),
                            id: None,
                            classes: ClassSelections::from_static(&[]),
                            attributes: AttributeSelections::from_static(&[]),
                            logical: crate::LogicalPredicates::from_static(&[]),
                            structural: crate::StructuralPredicates::from_static(&[]),
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
                alternatives: vec![
                    vec![TransitionId(0)..TransitionId(1)].into_boxed_slice(),
                    vec![TransitionId(1)..TransitionId(2)].into_boxed_slice(),
                    vec![TransitionId(2)..TransitionId(3)].into_boxed_slice(),
                ]
                .into_boxed_slice(),
            }
        );
    }

    #[test]
    fn lazy_query_accepts_selector_lists_with_engine_support() {
        let result = unsafe { LazyQuery::all("div, span", Save::none()).try_to_query() };

        assert!(result.is_ok());
    }

    #[test]
    fn lazy_query_accepts_structural_selectors_with_engine_support() {
        let result = unsafe { LazyQuery::all("li:nth-child(2)", Save::none()).try_to_query() };

        assert!(result.is_ok());
    }

    #[test]
    fn test_early_exit() {
        let (_, query) = unsafe { LazyQuery::all("a", Save::all()).to_query() };
        assert_eq!(query.exit_at_section_end, None);

        let (_, query) = unsafe { LazyQuery::all("a", Save::none()).to_query() };
        assert_eq!(query.exit_at_section_end, None);

        let (_, query) = unsafe { LazyQuery::first("a", Save::all()).to_query() };
        assert_eq!(query.exit_at_section_end, Some(QuerySectionId(0)));

        let (_, query) = unsafe { LazyQuery::first("a", Save::none()).to_query() };
        assert_eq!(query.exit_at_section_end, Some(QuerySectionId(0)));

        let (_, query) = unsafe {
            LazyQuery::all("p", Save::all())
                .first("a", Save::all())
                .to_query()
        };
        assert_eq!(query.exit_at_section_end, None);

        let (_, query) = unsafe {
            LazyQuery::first("p", Save::all())
                .all("a", Save::all())
                .to_query()
        };
        assert_eq!(query.exit_at_section_end, Some(QuerySectionId(0)));

        let (_, query) = unsafe {
            LazyQuery::first("p", Save::all())
                .first("a", Save::all())
                .to_query()
        };
        assert_eq!(query.exit_at_section_end, Some(QuerySectionId(0)));

        let (_, query) = unsafe {
            LazyQuery::first("p", Save::none())
                .first("a", Save::none())
                .to_query()
        };
        assert_eq!(query.exit_at_section_end, Some(QuerySectionId(1)));
    }
}
