use super::cursor::{SENTINEL_SCOPE, ScopedCursor};
use super::multiplexer::{DocumentPosition, SaveHit};
use crate::debug::ScopedCursorReason;
#[cfg(any(debug_assertions, test))]
use crate::debug::{
    CursorSuppressionReason, CursorTraceKind, TraceEvent, TransitionRejectReason,
};
use crate::store::ElementId;
use crate::store::Store;
use crate::{Combinator, Position, QuerySectionId, QuerySpec, SelectionKind, TransitionId, XHtmlElement};
use smallvec::SmallVec;

enum SpawnOutcome {
    Inserted,
    Dominated,
}

/// NFA execution engine for streaming StAX events.
///
/// Cursor 0 is the sentinel root. It is never depth-pruned; query progress is
/// represented by spawned moving cursors, while anchored cursors keep
/// descendant searches alive within their scope.
pub struct QueryExecutor<'a, Q> {
    pub(crate) query: &'a Q,
    pub(crate) cursors: Vec<ScopedCursor>,
}

impl<'a, 'html, 'query: 'html, Q> QueryExecutor<'a, Q>
where
    Q: QuerySpec<'query>,
{
    pub fn new(query: &'a Q) -> Self {
        let root = ScopedCursor::new_root(
            ElementId::default(),
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
        );
        Self {
            query,
            cursors: vec![root],
        }
    }

    pub fn query(&self) -> &Q {
        self.query
    }

    pub fn save_element(
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] runner_index: usize,
        tree: &Q,
        store: &mut Store<'html, 'query>,
        element: XHtmlElement<'html>,
        cursor: &mut ScopedCursor,
    ) -> SaveHit {
        let section = tree.get_selection(cursor.get_position().selection);

        let element_pointer = store.push(cursor.get_parent(), section, element);
        crate::scah_trace!(
            store,
            TraceEvent::ElementSaved {
                runner_index,
                selector: section.source,
                element: store.elements[element_pointer].name,
                element_id: element_pointer,
                parent_id: cursor.get_parent(),
                save_inner_html: section.save.inner_html,
                save_text_content: section.save.text_content,
            }
        );
        if !tree.is_last_save_point(cursor.get_position()) {
            cursor.set_parent(element_pointer);
        }

        SaveHit {
            element_id: element_pointer,
            save_inner_html: section.save.inner_html,
            save_text_content: section.save.text_content,
        }
    }

    #[cfg(any(debug_assertions, test))]
    fn transition_reject_reason(
        tree: &Q,
        position: &crate::Position,
        depth: super::DepthSize,
        last_depth: super::DepthSize,
        element: &XHtmlElement<'html>,
    ) -> TransitionRejectReason {
        let transition = tree.get_transition(position.state);
        if transition.predicate.matches_element(element) {
            let _ = (depth, last_depth);
            TransitionRejectReason::DepthGuardFailed
        } else {
            TransitionRejectReason::PredicateFailed
        }
    }

    #[cfg(any(debug_assertions, test))]
    fn trace_kind(&self, index: usize) -> CursorTraceKind {
        if index == 0 && self.cursors[0].scope_depth == SENTINEL_SCOPE {
            CursorTraceKind::Root
        } else {
            CursorTraceKind::Scoped { index }
        }
    }

    fn existing_dominates(
        existing: &ScopedCursor,
        candidate: &ScopedCursor,
        query: &Q,
    ) -> bool {
        if existing.end() {
            return false;
        }
        if existing.parent != candidate.parent || existing.position != candidate.position {
            return false;
        }

        let existing_base = existing.match_base_depth();
        let candidate_base = candidate.match_base_depth();

        match &query.get_transition(candidate.position.state).guard {
            Combinator::Descendant => {
                if existing_base <= candidate_base {
                    true
                } else {
                    debug_assert!(
                        false,
                        "shallower descendant candidate while deeper exists"
                    );
                    false
                }
            }
            Combinator::Child => existing_base == candidate_base,
            Combinator::NextSibling | Combinator::SubsequentSibling | Combinator::Namespace => {
                existing_base == candidate_base
            }
        }
    }

    fn try_push_cursor(
        &mut self,
        candidate: ScopedCursor,
        runner_index: usize,
        store: &mut Store<'html, 'query>,
        create_reason: Option<ScopedCursorReason>,
    ) -> SpawnOutcome {
        for existing in self.cursors.iter().rev() {
            if Self::existing_dominates(existing, &candidate, self.query) {
                #[cfg(any(debug_assertions, test))]
                {
                    let reason = match &self.query.get_transition(candidate.position.state).guard {
                        Combinator::Descendant => {
                            if existing.match_base_depth() == candidate.match_base_depth() {
                                CursorSuppressionReason::ExactDuplicate
                            } else {
                                CursorSuppressionReason::DescendantDominated
                            }
                        }
                        _ => CursorSuppressionReason::ExactDuplicate,
                    };
                    crate::scah_trace!(
                        store,
                        TraceEvent::CursorSuppressed {
                            runner_index,
                            parent: candidate.parent,
                            selection: candidate.position.selection,
                            state: candidate.position.state,
                            candidate_base_depth: candidate.match_base_depth(),
                            dominating_base_depth: existing.match_base_depth(),
                            reason,
                        }
                    );
                }
                return SpawnOutcome::Dominated;
            }
        }

        if let Some(reason) = create_reason {
            #[cfg(any(debug_assertions, test))]
            {
                crate::scah_trace!(
                    store,
                    TraceEvent::ScopedCursorCreated {
                        runner_index,
                        depth: candidate.scope_depth,
                        scope_depth: candidate.scope_depth,
                        parent: candidate.parent,
                        selection: candidate.position.selection,
                        state: candidate.position.state,
                        reason,
                    }
                );
            }
        }

        self.cursors.push(candidate);
        SpawnOutcome::Inserted
    }

    pub fn next(
        &mut self,
        runner_index: usize,
        element: &XHtmlElement<'html>,
        document_position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        save_hits: &mut Vec<SaveHit>,
    ) {
        let depth = document_position.element_depth;
        let snapshot_len = self.cursors.len();

        // A single element visit can be reached by several descendant forks.
        // Track saves keyed by (parent scope, section) so the same physical
        // element is stored once per scope+section: a flat descendant selector
        // dedups globally (shared root parent), while distinct `.then()`
        // parents each keep their own copy (distinct parent ids).
        let mut saved_this_step: SmallVec<[(ElementId, QuerySectionId, ElementId); 4]> =
            SmallVec::new();

        for i in 0..snapshot_len {
            if self.cursors[i].end() {
                continue;
            }

            let position = self.cursors[i].position;
            let matched = self.cursors[i].next(self.query, depth, element);

            if !matched {
                #[cfg(any(debug_assertions, test))]
                {
                    let last_depth = self.cursors[i].match_base_depth();
                    crate::scah_trace!(
                        store,
                        TraceEvent::TransitionRejected {
                            runner_index,
                            cursor: self.trace_kind(i),
                            selector: self.query.get_selection(position.selection).source,
                            element: element.name,
                            depth,
                            selection: position.selection,
                            state: position.state,
                            reason: Self::transition_reject_reason(
                                self.query, &position, depth, last_depth, element,
                            ),
                        }
                    );
                }
                continue;
            }

            crate::scah_trace!(
                store,
                TraceEvent::TransitionMatched {
                    runner_index,
                    cursor: self.trace_kind(i),
                    selector: self.query.get_selection(position.selection).source,
                    element: element.name,
                    depth,
                    selection: position.selection,
                    state: position.state,
                }
            );

            let is_descendant = self.query.is_descendant(position.state);
            let is_save_point = self.query.is_save_point(&position);
            let is_section_end = is_save_point;
            let section_kind = self.query.get_section_selection_kind(position.selection);
            let is_first = matches!(section_kind, SelectionKind::First);
            let self_closing = document_position.self_closing;
            let terminal_all = is_section_end
                && matches!(section_kind, SelectionKind::All)
                && position.next_child(self.query).is_none();

            let spawned_positions;

            match &self.cursors[i].mode {
                super::cursor::CursorMode::Moving { .. } => {
                    // `save_element` advances the parent for children; restore it
                    // afterward so this cursor can still match later siblings.
                    let original_parent = self.cursors[i].parent;
                    let needs_anchor = self.query.needs_descendant_anchor(position);
                    let anchor_candidate =
                        needs_anchor.then(|| self.cursors[i].anchor_clone(depth));

                    let saved_parent = if is_save_point {
                        let save_parent = self.cursors[i].parent;
                        if let Some(existing) = saved_this_step
                            .iter()
                            .find(|(parent, section, _)| {
                                *parent == save_parent && *section == position.selection
                            })
                            .map(|(_, _, element_id)| *element_id)
                        {
                            // Same physical element already saved under this
                            // parent scope + section this visit: skip the
                            // duplicate store push, keep parenting consistent.
                            if self.query.is_last_save_point(&position) {
                                save_parent
                            } else {
                                existing
                            }
                        } else {
                            let hit = Self::save_element(
                                runner_index,
                                self.query,
                                store,
                                element.clone(),
                                &mut self.cursors[i],
                            );
                            let sp = self.cursors[i].parent;
                            self.cursors[i].parent = original_parent;
                            saved_this_step.push((save_parent, position.selection, hit.element_id));
                            save_hits.push(hit);
                            sp
                        }
                    } else {
                        original_parent
                    };

                    if self_closing {
                        continue;
                    }

                    if terminal_all {
                        self.cursors[i].set_unwind_depth(depth);
                        continue;
                    }

                    if is_descendant || is_section_end {
                        self.cursors[i].block_until_close(depth);
                    }

                    if let Some(anchor) = anchor_candidate {
                        let _ = self.try_push_cursor(
                            anchor,
                            runner_index,
                            store,
                            Some(ScopedCursorReason::DescendantFork),
                        );
                    }

                    spawned_positions = self.cursors[i].next_positions(self.query);
                    for pos in &spawned_positions {
                        let continuation =
                            ScopedCursor::new_moving(depth, saved_parent, *pos);
                        let _ = self.try_push_cursor(continuation, runner_index, store, None);
                    }
                }
                super::cursor::CursorMode::Anchored { .. } => {
                    if self_closing {
                        if is_save_point {
                            let save_parent = self.cursors[i].parent;
                            let already = saved_this_step.iter().any(|(parent, section, _)| {
                                *parent == save_parent && *section == position.selection
                            });
                            if !already {
                                let mut base = ScopedCursor::new_moving(
                                    depth,
                                    save_parent,
                                    self.cursors[i].position,
                                );
                                let hit = Self::save_element(
                                    runner_index,
                                    self.query,
                                    store,
                                    element.clone(),
                                    &mut base,
                                );
                                saved_this_step.push((
                                    save_parent,
                                    position.selection,
                                    hit.element_id,
                                ));
                                save_hits.push(hit);
                            }
                        }
                        continue;
                    }

                    spawned_positions = self.cursors[i].next_positions(self.query);

                    let saved_parent = if is_save_point {
                        let save_parent = self.cursors[i].parent;
                        if let Some(existing) = saved_this_step
                            .iter()
                            .find(|(parent, section, _)| {
                                *parent == save_parent && *section == position.selection
                            })
                            .map(|(_, _, element_id)| *element_id)
                        {
                            if self.query.is_last_save_point(&position) {
                                save_parent
                            } else {
                                existing
                            }
                        } else {
                            let mut base = ScopedCursor::new_moving(
                                depth,
                                save_parent,
                                self.cursors[i].position,
                            );
                            let hit = Self::save_element(
                                runner_index,
                                self.query,
                                store,
                                element.clone(),
                                &mut base,
                            );
                            saved_this_step.push((save_parent, position.selection, hit.element_id));
                            save_hits.push(hit);
                            base.parent
                        }
                    } else {
                        self.cursors[i].parent
                    };

                    if is_first && is_section_end {
                        self.cursors[i].set_end(true);
                    }

                    for pos in &spawned_positions {
                        let continuation =
                            ScopedCursor::new_moving(depth, saved_parent, *pos);
                        let _ = self.try_push_cursor(continuation, runner_index, store, None);
                    }
                }
            }
        }
    }

    pub fn early_exit(&self) -> bool {
        if let Some(exit_section) = self.query.exit_at_section_end() {
            // `.first()` can exit only after the root match and any `.then()`
            // branches have all completed.
            let root = &self.cursors[0];
            if root.position.selection != exit_section || !root.end() {
                return false;
            }
            return self.cursors[1..].iter().all(|c| c.end());
        }
        false
    }

    pub fn back(
        &mut self,
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] runner_index: usize,
        _element: &'html str,
        document_position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
    ) -> bool {
        let close_depth = document_position.element_depth;
        let mut last_pruned_parent = None;
        let mut significant_close = false;

        // Walk backwards so `swap_remove` cannot move an unvisited cursor.
        let mut i = self.cursors.len();
        while i > 0 {
            i -= 1;
            let cur = &self.cursors[i];

            if cur.scope_depth == SENTINEL_SCOPE {
                if cur.unwind_depth() == Some(close_depth) {
                    if cur.end() {
                        let section_kind = self
                            .query
                            .get_section_selection_kind(cur.position.selection);
                        if matches!(section_kind, SelectionKind::First) {
                            self.cursors[i].position.back(self.query);
                            #[cfg(any(debug_assertions, test))]
                            if let Some(section) = self.query.exit_at_section_end() {
                                crate::scah_trace!(
                                    store,
                                    TraceEvent::EarlyExit {
                                        runner_index,
                                        selector: self.query.get_selection(section).source,
                                        section,
                                    }
                                );
                            }
                        } else {
                            self.cursors[i].reactivate_after_close();
                        }
                    } else {
                        self.cursors[i].position.back(self.query);
                        self.cursors[i].reactivate_after_close();
                    }
                    significant_close = true;
                }
            } else if cur.scope_depth >= close_depth {
                let pruned = self.cursors.swap_remove(i);
                last_pruned_parent = Some(pruned.parent);
                significant_close = true;

                crate::scah_trace!(
                    store,
                    TraceEvent::ScopedCursorPruned {
                        runner_index,
                        cursor_index: i,
                        scope_depth: pruned.scope_depth,
                        close_depth,
                        selection: pruned.position.selection,
                        state: pruned.position.state,
                    }
                );
            } else if cur.is_moving() && cur.unwind_depth() == Some(close_depth) {
                if cur.end() {
                    let section_kind = self
                        .query
                        .get_section_selection_kind(cur.position.selection);
                    if matches!(section_kind, SelectionKind::First) {
                        self.cursors[i].position.back(self.query);
                    } else {
                        self.cursors[i].reactivate_after_close();
                    }
                } else {
                    self.cursors[i].reactivate_after_close();
                }
                significant_close = true;
            }
        }

        // Restore the parent for future sibling matches after scoped cursors
        // are pruned at the close boundary.
        if let Some(parent) = last_pruned_parent
            && let Some(root) = self.cursors.first_mut()
            && root.scope_depth == SENTINEL_SCOPE
        {
            root.parent = parent;
        }

        significant_close
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::{
        Element, ElementId, Position, Query, QuerySectionId, Reader, Save, TransitionId,
        XHtmlElement,
    };
    use crate::{QueryMultiplexer, XHtmlParser};

    fn anchored_cursor(scope_depth: u16, parent: ElementId, position: Position) -> ScopedCursor {
        ScopedCursor::new_anchored(scope_depth, parent, position)
    }

    fn elem(name: &'static str) -> XHtmlElement<'static> {
        XHtmlElement {
            name,
            id: None,
            class: None,
            attributes: &[],
        }
    }

    fn doc_pos(depth: u16) -> DocumentPosition {
        DocumentPosition {
            reader_position: 0,
            text_content_position: 0,
            element_depth: depth,
            self_closing: false,
        }
    }

    fn terminal_state<Q: QuerySpec<'static>>(query: &Q) -> TransitionId {
        let section = query.get_selection(QuerySectionId(0));
        TransitionId(section.range.end.index() - 1)
    }

    fn live_moving_cursors_at(
        selection: &QueryExecutor<'_, Query>,
        state: TransitionId,
        parent: ElementId,
    ) -> usize {
        selection
            .cursors
            .iter()
            .filter(|c| {
                !c.end()
                    && c.is_moving()
                    && c.position.state == state
                    && c.parent == parent
            })
            .count()
    }

    #[test]
    fn test_fsm_next_descendant() {
        let query = &Query::all("div a", Save::none()).unwrap().build();

        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        selection.next(
            0,
            &XHtmlElement {
                name: "div",
                id: None,
                class: None,
                attributes: &[],
            },
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: false,
            },
            &mut store,
            &mut Vec::new(),
        );

        assert!(store.get("div a").is_none());

        assert_eq!(selection.cursors[0].position.state, TransitionId(0));
        assert_eq!(selection.cursors.len(), 2);
        let spawned = selection
            .cursors
            .iter()
            .find(|c| c.is_moving() && c.position.state == TransitionId(1))
            .expect("Should have spawned MOVING cursor at state 1");
        assert_eq!(spawned.scope_depth, 0);

        selection.next(
            0,
            &XHtmlElement {
                name: "a",
                id: None,
                class: None,
                attributes: &[],
            },
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 1,
                self_closing: false,
            },
            &mut store,
            &mut Vec::new(),
        );

        assert_eq!(store.get("div a").unwrap().count(), 1);
        let children = store.get("div a").unwrap();
        let children: Vec<&Element> = children.collect();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "a");
    }

    #[test]
    fn test_complex_fsm_query() {
        let query = &Query::first("div p.class", Save::none())
            .unwrap()
            .then(|p| Ok([p.first("span", Save::none())?, p.first("a", Save::none())?]))
            .unwrap()
            .build();

        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        selection.next(
            0,
            &XHtmlElement {
                name: "div",
                id: None,
                class: None,
                attributes: &[],
            },
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: false,
            },
            &mut store,
            &mut Vec::new(),
        );

        assert!(store.get("div p.class").is_none());

        selection.next(
            0,
            &XHtmlElement {
                name: "p",
                id: None,
                class: Some("class"),
                attributes: &[],
            },
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 1,
                self_closing: false,
            },
            &mut store,
            &mut Vec::new(),
        );

        assert_eq!(store.get("div p.class").unwrap().count(), 1);
        let children = store.get("div p.class").unwrap();
        let children: Vec<&Element> = children.collect();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "p");
        assert_eq!(children[0].class, Some("class"));
    }

    #[test]
    fn test_scoped_fsm_pruning_removes_interleaved_expired_cursors() {
        let query = Query::first("article", Save::none()).unwrap().build();
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(&query);
        let position = Position {
            selection: QuerySectionId(0),
            state: TransitionId(0),
        };

        selection.cursors = vec![
            ScopedCursor::new_root(ElementId(0), position),
            anchored_cursor(1, ElementId(10), position),
            anchored_cursor(3, ElementId(20), position),
            anchored_cursor(1, ElementId(30), position),
            anchored_cursor(2, ElementId(40), position),
            anchored_cursor(0, ElementId(50), position),
        ];

        let _ = selection.back(
            0,
            "section",
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 2,
                self_closing: false,
            },
            &mut store,
        );

        let retained = &selection.cursors;
        assert_eq!(retained.len(), 4, "Should retain root + 3 scoped < 2");
        assert!(
            retained[0].scope_depth == SENTINEL_SCOPE,
            "Root should be kept"
        );
        assert!(retained[1..].iter().all(|c| c.scope_depth < 2));

        let mut retained_parents: Vec<usize> =
            retained[1..].iter().map(|c| c.parent.index()).collect();
        retained_parents.sort_unstable();
        assert_eq!(retained_parents, vec![10, 30, 50]);
    }

    #[test]
    fn test_simple_open_close() {
        let query = Query::all("div", Save::none()).unwrap().build();

        let mut store = Store::default();
        let mut selection = QueryExecutor::new(&query);

        selection.next(
            0,
            &XHtmlElement {
                name: "div",
                id: None,
                class: None,
                attributes: &[],
            },
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: false,
            },
            &mut store,
            &mut Vec::new(),
        );
        store.text_content.set_start(4);

        assert_eq!(selection.cursors[0].position.state, TransitionId(0));
        assert!(!selection.cursors.is_empty());

        store.text_content.push(&Reader::new("<div></div>"), 4);
        let reactivated = selection.back(
            0,
            "div",
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: false,
            },
            &mut store,
        );

        assert!(reactivated);
        assert!(!selection.cursors[0].end());
    }

    #[test]
    fn test_descendant_forking_with_anchoring_model() {
        let query = &Query::all("div a", Save::none()).unwrap().build();
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        selection.next(
            0,
            &XHtmlElement {
                name: "div",
                id: None,
                class: None,
                attributes: &[],
            },
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: false,
            },
            &mut store,
            &mut Vec::new(),
        );

        let anchored_count = selection.cursors.iter().filter(|c| c.is_anchored()).count();
        assert_eq!(
            anchored_count, 0,
            "No anchored fork when next transition is also descendant (div a)"
        );
    }

    #[test]
    fn test_child_combinator_sibling_rematching() {
        let html = "<main><section>A</section><section>B</section></main>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("main > section", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let sections: Vec<_> = store.get("main > section").unwrap().collect();
        assert_eq!(sections.len(), 2, "Expected 2 section matches");
    }

    #[test]
    fn test_nested_descendant_matching() {
        let html = "<div><div><a>link</a></div></div>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("div div a", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let links: Vec<_> = store.get("div div a").unwrap().collect();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].name, "a");
    }

    #[test]
    fn test_mixed_child_and_descendant() {
        let html = "<main><section><div><a>link</a></div></section></main>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("main > section div a", Save::all())
            .unwrap()
            .build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let links: Vec<_> = store.get("main > section div a").unwrap().collect();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].name, "a");
    }

    #[test]
    fn test_then_branching_with_anchoring_model() {
        let html = r#"<section><div class="product"><h1>P1</h1><img src="p1.png" /><p>Desc</p></div></section>"#;
        let reader = &mut Reader::new(html);
        let query = &[Query::all("section .product", Save::all())
            .unwrap()
            .then(|p| {
                Ok([
                    p.all("h1", Save::all())?,
                    p.all("img", Save::none())?,
                    p.all("p", Save::all())?,
                ])
            })
            .unwrap()
            .build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let products: Vec<_> = store.get("section .product").unwrap().collect();
        assert_eq!(products.len(), 1);

        let product = products[0];
        let h1s: Vec<_> = product.get(&store, "h1").unwrap().collect();
        assert_eq!(h1s.len(), 1);
        let imgs: Vec<_> = product.get(&store, "img").unwrap().collect();
        assert_eq!(imgs.len(), 1);
        let ps: Vec<_> = product.get(&store, "p").unwrap().collect();
        assert_eq!(ps.len(), 1);
    }

    #[test]
    fn test_self_closing_elements_preserved() {
        let html = "<div><br /><span>text</span></div>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("div span", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let spans: Vec<_> = store.get("div span").unwrap().collect();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "span");
    }

    #[test]
    fn test_implicit_li_close() {
        let html = "<ul><li>Item 1<li>Item 2</ul>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("ul li", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let items: Vec<_> = store.get("ul li").unwrap().collect();
        assert_eq!(items.len(), 2, "Expected 2 li matches with implicit close");
    }

    #[test]
    fn test_multiple_nested_descendant_levels() {
        let html = "<body><div><ul><li><a href='#'>link</a></li></ul></div></body>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("body div ul li a", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let links: Vec<_> = store.get("body div ul li a").unwrap().collect();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].name, "a");
    }

    #[test]
    fn test_then_first_selection_only_matches_once() {
        let html = "<article><h1>First</h1><h1>Second</h1><a href='1'>link1</a><a href='2'>link2</a></article>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("article", Save::none())
            .unwrap()
            .then(|article| {
                Ok([
                    article.first("h1", Save::all())?,
                    article.all("a[href]", Save::all())?,
                ])
            })
            .unwrap()
            .build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let articles: Vec<_> = store.get("article").unwrap().collect();
        assert_eq!(articles.len(), 1);

        let h1s: Vec<_> = articles[0].get(&store, "h1").unwrap().collect();
        assert_eq!(
            h1s.len(),
            1,
            "first('h1') should match only one h1, not all"
        );
        assert_eq!(h1s[0].name, "h1");

        let links: Vec<_> = articles[0].get(&store, "a[href]").unwrap().collect();
        assert_eq!(links.len(), 2, "all('a[href]') should match all links");
    }

    #[test]
    fn test_store_push_then_pattern() {
        let query = &Query::all("div", Save::none())
            .unwrap()
            .then(|div| Ok([div.first("p", Save::all())?]))
            .unwrap()
            .build();

        let mut store = Store::default();

        let div_id = store.push(
            ElementId::default(),
            &query.queries[0],
            XHtmlElement {
                name: "div",
                id: None,
                class: None,
                attributes: &[],
            },
        );
        assert_eq!(div_id, ElementId(0));
        assert!(store.get("div").is_some(), "div query should exist");

        let p_id = store.push(
            div_id,
            &query.queries[1],
            XHtmlElement {
                name: "p",
                id: None,
                class: None,
                attributes: &[],
            },
        );
        assert_eq!(p_id, ElementId(1));

        let divs: Vec<_> = store.get("div").unwrap().collect();
        let div = divs[0];
        let ps: Vec<_> = div.get(&store, "p").unwrap().collect();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].name, "p");
    }

    #[test]
    fn test_then_single_first_child_direct_executor() {
        let query = &Query::all("div", Save::none())
            .unwrap()
            .then(|div| Ok([div.first("p", Save::all())?]))
            .unwrap()
            .build();

        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        selection.next(
            0,
            &XHtmlElement {
                name: "div",
                id: None,
                class: None,
                attributes: &[],
            },
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: false,
            },
            &mut store,
            &mut Vec::new(),
        );

        assert_eq!(selection.cursors[0].position.selection, QuerySectionId(0));
        assert!(
            store.get("div").is_some(),
            "div should be in store even with Save::none()"
        );

        let p_cursors: Vec<_> = selection
            .cursors
            .iter()
            .filter(|c| c.position.selection == QuerySectionId(1))
            .collect();
        assert!(
            !p_cursors.is_empty(),
            "Should have spawned cursor for p section"
        );

        let mut save_hits = Vec::new();
        selection.next(
            0,
            &XHtmlElement {
                name: "p",
                id: None,
                class: None,
                attributes: &[],
            },
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 1,
                self_closing: false,
            },
            &mut store,
            &mut save_hits,
        );

        assert!(!save_hits.is_empty(), "p should have save hits");
        assert_eq!(save_hits[0].element_id, ElementId(1));

        let p_cursor = selection
            .cursors
            .iter()
            .find(|c| c.position.selection == QuerySectionId(1) && c.is_moving())
            .unwrap();
        assert!(
            p_cursor.end(),
            "First p cursor should be at end after match"
        );

        let mut save_hits2 = Vec::new();
        selection.next(
            0,
            &XHtmlElement {
                name: "p",
                id: None,
                class: None,
                attributes: &[],
            },
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 1,
                self_closing: false,
            },
            &mut store,
            &mut save_hits2,
        );

        assert!(save_hits2.is_empty(), "Second p should NOT be saved");

        let divs: Vec<_> = store.get("div").unwrap().collect();
        let div = divs[0];
        let ps: Vec<_> = div.get(&store, "p").unwrap().collect();
        assert_eq!(ps.len(), 1, "Only one p should be saved");
    }

    #[test]
    fn test_then_single_first_child_no_descendant() {
        let html = "<div><p>A</p><p>B</p></div>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("div", Save::none())
            .unwrap()
            .then(|div| Ok([div.first("p", Save::all())?]))
            .unwrap()
            .build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let divs: Vec<_> = store.get("div").unwrap().collect();
        assert_eq!(divs.len(), 1, "Should match one div");
        let ps: Vec<_> = divs[0].get(&store, "p").unwrap().collect();
        assert_eq!(ps.len(), 1, "first('p') should match only one <p>");
    }

    #[test]
    fn test_then_multiple_product_cards_first_h1() {
        let html = r#"<div>
            <div class="product"><h1>P1</h1><p>Desc1</p></div>
            <div class="product"><h1>P2</h1><p>Desc2</p></div>
        </div>"#;
        let reader = &mut Reader::new(html);
        let query = &[Query::all("div .product", Save::all())
            .unwrap()
            .then(|product| {
                Ok([
                    product.first("h1", Save::all())?,
                    product.all("p", Save::all())?,
                ])
            })
            .unwrap()
            .build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let products: Vec<_> = store.get("div .product").unwrap().collect();
        assert_eq!(products.len(), 2, "Should match 2 product cards");

        for product in &products {
            let h1s: Vec<_> = product.get(&store, "h1").unwrap().collect();
            assert_eq!(h1s.len(), 1, "Each product should have exactly 1 h1");
            let ps: Vec<_> = product.get(&store, "p").unwrap().collect();
            assert_eq!(ps.len(), 1, "Each product should have exactly 1 p");
        }
    }

    #[test]
    fn test_spawn_model_root_stays_after_match() {
        let query = &Query::all("div a", Save::none()).unwrap().build();
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        selection.next(
            0,
            &XHtmlElement {
                name: "div",
                id: None,
                class: None,
                attributes: &[],
            },
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: false,
            },
            &mut store,
            &mut Vec::new(),
        );

        let root = &selection.cursors[0];
        assert_eq!(root.position.state, TransitionId(0));
        assert_eq!(
            root.position.selection,
            QuerySectionId(0),
            "Root cursor should stay at initial section"
        );
        assert!(
            root.end(),
            "Root should deactivate after descendant match (anchored fork handles deeper matches)"
        );
    }

    #[test]
    fn test_spawn_model_all_rematches_after_close() {
        let html = "<div><p>A</p><p>B</p></div>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("div p", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let ps: Vec<_> = store.get("div p").unwrap().collect();
        assert_eq!(ps.len(), 2, "Should match all p elements");
    }

    #[test]
    fn test_spawn_model_first_does_not_rematch() {
        let html = "<div><span>A</span><span>B</span></div>";
        let reader = &mut Reader::new(html);
        let query = &[Query::first("div span", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let spans: Vec<_> = store.get("div span").unwrap().collect();
        assert_eq!(spans.len(), 1, "First selection should match only 1 span");
    }

    #[test]
    fn test_spawn_model_sentinel_never_pruned() {
        let query = Query::all("div", Save::none()).unwrap().build();
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(&query);

        let position = Position {
            selection: QuerySectionId(0),
            state: TransitionId(0),
        };

        selection.cursors = vec![
            ScopedCursor::new_root(ElementId(0), position),
            anchored_cursor(1, ElementId(10), position),
        ];

        let _ = selection.back(
            0,
            "div",
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: false,
            },
            &mut store,
        );

        assert_eq!(selection.cursors.len(), 1, "Only root should remain");
        assert_eq!(
            selection.cursors[0].scope_depth, SENTINEL_SCOPE,
            "Root cursor must not be pruned"
        );
    }

    #[test]
    fn test_spawn_model_early_exit_first_selection() {
        let query = &Query::first("div", Save::all()).unwrap().build();
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        selection.next(
            0,
            &XHtmlElement {
                name: "div",
                id: None,
                class: None,
                attributes: &[],
            },
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: false,
            },
            &mut store,
            &mut Vec::new(),
        );

        assert!(
            selection.early_exit(),
            "early_exit should be true after First match completes"
        );

        store.text_content.set_start(4);
        store.text_content.push(&Reader::new("<div></div>"), 4);
        let reactivated = selection.back(
            0,
            "div",
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: false,
            },
            &mut store,
        );

        assert!(reactivated, "back() should return true on first close");
    }

    #[test]
    fn test_spawn_model_multiple_siblings_from_then() {
        let query = &Query::all("div", Save::none())
            .unwrap()
            .then(|div| {
                Ok([
                    div.first("h1", Save::all())?,
                    div.all("p", Save::all())?,
                    div.all("span", Save::all())?,
                ])
            })
            .unwrap()
            .build();

        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        selection.next(
            0,
            &XHtmlElement {
                name: "div",
                id: None,
                class: None,
                attributes: &[],
            },
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: false,
            },
            &mut store,
            &mut Vec::new(),
        );

        let spawned: Vec<_> = selection
            .cursors
            .iter()
            .filter(|c| c.is_moving() && c.position.selection != QuerySectionId(0))
            .collect();
        assert_eq!(
            spawned.len(),
            3,
            "Should spawn one MOVING cursor per .then() child"
        );

        let selections: Vec<_> = spawned.iter().map(|c| c.position.selection).collect();
        assert!(
            selections.contains(&QuerySectionId(1)),
            "h1 section missing"
        );
        assert!(selections.contains(&QuerySectionId(2)), "p section missing");
        assert!(
            selections.contains(&QuerySectionId(3)),
            "span section missing"
        );
    }

    #[test]
    fn test_spawn_model_pruning_at_scope_depth() {
        let query = Query::all("div", Save::none()).unwrap().build();
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(&query);

        let position = Position {
            selection: QuerySectionId(0),
            state: TransitionId(0),
        };

        selection.cursors = vec![
            ScopedCursor::new_root(ElementId(0), position),
            anchored_cursor(1, ElementId(10), position),
            anchored_cursor(2, ElementId(20), position),
            anchored_cursor(3, ElementId(30), position),
            anchored_cursor(0, ElementId(40), position),
        ];

        let _ = selection.back(
            0,
            "div",
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 2,
                self_closing: false,
            },
            &mut store,
        );

        let remaining_scopes: Vec<_> = selection
            .cursors
            .iter()
            .filter(|c| c.scope_depth != SENTINEL_SCOPE)
            .map(|c| c.scope_depth)
            .collect();

        assert_eq!(remaining_scopes.len(), 2);
        assert!(remaining_scopes.contains(&0));
        assert!(remaining_scopes.contains(&1));
        assert!(!remaining_scopes.contains(&2));
        assert!(!remaining_scopes.contains(&3));
    }

    // ── Cursor canonicalization regression tests (Commit 1) ───────────────
    // These document expected cursor invariants before executor refactor.
    // Cursor-invariant assertions are expected to FAIL until the fix lands.

    /// Test A: child-depth regression (`main > div p`).
    /// After the direct-child div matches, descendant cursors must keep main
    /// depth as match base; nested div must not spawn redundant p cursors.
    #[test]
    fn test_a_child_depth_regression_main_div_p() {
        let query = Query::all("main > div p", Save::all()).unwrap().build();
        let query = &query;
        let p_state = terminal_state(query);

        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        // depth 0: <main>
        selection.next(0, &elem("main"), &doc_pos(0), &mut store, &mut save_hits);

        // depth 1: direct-child <div>
        selection.next(0, &elem("div"), &doc_pos(1), &mut store, &mut save_hits);

        let main_depth = 0u16;
        let div_depth = 1u16;
        let output_parent = ElementId::default();

        // Child-div obligation keeps main as match base after matching a direct child.
        let child_div_state = TransitionId(1); // `main > div` transition after `main`
        let child_div_cursors: Vec<_> = selection
            .cursors
            .iter()
            .filter(|c| c.is_moving() && c.position.state == child_div_state)
            .collect();
        assert!(
            !child_div_cursors.is_empty(),
            "Expected child-div cursors after direct-child div match"
        );
        for cursor in &child_div_cursors {
            assert_eq!(
                cursor.match_base_depth(),
                main_depth,
                "child-div cursor match base must remain main depth ({main_depth})"
            );
        }

        // Descendant-p continuation is rooted at the matched div depth.
        let p_cursors: Vec<_> = selection
            .cursors
            .iter()
            .filter(|c| c.is_moving() && !c.end() && c.position.state == p_state)
            .collect();
        assert!(
            !p_cursors.is_empty(),
            "Expected live p-position cursors after direct-child div match"
        );
        for cursor in &p_cursors {
            assert_eq!(
                cursor.match_base_depth(),
                div_depth,
                "p cursor match base must be matched div depth ({div_depth})"
            );
        }

        // depth 2: nested <div> — must not fork another p obligation.
        selection.next(0, &elem("div"), &doc_pos(2), &mut store, &mut save_hits);

        assert_eq!(
            live_moving_cursors_at(&selection, p_state, output_parent),
            1,
            "CURSOR INVARIANT (expected to fail until admission): nested div must not spawn duplicate live p cursors for same parent+position"
        );

        // depth 3: <p>
        selection.next(0, &elem("p"), &doc_pos(3), &mut store, &mut save_hits);

        let ps: Vec<_> = store.get("main > div p").unwrap().collect();
        assert_eq!(ps.len(), 1, "Expected exactly one p result");
    }

    /// Test B: sibling direct children still work (`main > div p`).
    #[test]
    fn test_b_sibling_direct_children_main_div_p() {
        let html = "<main><div><p>A</p></div><div><p>B</p></div></main>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("main > div p", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let ps: Vec<_> = store.get("main > div p").unwrap().collect();
        assert_eq!(ps.len(), 2, "Both sibling p elements must match");
    }

    /// Test C: overlapping nested prefixes — deeper p candidate rejected.
    #[test]
    fn test_c_overlapping_nested_prefixes() {
        let query = Query::all("main > div p", Save::all()).unwrap().build();
        let query = &query;
        let p_state = terminal_state(query);

        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(0, &elem("main"), &doc_pos(0), &mut store, &mut save_hits);
        selection.next(0, &elem("div"), &doc_pos(1), &mut store, &mut save_hits);
        selection.next(0, &elem("main"), &doc_pos(2), &mut store, &mut save_hits);
        selection.next(0, &elem("div"), &doc_pos(3), &mut store, &mut save_hits);

        let output_parent = ElementId::default();
        assert_eq!(
            live_moving_cursors_at(&selection, p_state, output_parent),
            1,
            "CURSOR INVARIANT (expected to fail): overlapping nested main>div prefixes must not leave two live p cursors with same parent+position"
        );

        selection.next(0, &elem("p"), &doc_pos(4), &mut store, &mut save_hits);

        let ps: Vec<_> = store.get("main > div p").unwrap().collect();
        assert_eq!(ps.len(), 1, "Expected exactly one p result");
    }

    /// Test D: repeated child-prefix overlap (`div > div p`).
    #[test]
    fn test_d_repeated_child_prefix_overlap() {
        let query = Query::all("div > div p", Save::all()).unwrap().build();
        let query = &query;
        let p_state = terminal_state(query);

        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(0, &elem("div"), &doc_pos(0), &mut store, &mut save_hits);
        selection.next(0, &elem("div"), &doc_pos(1), &mut store, &mut save_hits);

        let output_parent = ElementId::default();
        let p_count_after_second_div =
            live_moving_cursors_at(&selection, p_state, output_parent);
        assert_eq!(
            p_count_after_second_div, 1,
            "CURSOR INVARIANT (expected to fail): only one live p cursor after first div>div match"
        );

        // Third nested div must not rebased-match as another direct child.
        selection.next(0, &elem("div"), &doc_pos(2), &mut store, &mut save_hits);
        assert_eq!(
            live_moving_cursors_at(&selection, p_state, output_parent),
            1,
            "CURSOR INVARIANT (expected to fail): innermost div must not spawn another p cursor"
        );

        selection.next(0, &elem("p"), &doc_pos(3), &mut store, &mut save_hits);

        let ps: Vec<_> = store.get("div > div p").unwrap().collect();
        assert_eq!(ps.len(), 1, "Expected exactly one p result");
    }

    /// Test E: child anchors not over-pruned (`div > p`).
    #[test]
    fn test_e_child_anchors_not_over_pruned() {
        let html = "<div><p>Outer</p><div><p>Inner</p></div></div>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("div > p", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let ps: Vec<_> = store.get("div > p").unwrap().collect();
        assert_eq!(ps.len(), 2, "Both direct-child p elements must match");
    }

    /// Test F: terminal `all()` nested matches (`div`).
    #[test]
    fn test_f_terminal_all_nested_matches() {
        let html = "<div><div><div></div></div></div>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("div", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let divs: Vec<_> = store.get("div").unwrap().collect();
        assert_eq!(divs.len(), 3, "All three nested divs must match");
    }

    /// Test G: `.then()` scopes are not globally canonicalized.
    #[test]
    fn test_g_then_scopes_not_globally_canonicalized() {
        let html = "<div><div><div><p>Hello</p></div></div></div>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("div", Save::all())
            .unwrap()
            .then(|div| Ok([div.all("p", Save::all())?]))
            .unwrap()
            .build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let divs: Vec<_> = store.get("div").unwrap().collect();
        assert_eq!(divs.len(), 3);

        let mut parents_with_p = 0;
        let mut p_refs = Vec::new();
        for div in &divs {
            let ps: Vec<_> = div.get(&store, "p").unwrap().collect();
            if !ps.is_empty() {
                parents_with_p += 1;
                p_refs.push(ps[0] as *const _);
            }
        }
        assert_eq!(
            parents_with_p, 3,
            "Each matching div scope must retain its own p child (not globally deduped)"
        );
        assert_eq!(p_refs.len(), 3);
        assert!(
            p_refs.windows(2).all(|w| !std::ptr::eq(w[0], w[1])),
            "Distinct .then() parents must keep separate saved p elements"
        );
    }

    /// Test H: `first()` behavior unchanged for flat and `.then()` child queries.
    #[test]
    fn test_h_first_behavior_flat_and_then() {
        let html = "<div><p>A</p><p>B</p></div><div><p>C</p><p>D</p></div>";
        let reader = &mut Reader::new(html);
        let query = &[Query::first("div p", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let ps: Vec<_> = store.get("div p").unwrap().collect();
        assert_eq!(ps.len(), 1, "first('div p') must match only one p globally");

        let html2 = "<div><p>A</p><p>B</p></div>";
        let reader2 = &mut Reader::new(html2);
        let query2 = &[Query::all("div", Save::none())
            .unwrap()
            .then(|div| Ok([div.first("p", Save::all())?]))
            .unwrap()
            .build()];
        let manager2 = QueryMultiplexer::new(query2);
        let mut parser2 = XHtmlParser::new(manager2);
        while parser2.next(reader2) {}
        let store2 = parser2.matches();

        let divs: Vec<_> = store2.get("div").unwrap().collect();
        assert_eq!(divs.len(), 1);
        let child_ps: Vec<_> = divs[0].get(&store2, "p").unwrap().collect();
        assert_eq!(child_ps.len(), 1, "then first('p') must match one p per parent");

        // Early exit for flat first() is covered by the single global result above;
        // verify first('div') still triggers early_exit on the executor directly.
        let div_only = Query::first("div", Save::all()).unwrap().build();
        let mut selection = QueryExecutor::new(&div_only);
        let mut store3 = Store::default();
        selection.next(
            0,
            &elem("div"),
            &doc_pos(0),
            &mut store3,
            &mut Vec::new(),
        );
        assert!(
            selection.early_exit(),
            "first('div') must set early_exit after terminal match"
        );
    }

    /// Test I: malformed / implicit-close / self-closing — cursor set stays valid.
    #[test]
    fn test_i_malformed_implicit_close_self_closing_cursors_valid() {
        let query = Query::all("div > div p", Save::all()).unwrap().build();
        let query = &query;
        let p_state = terminal_state(query);

        // Implicit <li> close variant.
        let html_li = "<ul><li><div><div><p>X</p></div></div><li>Y</ul>";
        let reader = &mut Reader::new(html_li);
        let queries = [query.clone()];
        let manager = QueryMultiplexer::new(&queries);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();
        let ps: Vec<_> = store.get("div > div p").unwrap().collect();
        assert_eq!(ps.len(), 1, "Implicit close must still yield one p match");

        // Mismatched close + self-closing: drive manually to inspect cursors.
        let mut store2 = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(0, &elem("div"), &doc_pos(0), &mut store2, &mut save_hits);
        selection.next(0, &elem("div"), &doc_pos(1), &mut store2, &mut save_hits);
        selection.next(
            0,
            &XHtmlElement {
                name: "br",
                id: None,
                class: None,
                attributes: &[],
            },
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 2,
                self_closing: true,
            },
            &mut store2,
            &mut save_hits,
        );
        selection.next(0, &elem("p"), &doc_pos(3), &mut store2, &mut save_hits);

        let live_p: Vec<_> = selection
            .cursors
            .iter()
            .filter(|c| c.is_moving() && c.position.state == p_state)
            .collect();
        assert!(
            live_p.iter().all(|c| c.end() || c.match_base_depth() <= 3),
            "CURSOR INVARIANT: live p cursors must have sane depth after self-closing element"
        );

        let ps2: Vec<_> = store2.get("div > div p").unwrap().collect();
        assert_eq!(ps2.len(), 1);
    }
}
