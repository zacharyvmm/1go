use smallvec::SmallVec;
use std::ops::Range;

use super::builder::{QueryBuilder, Save, SelectionKind};
use super::error::SelectorParseError;
use super::transition::Transition;
use crate::query::selector::{Combinator, LocalSelectorList};

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

pub trait QuerySpec<'query> {
    fn states(&self) -> &[Transition<'query>];
    fn queries(&self) -> &[QuerySection<'query>];
    fn exit_at_section_end(&self) -> Option<QuerySectionId>;

    fn selection_ranges(&self, section: QuerySectionId) -> &[Range<TransitionId>];

    fn root_positions(&self) -> Vec<Position> {
        self.selection_ranges(QuerySectionId(0))
            .iter()
            .map(|range| Position {
                selection: QuerySectionId(0),
                state: range.start,
            })
            .collect()
    }

    fn child_positions(&self, position: &Position) -> SmallVec<[Position; 4]> {
        let mut positions = SmallVec::new();
        if position.selection.index() + 1 >= self.queries().len() {
            return positions;
        }
        let mut child = QuerySectionId(position.selection.index() + 1);
        loop {
            if self.get_selection(child).parent != Some(position.selection) {
                break;
            }
            for range in self.selection_ranges(child) {
                positions.push(Position {
                    selection: child,
                    state: range.start,
                });
            }
            let Some(next) = self.get_selection(child).next_sibling else {
                break;
            };
            child = next;
        }
        positions
    }

    fn has_child_positions(&self, position: &Position) -> bool {
        let Some(child) = self.queries().get(position.selection.index() + 1) else {
            return false;
        };
        child.parent == Some(position.selection)
    }

    fn previous_positions(&self, position: &Position) -> Vec<Position> {
        let Some(range) = self
            .selection_ranges(position.selection)
            .iter()
            .find(|range| range.contains(&position.state))
        else {
            return Vec::new();
        };

        if position.state.index() > range.start.index() {
            return vec![Position {
                selection: position.selection,
                state: TransitionId(position.state.index() - 1),
            }];
        }

        let Some(parent) = self.get_selection(position.selection).parent else {
            return Vec::new();
        };
        self.selection_ranges(parent)
            .iter()
            .filter_map(|range| {
                range.end.index().checked_sub(1).map(|state| Position {
                    selection: parent,
                    state: TransitionId(state),
                })
            })
            .collect()
    }

    /// Whether any compiled transition uses `+` or `~`.
    fn has_sibling_combinator(&self) -> bool {
        self.states().iter().any(|transition| {
            matches!(
                transition.guard,
                Combinator::NextSibling | Combinator::SubsequentSibling
            )
        })
    }

    fn has_structural_queries(&self) -> bool {
        self.states()
            .iter()
            .any(|transition| transition.predicate().requires_structural())
    }

    fn structural_filters(&'query self) -> Vec<&'query LocalSelectorList<'query>> {
        let mut filters: Vec<&'query LocalSelectorList<'query>> = Vec::new();
        for transition in self.states() {
            for predicate in transition.predicate().structural.as_slice() {
                if let crate::query::selector::StructuralPredicate::NthChildOf(_, filter) =
                    predicate
                    && !filters
                        .iter()
                        .any(|existing| std::ptr::eq(*existing, filter))
                {
                    filters.push(filter);
                }
            }
        }
        filters
    }

    fn requires_text_content(&self) -> bool {
        self.queries()
            .iter()
            .any(|section| section.save.text_content)
    }

    fn requires_attribute_storage(&self) -> bool {
        self.queries().iter().any(|section| section.save.attributes)
    }

    fn requires_attribute_parsing(&self) -> bool {
        self.requires_attribute_storage()
            || self
                .states()
                .iter()
                .any(|transition| transition.predicate().requires_attributes())
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
            return matches!(kind, SelectionKind::All) && self.has_child_positions(&position);
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
        self.selection_ranges(position.selection)
            .iter()
            .any(|range| range.end.index() - 1 == position.state.index())
    }

    fn is_last_save_point(&self, position: &Position) -> bool {
        debug_assert!(position.selection.index() < self.queries().len());
        let is_last_query = self.queries().len() - 1 == position.selection.index();
        let is_last_state = self.is_save_point(position);
        is_last_query && is_last_state
    }

    fn children(&'query self, position: &Position) -> Option<impl Iterator<Item = Position>>
    where
        Self: Sized,
    {
        let positions = self.child_positions(position);
        (!positions.is_empty()).then(|| positions.into_iter())
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
        let selection_range = query
            .selection_ranges(self.selection)
            .iter()
            .find(|range| range.contains(&self.state))?;
        if self.state.index() + 1 < selection_range.end.index() {
            Some(TransitionId(self.state.index() + 1))
        } else {
            None
        }
    }

    /// Returns the first selector alternative in the first child query section.
    ///
    /// Use [`QuerySpec::child_positions`] to traverse every child section and
    /// selector-list alternative.
    #[deprecated(note = "use QuerySpec::child_positions")]
    pub fn next_child<'query, Q: QuerySpec<'query> + ?Sized>(&self, query: &Q) -> Option<Self> {
        debug_assert!(self.selection.index() < query.queries().len());
        debug_assert!(
            query
                .selection_ranges(self.selection)
                .iter()
                .any(|range| range.contains(&self.state))
        );

        if self.selection.index() == query.queries().len() - 1 {
            return None;
        }

        let next_selection_index = QuerySectionId(self.selection.index() + 1);
        let next_selection = query.get_selection(next_selection_index);
        if next_selection.parent.is_some_and(|p| p == self.selection) {
            let first_alternative = query.selection_ranges(next_selection_index).first()?;
            return Some(Self {
                selection: next_selection_index,
                state: first_alternative.start,
            });
        }

        None
    }

    /// Returns the first selector alternative in the next sibling query section.
    ///
    /// This method does not traverse selector-list alternatives. Prefer
    /// [`QuerySpec::child_positions`] when enumerating a section's children.
    #[deprecated(note = "use QuerySpec::child_positions")]
    pub fn next_sibling<'query, Q: QuerySpec<'query> + ?Sized>(&self, query: &Q) -> Option<Self> {
        debug_assert!(self.selection.index() < query.queries().len());
        debug_assert!(
            query
                .selection_ranges(self.selection)
                .iter()
                .any(|range| range.contains(&self.state))
        );

        let sibling = query.get_selection(self.selection).next_sibling?;
        let first_alternative = query.selection_ranges(sibling).first()?;
        Some(Self {
            selection: sibling,
            state: first_alternative.start,
        })
    }

    /// Moves to one predecessor, choosing the first parent alternative at a
    /// query-section boundary.
    ///
    /// Use [`QuerySpec::previous_positions`] when every valid predecessor is
    /// required.
    #[deprecated(note = "use QuerySpec::previous_positions")]
    pub fn back<'query, Q: QuerySpec<'query> + ?Sized>(&mut self, query: &Q) {
        debug_assert!(self.selection.index() < query.queries().len());
        debug_assert!(
            query
                .selection_ranges(self.selection)
                .iter()
                .any(|range| range.contains(&self.state))
        );

        if let Some(previous) = query.previous_positions(self).first() {
            *self = *previous;
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
    pub alternatives: Box<[Box<[Range<TransitionId>]>]>,
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

    fn selection_ranges(&self, section: QuerySectionId) -> &[Range<TransitionId>] {
        &self.alternatives[section.index()]
    }

    fn root_positions(&self) -> Vec<Position> {
        self.selection_ranges(QuerySectionId(0))
            .iter()
            .map(|range| Position {
                selection: QuerySectionId(0),
                state: range.start,
            })
            .collect()
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct StaticQuery<'query, const N_STATES: usize, const N_SECTIONS: usize> {
    pub states: [Transition<'query>; N_STATES],
    pub queries: [QuerySection<'query>; N_SECTIONS],
    pub exit_at_section_end: Option<QuerySectionId>,
    pub alternatives: &'query [&'query [Range<TransitionId>]],
}

impl<'query, const N_STATES: usize, const N_SECTIONS: usize>
    StaticQuery<'query, N_STATES, N_SECTIONS>
{
    pub const fn new(
        states: [Transition<'query>; N_STATES],
        queries: [QuerySection<'query>; N_SECTIONS],
        exit_at_section_end: Option<QuerySectionId>,
        alternatives: &'query [&'query [Range<TransitionId>]],
    ) -> Self {
        Self {
            states,
            queries,
            exit_at_section_end,
            alternatives,
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

    fn selection_ranges(&self, section: QuerySectionId) -> &[Range<TransitionId>] {
        self.alternatives[section.index()]
    }
}

impl<'query> Query<'query> {
    pub fn first(
        query: &'query str,
        save: Save,
    ) -> Result<QueryBuilder<'query>, SelectorParseError> {
        Self::build_initial(query, save, SelectionKind::First)
    }

    fn build_initial(
        query: &'query str,
        save: Save,
        kind: SelectionKind,
    ) -> Result<QueryBuilder<'query>, SelectorParseError> {
        let paths = Transition::generate_transition_paths_from_string(query)?;
        let mut states = Vec::new();
        let mut alternatives = Vec::new();
        for path in paths {
            let start = TransitionId(states.len());
            states.extend(path);
            alternatives.push(start..TransitionId(states.len()));
        }
        let range = alternatives.first().unwrap().start..alternatives.last().unwrap().end;
        let queries = vec![QuerySection::new(query, save, kind, range, None)];

        Ok(QueryBuilder {
            states,
            selection: queries,
            alternatives: vec![alternatives],
        })
    }

    pub fn all(query: &'query str, save: Save) -> Result<QueryBuilder<'query>, SelectorParseError> {
        Self::build_initial(query, save, SelectionKind::All)
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
    use crate::{
        Position, Query, QuerySection, QuerySectionId, QuerySpec, Save, SelectionKind, TransitionId,
    };

    #[test]
    fn sibling_feature_scan_tracks_public_guard_mutation() {
        let mut query = Query::all("div p", Save::none()).unwrap().build();
        assert!(!query.has_sibling_combinator());

        query.states[1].guard = Combinator::NextSibling;

        assert!(query.has_sibling_combinator());
    }

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
                    attributes: AttributeSelections::from_static(&[]),
                    logical: crate::LogicalPredicates::from_static(&[]),
                    structural: crate::StructuralPredicates::from_static(&[]),
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
            query.states[1].predicate(),
            &ElementPredicate {
                name: Some("a"),
                id: Some("link1"),
                classes: ClassSelections::from_static(&["foo"]),
                attributes: AttributeSelections::from(vec![AttributeSelection {
                    name: "href",
                    value: Some("https"),
                    kind: AttributeSelectionKind::Prefix,
                    case_sensitivity: crate::AttributeCaseSensitivity::Default,
                }]),
                logical: crate::LogicalPredicates::from_static(&[]),
                structural: crate::StructuralPredicates::from_static(&[]),
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

    #[test]
    fn previous_positions_include_every_parent_selector_alternative() {
        let query = Query::all("article > p, section > div", Save::none())
            .unwrap()
            .all("span", Save::none())
            .unwrap()
            .build();
        let position = query.child_positions(&Position {
            selection: QuerySectionId(0),
            state: TransitionId(1),
        })[0];

        assert_eq!(
            query.previous_positions(&position),
            vec![
                Position {
                    selection: QuerySectionId(0),
                    state: TransitionId(1),
                },
                Position {
                    selection: QuerySectionId(0),
                    state: TransitionId(3),
                },
            ]
        );
    }

    #[test]
    fn position_traversal_visits_every_child_selector_alternative() {
        let query = Query::all("main", Save::none())
            .unwrap()
            .then(|main| {
                Ok([
                    main.all("h1, h2", Save::none())?,
                    main.all("p, aside", Save::none())?,
                ])
            })
            .unwrap()
            .build();
        let root = query.root_positions()[0];
        let expected = query.child_positions(&root);
        let traversed: Vec<_> = query.children(&root).unwrap().collect();

        assert_eq!(traversed, expected.as_slice());
        assert_eq!(
            traversed,
            vec![
                Position {
                    selection: QuerySectionId(1),
                    state: TransitionId(1),
                },
                Position {
                    selection: QuerySectionId(1),
                    state: TransitionId(2),
                },
                Position {
                    selection: QuerySectionId(2),
                    state: TransitionId(3),
                },
                Position {
                    selection: QuerySectionId(2),
                    state: TransitionId(4),
                },
            ]
        );
    }
}
