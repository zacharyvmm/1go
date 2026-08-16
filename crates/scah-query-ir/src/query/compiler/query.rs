use std::ops::Range;

use super::builder::{QueryBuilder, Save, SelectionKind};
use super::error::SelectorParseError;
use super::transition::Transition;
use crate::query::selector::Combinator;

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone, Copy)]
pub struct TransitionId(pub usize);

impl TransitionId {
    #[inline(always)]
    pub fn index(self) -> usize {
        self.0
    }
}

impl From<usize> for TransitionId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone, Copy)]
pub struct QuerySectionId(pub usize);

impl QuerySectionId {
    #[inline(always)]
    pub fn index(self) -> usize {
        self.0
    }
}

impl From<usize> for QuerySectionId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

struct PositionIterator<'query, Q: QuerySpec<'query>> {
    arena: &'query Q,
    current: Option<Position>,
}
impl<'query, Q: QuerySpec<'query>> Iterator for PositionIterator<'query, Q> {
    type Item = Position;
    fn next(&mut self) -> Option<Self::Item> {
        self.current
            .inspect(|position| self.current = position.next_sibling(self.arena))
    }
}

pub trait QuerySpec<'query> {
    fn states(&self) -> &[Transition<'query>];
    fn queries(&self) -> &[QuerySection<'query>];
    fn exit_at_section_end(&self) -> Option<QuerySectionId>;

    fn requires_text_content(&self) -> bool {
        self.queries()
            .iter()
            .any(|section| section.save.text_content)
    }

    fn requires_attribute_storage(&self) -> bool {
        self.queries().iter().any(|section| section.save.attributes)
    }

    fn get_transition(&self, state: TransitionId) -> &Transition<'query> {
        &self.states()[state.index()]
    }

    fn get_section_selection_kind(&self, section_index: QuerySectionId) -> SelectionKind {
        self.queries()[section_index.index()].kind
    }

    fn get_selection(&self, section_index: QuerySectionId) -> &QuerySection<'query> {
        &self.queries()[section_index.index()]
    }

    fn is_descendant(&self, state: TransitionId) -> bool {
        self.get_transition(state).guard == Combinator::Descendant
    }

    fn needs_descendant_anchor(&self, position: Position) -> bool {
        if !self.is_descendant(position.state) {
            return false;
        }

        if self.is_save_point(&position) {
            let kind = self.get_section_selection_kind(position.selection);
            // Each All match with child sections creates a distinct output scope.
            return matches!(kind, SelectionKind::All) && position.next_child(self).is_some();
        }

        let next = position
            .next_transition(self)
            .expect("non-save-point must have a next transition");

        // Descendant continuations already cover nested rematches.
        !self.is_descendant(next)
    }

    fn is_save_point(&self, position: &Position) -> bool {
        debug_assert!(
            self.get_selection(position.selection)
                .range
                .contains(&position.state)
        );
        self.get_selection(position.selection).range.end.index() - 1 == position.state.index()
    }

    fn is_last_save_point(&self, position: &Position) -> bool {
        debug_assert!(position.selection.index() < self.queries().len());
        let is_last_query = self.queries().len() - 1 == position.selection.index();
        let is_last_state =
            self.get_selection(position.selection).range.end.index() - 1 == position.state.index();
        is_last_query && is_last_state
    }

    fn children(&'query self, position: &Position) -> Option<impl Iterator<Item = Position>>
    where
        Self: Sized,
    {
        position.next_child(self).map(|child| PositionIterator {
            arena: self,
            current: Some(child),
        })
    }
}

#[derive(PartialEq, Debug, Clone, Copy)]
pub struct Position {
    pub selection: QuerySectionId,
    pub state: TransitionId,
}

impl Position {
    pub fn next_transition<'query, Q: QuerySpec<'query> + ?Sized>(
        &self,
        query: &Q,
    ) -> Option<TransitionId> {
        debug_assert!(self.selection.index() < query.queries().len());
        debug_assert!(
            query
                .get_selection(self.selection)
                .range
                .contains(&self.state)
        );

        let selection_range = &query.get_selection(self.selection).range;
        if self.state.index() + 1 < selection_range.end.index() {
            Some(TransitionId(self.state.index() + 1))
        } else {
            None
        }
    }

    pub fn next_child<'query, Q: QuerySpec<'query> + ?Sized>(&self, query: &Q) -> Option<Self> {
        debug_assert!(self.selection.index() < query.queries().len());
        debug_assert!(
            query
                .get_selection(self.selection)
                .range
                .contains(&self.state)
        );

        if self.selection.index() == query.queries().len() - 1 {
            return None;
        }

        let next_selection_index = QuerySectionId(self.selection.index() + 1);
        let next_selection = query.get_selection(next_selection_index);
        if next_selection.parent.is_some_and(|p| p == self.selection) {
            return Some(Self {
                selection: next_selection_index,
                state: next_selection.range.start,
            });
        }

        None
    }

    pub fn next_sibling<'query, Q: QuerySpec<'query> + ?Sized>(&self, query: &Q) -> Option<Self> {
        debug_assert!(self.selection.index() < query.queries().len());
        debug_assert!(
            query
                .get_selection(self.selection)
                .range
                .contains(&self.state)
        );

        query
            .get_selection(self.selection)
            .next_sibling
            .map(|sibling| Self {
                selection: sibling,
                state: query.get_selection(sibling).range.start,
            })
    }

    pub fn back<'query, Q: QuerySpec<'query> + ?Sized>(&mut self, query: &Q) {
        debug_assert!(self.selection.index() < query.queries().len());
        debug_assert!(self.state < query.get_selection(self.selection).range.end);

        let selection = query.get_selection(self.selection);
        if self.state.index() > selection.range.start.index() {
            self.state = TransitionId(self.state.index() - 1);
        } else if let Some(parent) = selection.parent {
            self.selection = parent;
            self.state = TransitionId(query.get_selection(self.selection).range.end.index() - 1);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuerySection<'query> {
    pub source: &'query str,
    pub range: Range<TransitionId>,
    pub parent: Option<QuerySectionId>,
    pub next_sibling: Option<QuerySectionId>,
    pub save: Save,
    pub kind: SelectionKind,
}

impl<'query> QuerySection<'query> {
    pub fn new(
        source: &'query str,
        save: Save,
        kind: SelectionKind,
        range: Range<TransitionId>,
        parent: Option<QuerySectionId>,
    ) -> Self {
        Self {
            source,
            save,
            kind,
            range,
            parent,
            next_sibling: None,
        }
    }

    pub const fn new_const(
        source: &'query str,
        save: Save,
        kind: SelectionKind,
        range: Range<TransitionId>,
        parent: Option<QuerySectionId>,
        next_sibling: Option<QuerySectionId>,
    ) -> Self {
        Self {
            source,
            save,
            kind,
            range,
            parent,
            next_sibling,
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct Query<'query> {
    pub states: Box<[Transition<'query>]>,
    pub queries: Box<[QuerySection<'query>]>,
    pub exit_at_section_end: Option<QuerySectionId>,
}

impl<'query> QuerySpec<'query> for Query<'query> {
    fn states(&self) -> &[Transition<'query>] {
        &self.states
    }

    fn queries(&self) -> &[QuerySection<'query>] {
        &self.queries
    }

    fn exit_at_section_end(&self) -> Option<QuerySectionId> {
        self.exit_at_section_end
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StaticQuery<'query, const N_STATES: usize, const N_SECTIONS: usize> {
    pub states: [Transition<'query>; N_STATES],
    pub queries: [QuerySection<'query>; N_SECTIONS],
    pub exit_at_section_end: Option<QuerySectionId>,
}

impl<'query, const N_STATES: usize, const N_SECTIONS: usize>
    StaticQuery<'query, N_STATES, N_SECTIONS>
{
    pub const fn new(
        states: [Transition<'query>; N_STATES],
        queries: [QuerySection<'query>; N_SECTIONS],
        exit_at_section_end: Option<QuerySectionId>,
    ) -> Self {
        Self {
            states,
            queries,
            exit_at_section_end,
        }
    }
}

impl<'query, const N_STATES: usize, const N_SECTIONS: usize> QuerySpec<'query>
    for StaticQuery<'query, N_STATES, N_SECTIONS>
{
    fn states(&self) -> &[Transition<'query>] {
        &self.states
    }

    fn queries(&self) -> &[QuerySection<'query>] {
        &self.queries
    }

    fn exit_at_section_end(&self) -> Option<QuerySectionId> {
        self.exit_at_section_end
    }
}

impl<'query> Query<'query> {
    pub fn first(
        query: &'query str,
        save: Save,
    ) -> Result<QueryBuilder<'query>, SelectorParseError> {
        let states = Transition::generate_transitions_from_string(query)?;
        let queries = vec![QuerySection::new(
            query,
            save,
            SelectionKind::First,
            TransitionId(0)..TransitionId(states.len()),
            None,
        )];

        Ok(QueryBuilder {
            states,
            selection: queries,
        })
    }

    pub fn all(query: &'query str, save: Save) -> Result<QueryBuilder<'query>, SelectorParseError> {
        let states = Transition::generate_transitions_from_string(query)?;
        let queries = vec![QuerySection::new(
            query,
            save,
            SelectionKind::All,
            TransitionId(0)..TransitionId(states.len()),
            None,
        )];

        Ok(QueryBuilder {
            states,
            selection: queries,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::query::compiler::transition::Transition;
    use crate::query::selector::AttributeSelection;
    use crate::query::selector::AttributeSelectionKind;
    use crate::query::selector::AttributeSelections;
    use crate::query::selector::ClassSelections;
    use crate::query::selector::Combinator;
    use crate::query::selector::ElementPredicate;
    use crate::{Query, QuerySection, QuerySectionId, Save, SelectionKind, TransitionId};

    #[test]
    fn test_query_builder_one_selection() {
        let query = Query::all("a", Save::all()).unwrap().build();

        assert_eq!(
            query.states.iter().as_slice(),
            [Transition::new(
                Combinator::Descendant,
                ElementPredicate {
                    name: Some("a"),
                    id: None,
                    classes: ClassSelections::from_static(&[]),
                    attributes: AttributeSelections::from_static(&[])
                }
            )]
        );

        assert_eq!(
            query.queries.iter().as_slice(),
            [QuerySection {
                source: "a",
                save: Save::all(),
                kind: SelectionKind::All,
                parent: None,
                range: TransitionId(0)..TransitionId(1),
                next_sibling: None,
            }]
        );
    }

    #[test]
    fn test_query_builder_chainned_selection() {
        let query = Query::first("span", Save::all())
            .unwrap()
            .all("a", Save::all())
            .unwrap()
            .build();

        assert_eq!(
            query.states.iter().as_slice(),
            [
                Transition::new(
                    Combinator::Descendant,
                    ElementPredicate {
                        name: Some("span"),
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
                )
            ]
        );
    }

    #[test]
    fn test_query_builder_chainned_multi_element_selection() {
        let query = Query::first("span#top.inner", Save::all())
            .unwrap()
            .all("a#link1.foo[href^=\"https\"]", Save::all())
            .unwrap()
            .build();

        assert_eq!(query.states.len(), 2);
        assert_eq!(query.queries.len(), 2);
        assert_eq!(
            query.states[1].predicate,
            ElementPredicate {
                name: Some("a"),
                id: Some("link1"),
                classes: ClassSelections::from_static(&["foo"]),
                attributes: AttributeSelections::from(vec![AttributeSelection {
                    name: "href",
                    value: Some("https"),
                    kind: AttributeSelectionKind::Prefix,
                }]),
            }
        );
    }

    #[test]
    fn test_query_builder_chainned_multi_element_selection_with_branching() {
        let query = Query::first("div", Save::all())
            .unwrap()
            .then(|ctx| {
                Ok([
                    ctx.all("a", Save::all())?,
                    ctx.first("p.note", Save::none())?,
                ])
            })
            .unwrap()
            .build();

        assert_eq!(query.queries.len(), 3);
        assert_eq!(query.queries[1].next_sibling, Some(QuerySectionId(2)));
        assert_eq!(query.queries[2].next_sibling, None);
    }
}
