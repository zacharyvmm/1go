use super::attribute_interest::AttributeInterest;
use super::cursor::{SENTINEL_SCOPE, ScopedCursor};
use super::multiplexer::{DocumentPosition, SaveHit};
use crate::debug::ScopedCursorReason;
#[cfg(any(debug_assertions, test))]
use crate::debug::{CursorSuppressionReason, CursorTraceKind, TraceEvent, TransitionRejectReason};
use crate::store::ElementId;
use crate::store::Store;
use crate::{
    Combinator, Position, QuerySectionId, QuerySpec, SelectionKind, TransitionId, XHtmlElement,
};
#[cfg(any(debug_assertions, test))]
use smallvec::SmallVec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// Sentinel-aware broadest-scope combination for First ownership.
    ///
    /// `SENTINEL_SCOPE` is numerically max, but semantically broader than every
    /// ordinary element scope, so ordinary numeric `min` is incorrect.
    #[inline(always)]
    fn broader_scope_depth(
        current: super::DepthSize,
        candidate: super::DepthSize,
    ) -> super::DepthSize {
        if current == SENTINEL_SCOPE || candidate == SENTINEL_SCOPE {
            SENTINEL_SCOPE
        } else {
            current.min(candidate)
        }
    }

    fn claim_first_scope(
        &mut self,
        section: QuerySectionId,
        output_parent: ElementId,
        selected_cursor_index: usize,
        selected_depth: super::DepthSize,
    ) {
        debug_assert!(
            matches!(
                self.query.get_section_selection_kind(section),
                SelectionKind::First
            ),
            "claim_first_scope requires SelectionKind::First"
        );

        debug_assert_eq!(
            self.cursors[selected_cursor_index].position.selection, section,
            "selected cursor must belong to claimed First section"
        );
        debug_assert_eq!(
            self.cursors[selected_cursor_index].parent, output_parent,
            "selected cursor parent must match First output parent"
        );
        debug_assert!(
            self.cursors[selected_cursor_index].is_moving(),
            "First winner must be a moving cursor"
        );
        debug_assert!(
            self.cursors[selected_cursor_index].is_active(),
            "First winner must be active before claim"
        );

        debug_assert!(
            !self.cursors.iter().any(|cursor| {
                cursor.position.selection == section
                    && cursor.parent == output_parent
                    && cursor.is_first_winner()
            }),
            "First scope claimed twice for section+output_parent"
        );

        // Rebind the winner only after its original scope contributes to ownership.
        let mut ownership_scope_depth = self.cursors[selected_cursor_index].scope_depth;

        for (index, cursor) in self.cursors.iter_mut().enumerate() {
            if cursor.position.selection != section || cursor.parent != output_parent {
                continue;
            }

            ownership_scope_depth =
                Self::broader_scope_depth(ownership_scope_depth, cursor.scope_depth);

            if index != selected_cursor_index {
                cursor.cancel_complete();
            }
        }

        debug_assert!(
            ownership_scope_depth == SENTINEL_SCOPE || ownership_scope_depth <= selected_depth,
            "First ownership scope must contain selected element"
        );

        self.cursors[selected_cursor_index]
            .select_first_until_close(selected_depth, ownership_scope_depth);
    }

    pub fn query(&self) -> &Q {
        self.query
    }

    /// Return whether any currently active cursor may need attributes for a
    /// tag with `name`.
    ///
    /// This deliberately ignores depth guards: doing a little extra parsing
    /// is safe, while skipping attributes needed by a viable transition is
    /// not. Save points always need the complete element because attributes
    /// are part of the stored result even when the selector itself is tag-only.
    pub(crate) fn extend_attribute_interest_for(
        &self,
        name: &str,
        interest: &mut AttributeInterest<'query>,
    ) {
        for cursor in &self.cursors {
            if !cursor.is_active() {
                continue;
            }

            let position = cursor.position;
            let predicate = &self.query.get_transition(position.state).predicate;
            if !predicate.matches_name(name) {
                continue;
            }

            if self.query.is_save_point(&position)
                && self.query.get_selection(position.selection).save.attributes
            {
                interest.require_all();
                return;
            }

            interest.add_predicate(predicate);
        }
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

    #[cfg(any(debug_assertions, test))]
    fn trace_cursor_suppressed(
        store: &mut Store<'html, 'query>,
        runner_index: usize,
        candidate: &ScopedCursor,
        existing: &ScopedCursor,
        reason: CursorSuppressionReason,
    ) {
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

    #[inline]
    fn first_scope_is_claimed(&self, candidate: &ScopedCursor) -> bool {
        let section = candidate.position.selection;
        if !matches!(
            self.query.get_section_selection_kind(section),
            SelectionKind::First
        ) {
            return false;
        }

        self.cursors.iter().rev().any(|cursor| {
            cursor.is_first_winner()
                && cursor.position.selection == section
                && cursor.parent == candidate.parent
        })
    }

    fn finish_push_cursor(
        &mut self,
        candidate: ScopedCursor,
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] runner_index: usize,
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] store: &mut Store<
            'html,
            'query,
        >,
        create_reason: Option<ScopedCursorReason>,
    ) -> SpawnOutcome {
        #[cfg(any(debug_assertions, test))]
        if let Some(reason) = create_reason {
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
        #[cfg(not(any(debug_assertions, test)))]
        let _ = create_reason;

        self.cursors.push(candidate);
        SpawnOutcome::Inserted
    }

    /// Admit a descendant obligation unless a shallower equivalent is live.
    ///
    /// Live cursors cannot be deeper than a new candidate: candidates use the
    /// current document depth, and deeper scopes are pruned before parsing
    /// resumes at a shallower depth.
    fn try_push_descendant(
        &mut self,
        candidate: ScopedCursor,
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] runner_index: usize,
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] store: &mut Store<
            'html,
            'query,
        >,
        create_reason: Option<ScopedCursorReason>,
    ) -> SpawnOutcome {
        let candidate_base = candidate.match_base_depth();
        for existing in self.cursors.iter().rev() {
            if existing.end() {
                continue;
            }
            if existing.parent != candidate.parent || existing.position != candidate.position {
                continue;
            }
            let existing_base = existing.match_base_depth();
            if existing_base <= candidate_base {
                #[cfg(any(debug_assertions, test))]
                {
                    let reason = if existing_base == candidate_base {
                        CursorSuppressionReason::ExactDuplicate
                    } else {
                        CursorSuppressionReason::DescendantDominated
                    };
                    Self::trace_cursor_suppressed(
                        store,
                        runner_index,
                        &candidate,
                        existing,
                        reason,
                    );
                }
                return SpawnOutcome::Dominated;
            }
            debug_assert!(false, "shallower descendant candidate while deeper exists");
        }
        self.finish_push_cursor(candidate, runner_index, store, create_reason)
    }

    /// Admit a child obligation unless the exact obligation is already live.
    fn try_push_child(
        &mut self,
        candidate: ScopedCursor,
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] runner_index: usize,
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] store: &mut Store<
            'html,
            'query,
        >,
        create_reason: Option<ScopedCursorReason>,
    ) -> SpawnOutcome {
        let candidate_base = candidate.match_base_depth();
        for existing in self.cursors.iter().rev() {
            if existing.end() {
                continue;
            }
            if existing.parent != candidate.parent || existing.position != candidate.position {
                continue;
            }
            if existing.match_base_depth() == candidate_base {
                #[cfg(any(debug_assertions, test))]
                Self::trace_cursor_suppressed(
                    store,
                    runner_index,
                    &candidate,
                    existing,
                    CursorSuppressionReason::ExactDuplicate,
                );
                return SpawnOutcome::Dominated;
            }
        }
        self.finish_push_cursor(candidate, runner_index, store, create_reason)
    }

    /// Admit a cursor after applying `First` ownership and combinator rules.
    fn try_push_cursor(
        &mut self,
        candidate: ScopedCursor,
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] runner_index: usize,
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] store: &mut Store<
            'html,
            'query,
        >,
        create_reason: Option<ScopedCursorReason>,
    ) -> SpawnOutcome {
        if self.first_scope_is_claimed(&candidate) {
            #[cfg(any(debug_assertions, test))]
            {
                if let Some(winner) = self.cursors.iter().rev().find(|cursor| {
                    cursor.position.selection == candidate.position.selection
                        && cursor.parent == candidate.parent
                        && cursor.is_first_winner()
                }) {
                    Self::trace_cursor_suppressed(
                        store,
                        runner_index,
                        &candidate,
                        winner,
                        CursorSuppressionReason::FirstScopeClaimed,
                    );
                }
            }
            return SpawnOutcome::Dominated;
        }

        let guard = &self.query.get_transition(candidate.position.state).guard;
        match guard {
            Combinator::Descendant => {
                self.try_push_descendant(candidate, runner_index, store, create_reason)
            }
            Combinator::Child => self.try_push_child(candidate, runner_index, store, create_reason),
            // Sibling obligations need stream identity before they can be deduplicated.
            Combinator::NextSibling | Combinator::SubsequentSibling => {
                debug_assert!(
                    false,
                    "sibling cursor admission requires sibling-stream identity"
                );
                self.finish_push_cursor(candidate, runner_index, store, create_reason)
            }
            Combinator::Namespace => {
                self.finish_push_cursor(candidate, runner_index, store, create_reason)
            }
        }
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

        #[cfg(any(debug_assertions, test))]
        let mut emitted_this_step: SmallVec<[(ElementId, QuerySectionId); 4]> = SmallVec::new();

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
            let section_kind = self.query.get_section_selection_kind(position.selection);
            let is_first = matches!(section_kind, SelectionKind::First);
            let self_closing = document_position.self_closing;
            let terminal_first = is_save_point && is_first;
            let terminal_all = is_save_point
                && matches!(section_kind, SelectionKind::All)
                && position.next_child(self.query).is_none();

            let spawned_positions;

            match &self.cursors[i].mode {
                super::cursor::CursorMode::Moving { .. } => {
                    // `save_element` advances the parent for children; restore it
                    // afterward so this cursor can still match later siblings.
                    let output_parent = self.cursors[i].parent;
                    let needs_anchor = self.query.needs_descendant_anchor(position);
                    let anchor_candidate =
                        needs_anchor.then(|| self.cursors[i].anchor_clone(depth));

                    let (saved_parent, saved_element) = if is_save_point {
                        #[cfg(any(debug_assertions, test))]
                        {
                            let save_parent = self.cursors[i].parent;
                            debug_assert!(
                                !emitted_this_step.iter().any(|(parent, section)| {
                                    *parent == save_parent && *section == position.selection
                                }),
                                "duplicate cursor emission for one physical element: \
                                 element={:?} depth={} parent={:?} section={:?} state={:?} cursor={} cursors={:?}",
                                element.name,
                                depth,
                                save_parent,
                                position.selection,
                                position.state,
                                i,
                                self.cursors,
                            );
                            emitted_this_step.push((save_parent, position.selection));
                        }
                        let hit = Self::save_element(
                            runner_index,
                            self.query,
                            store,
                            element.clone(),
                            &mut self.cursors[i],
                        );
                        let sp = self.cursors[i].parent;
                        self.cursors[i].parent = output_parent;
                        let saved = hit.element_id;
                        save_hits.push(hit);
                        (sp, Some(saved))
                    } else {
                        (output_parent, None)
                    };

                    // Update lifecycle before admitting the anchor so the
                    // matched source does not dominate it.
                    if !terminal_all {
                        if terminal_first {
                            debug_assert!(
                                saved_element.is_some(),
                                "terminal First must have a saved element"
                            );
                            self.claim_first_scope(position.selection, output_parent, i, depth);
                        } else if is_descendant || is_save_point {
                            self.cursors[i].block_until_close(depth);
                        }
                    }

                    if self_closing || terminal_all {
                        continue;
                    }

                    if !terminal_first && let Some(anchor) = anchor_candidate {
                        let _ = self.try_push_cursor(
                            anchor,
                            runner_index,
                            store,
                            Some(ScopedCursorReason::DescendantFork),
                        );
                    }

                    spawned_positions = self.cursors[i].next_positions(self.query);
                    for pos in &spawned_positions {
                        let continuation = ScopedCursor::new_moving(depth, saved_parent, *pos);
                        let _ = self.try_push_cursor(continuation, runner_index, store, None);
                    }
                }
                super::cursor::CursorMode::Anchored { .. } => {
                    // Anchors never advance to terminal First positions.
                    debug_assert!(
                        !(is_save_point && is_first),
                        "terminal First must be represented by a moving cursor"
                    );

                    if self_closing {
                        if is_save_point {
                            let output_parent = self.cursors[i].parent;
                            #[cfg(any(debug_assertions, test))]
                            {
                                debug_assert!(
                                    !emitted_this_step.iter().any(|(parent, section)| {
                                        *parent == output_parent && *section == position.selection
                                    }),
                                    "duplicate cursor emission for one physical element: \
                                     element={:?} depth={} parent={:?} section={:?} state={:?} cursor={} cursors={:?}",
                                    element.name,
                                    depth,
                                    output_parent,
                                    position.selection,
                                    position.state,
                                    i,
                                    self.cursors,
                                );
                                emitted_this_step.push((output_parent, position.selection));
                            }
                            let mut base = ScopedCursor::new_moving(
                                depth,
                                output_parent,
                                self.cursors[i].position,
                            );
                            let hit = Self::save_element(
                                runner_index,
                                self.query,
                                store,
                                element.clone(),
                                &mut base,
                            );
                            save_hits.push(hit);
                        }
                        continue;
                    }

                    spawned_positions = self.cursors[i].next_positions(self.query);

                    let saved_parent = if is_save_point {
                        let save_parent = self.cursors[i].parent;
                        #[cfg(any(debug_assertions, test))]
                        {
                            debug_assert!(
                                !emitted_this_step.iter().any(|(parent, section)| {
                                    *parent == save_parent && *section == position.selection
                                }),
                                "duplicate cursor emission for one physical element: \
                                 element={:?} depth={} parent={:?} section={:?} state={:?} cursor={} cursors={:?}",
                                element.name,
                                depth,
                                save_parent,
                                position.selection,
                                position.state,
                                i,
                                self.cursors,
                            );
                            emitted_this_step.push((save_parent, position.selection));
                        }
                        let mut base =
                            ScopedCursor::new_moving(depth, save_parent, self.cursors[i].position);
                        let hit = Self::save_element(
                            runner_index,
                            self.query,
                            store,
                            element.clone(),
                            &mut base,
                        );
                        save_hits.push(hit);
                        base.parent
                    } else {
                        self.cursors[i].parent
                    };

                    for pos in &spawned_positions {
                        let continuation = ScopedCursor::new_moving(depth, saved_parent, *pos);
                        let _ = self.try_push_cursor(continuation, runner_index, store, None);
                    }
                }
            }
        }
    }

    pub fn early_exit(&self) -> bool {
        let Some(exit_section) = self.query.exit_at_section_end() else {
            return false;
        };

        let closed_winner = self.cursors.iter().any(|cursor| {
            cursor.position.selection == exit_section
                && cursor.is_first_winner()
                && cursor.is_complete()
                && cursor.unwind_depth().is_none()
        });

        closed_winner && self.cursors.iter().all(|cursor| cursor.is_complete())
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
                    if cur.is_blocked() {
                        self.cursors[i].reactivate_after_close();
                    } else if cur.is_complete() {
                        self.cursors[i].complete_after_close();
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
                        debug_assert!(false, "active cursor should not have pending unwind");
                    }
                    significant_close = true;
                }
            } else if cur.is_moving() && cur.unwind_depth() == Some(close_depth) {
                if cur.is_blocked() {
                    self.cursors[i].reactivate_after_close();
                } else if cur.is_complete() {
                    self.cursors[i].complete_after_close();
                } else {
                    debug_assert!(false, "active cursor should not have pending unwind");
                }
                significant_close = true;
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
        Element, ElementId, Position, Query, QuerySectionId, Reader, Save, SelectionKind,
        TransitionId, XHtmlElement,
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
                !c.end() && c.is_moving() && c.position.state == state && c.parent == parent
            })
            .count()
    }

    fn parse<'html>(html: &'html str, queries: &'html [Query]) -> Store<'html, 'html> {
        let reader = &mut Reader::new(html);
        let manager = QueryMultiplexer::new(queries);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        parser.matches()
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
        assert!(!selection.cursors[0].end());

        store.text_content.push(&Reader::new("<div></div>"), 4);
        let _significant_close = selection.back(
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
            !selection.early_exit(),
            "early_exit must be false while selected element is only Matched"
        );
        assert!(
            selection.cursors[0].is_complete(),
            "selected cursor should be Complete awaiting close"
        );
        assert!(
            selection.cursors[0].unwind_depth().is_some(),
            "selected cursor should retain unwind depth until close"
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
        assert!(
            selection.early_exit(),
            "early_exit should be true after selected element closes"
        );
        assert!(
            selection.cursors[0].unwind_depth().is_none(),
            "selected cursor should clear unwind after close"
        );
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

    #[test]
    fn test_a_child_depth_regression_main_div_p() {
        let query = Query::all("main > div p", Save::all()).unwrap().build();
        let query = &query;
        let p_state = terminal_state(query);

        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(0, &elem("main"), &doc_pos(0), &mut store, &mut save_hits);
        selection.next(0, &elem("div"), &doc_pos(1), &mut store, &mut save_hits);

        let main_depth = 0u16;
        let div_depth = 1u16;
        let output_parent = ElementId::default();

        let child_div_state = TransitionId(1);
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

        selection.next(0, &elem("div"), &doc_pos(2), &mut store, &mut save_hits);

        assert_eq!(
            live_moving_cursors_at(&selection, p_state, output_parent),
            1,
            "nested div must not spawn duplicate live p cursors for same parent+position"
        );

        selection.next(0, &elem("p"), &doc_pos(3), &mut store, &mut save_hits);

        let ps: Vec<_> = store.get("main > div p").unwrap().collect();
        assert_eq!(ps.len(), 1, "Expected exactly one p result");
    }

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
            "overlapping nested main>div prefixes must not leave two live p cursors with same parent+position"
        );

        selection.next(0, &elem("p"), &doc_pos(4), &mut store, &mut save_hits);

        let ps: Vec<_> = store.get("main > div p").unwrap().collect();
        assert_eq!(ps.len(), 1, "Expected exactly one p result");
    }

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
        let p_count_after_second_div = live_moving_cursors_at(&selection, p_state, output_parent);
        assert_eq!(
            p_count_after_second_div, 1,
            "only one live p cursor after first div>div match"
        );

        selection.next(0, &elem("div"), &doc_pos(2), &mut store, &mut save_hits);
        assert_eq!(
            live_moving_cursors_at(&selection, p_state, output_parent),
            1,
            "innermost div must not spawn another p cursor"
        );

        selection.next(0, &elem("p"), &doc_pos(3), &mut store, &mut save_hits);

        let ps: Vec<_> = store.get("div > div p").unwrap().collect();
        assert_eq!(ps.len(), 1, "Expected exactly one p result");
    }

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
        assert_eq!(
            child_ps.len(),
            1,
            "then first('p') must match one p per parent"
        );

        let div_only = Query::first("div", Save::all()).unwrap().build();
        let mut selection = QueryExecutor::new(&div_only);
        let mut store3 = Store::default();
        selection.next(0, &elem("div"), &doc_pos(0), &mut store3, &mut Vec::new());
        assert!(
            !selection.early_exit(),
            "first('div') must not early-exit before selected close"
        );
        selection.back(0, "div", &doc_pos(0), &mut store3);
        assert!(
            selection.early_exit(),
            "first('div') must early-exit after selected close"
        );
    }

    #[test]
    fn test_i_malformed_implicit_close_self_closing_cursors_valid() {
        let query = Query::all("div > div p", Save::all()).unwrap().build();
        let query = &query;
        let p_state = terminal_state(query);

        let html_li = "<ul><li><div><div><p>X</p></div></div><li>Y</ul>";
        let reader = &mut Reader::new(html_li);
        let queries = [query.clone()];
        let manager = QueryMultiplexer::new(&queries);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();
        let ps: Vec<_> = store.get("div > div p").unwrap().collect();
        assert_eq!(ps.len(), 1, "Implicit close must still yield one p match");

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

    #[test]
    fn first_then_early_exit_after_root_close() {
        let query = Query::first("article", Save::all())
            .unwrap()
            .then(|a| Ok([a.all("p", Save::all())?]))
            .unwrap()
            .build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(0, &elem("article"), &doc_pos(0), &mut store, &mut save_hits);
        let root = &selection.cursors[0];
        assert!(root.end());
        assert_eq!(root.unwind_depth(), Some(0));
        assert!(!selection.early_exit());

        selection.next(0, &elem("p"), &doc_pos(1), &mut store, &mut save_hits);

        selection.back(0, "p", &doc_pos(1), &mut store);
        let root = &selection.cursors[0];
        assert!(root.end());
        assert_eq!(root.unwind_depth(), Some(0));
        assert!(!selection.early_exit());

        selection.back(0, "article", &doc_pos(0), &mut store);
        let root = &selection.cursors[0];
        assert!(root.end());
        assert_eq!(root.unwind_depth(), None);
        assert!(selection.early_exit());
    }

    #[test]
    fn first_child_selector_skips_failed_prefix() {
        let html = r#"
            <div><span>no match</span></div>
            <div><p id="hit">match</p></div>
        "#;

        let queries = &[Query::first("div > p", Save::all()).unwrap().build()];

        let store = parse(html, queries);

        let hits: Vec<_> = store.get("div > p").unwrap().collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, Some("hit"));
    }

    #[test]
    fn first_descendant_selector_skips_failed_prefix() {
        let html = r#"
            <div><span>no match</span></div>
            <div><section><p id="hit">match</p></section></div>
        "#;

        let queries = &[Query::first("div p", Save::all()).unwrap().build()];

        let store = parse(html, queries);

        let hits: Vec<_> = store.get("div p").unwrap().collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, Some("hit"));
    }

    #[test]
    fn first_mixed_child_descendant_selector_skips_failed_prefix() {
        let html = r#"
            <main><section><span>no match</span></section></main>
            <main><section><p id="hit">match</p></section></main>
        "#;

        let queries = &[Query::first("main > section p", Save::all())
            .unwrap()
            .build()];

        let store = parse(html, queries);

        let hits: Vec<_> = store.get("main > section p").unwrap().collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, Some("hit"));
    }

    #[test]
    fn first_then_child_selector_skips_failed_prefix() {
        let html = r#"
            <article>
                <div><span>no</span></div>
                <div><p id="hit">yes</p></div>
            </article>
        "#;

        let queries = &[Query::all("article", Save::all())
            .unwrap()
            .then(|article| Ok([article.first("div > p", Save::all())?]))
            .unwrap()
            .build()];

        let store = parse(html, queries);

        let articles: Vec<_> = store.get("article").unwrap().collect();
        assert_eq!(articles.len(), 1);
        let hits: Vec<_> = articles[0].get(&store, "div > p").unwrap().collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, Some("hit"));
    }

    #[test]
    fn first_void_element_matches_once() {
        for tag in ["br", "img"] {
            let html = format!("<{tag}><{tag}>");
            let reader = &mut Reader::new(&html);
            let query = &[Query::first(tag, Save::all()).unwrap().build()];
            let manager = QueryMultiplexer::new(query);
            let mut parser = XHtmlParser::new(manager);
            while parser.next(reader) {}
            let store = parser.matches();
            let hits: Vec<_> = store.get(tag).unwrap().collect();
            assert_eq!(hits.len(), 1, "first('{tag}') must match once");
        }
    }

    #[test]
    fn all_void_elements_match_all() {
        let html = "<br><br>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("br", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();
        let hits: Vec<_> = store.get("br").unwrap().collect();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn terminal_all_has_no_unwind_after_match() {
        let query = Query::all("div", Save::all()).unwrap().build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(0, &elem("div"), &doc_pos(0), &mut store, &mut save_hits);
        let root = &selection.cursors[0];
        assert!(!root.end());
        assert_eq!(root.unwind_depth(), None);

        let html = "<div><div><div></div></div></div>";
        let reader = &mut Reader::new(html);
        let nested = &[Query::all("div", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(nested);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();
        assert_eq!(store.get("div").unwrap().count(), 3);

        let sibling_html = "<main><section></section><section></section></main>";
        let reader = &mut Reader::new(sibling_html);
        let sibling_q = &[Query::all("main > section", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(sibling_q);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();
        assert_eq!(store.get("main > section").unwrap().count(), 2);
    }

    #[test]
    fn self_closing_match_prunes_scoped_state_immediately() {
        let html = "<div><br><p>x</p></div><div><br></div>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("div br", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();
        assert_eq!(store.get("div br").unwrap().count(), 2);

        let query = Query::first("div", Save::none())
            .unwrap()
            .then(|div| Ok([div.first("br", Save::all())?]))
            .unwrap()
            .build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(0, &elem("div"), &doc_pos(0), &mut store, &mut save_hits);
        save_hits.clear();
        selection.next(
            0,
            &elem("br"),
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 1,
                self_closing: true,
            },
            &mut store,
            &mut save_hits,
        );
        assert_eq!(save_hits.len(), 1, "void child First must save once");
        assert!(
            selection
                .cursors
                .iter()
                .any(|c| c.position.selection == QuerySectionId(1) && c.unwind_depth() == Some(1)),
            "void First should await synthetic close at match depth"
        );
        selection.back(
            0,
            "br",
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 1,
                self_closing: true,
            },
            &mut store,
        );
        assert!(
            selection
                .cursors
                .iter()
                .all(|c| c.unwind_depth() != Some(1)),
            "synthetic void close must clear br unwind (root may still await parent close)"
        );
        assert_eq!(
            store.elements.iter().filter(|e| e.name == "br").count(),
            1,
            "void First must not rematch after synthetic close"
        );
    }

    #[test]
    fn self_closing_parent_with_then_no_child_results() {
        let query = Query::first("br", Save::all())
            .unwrap()
            .then(|br| Ok([br.all("span", Save::all())?]))
            .unwrap()
            .build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(
            0,
            &elem("br"),
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: true,
            },
            &mut store,
            &mut save_hits,
        );
        assert_eq!(save_hits.len(), 1, "void parent must be saved once");
        assert!(
            selection
                .cursors
                .iter()
                .all(|c| c.position.selection != QuerySectionId(1)),
            "self-closing parent must not spawn child continuations"
        );

        selection.back(
            0,
            "br",
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: true,
            },
            &mut store,
        );
        assert!(
            selection.early_exit(),
            "first void parent must early-exit after synthetic close"
        );
        assert_eq!(store.elements.len(), 1);
    }

    #[test]
    fn void_intermediate_prefix_reactivates_after_synthetic_close() {
        let query = Query::all("div br span", Save::all()).unwrap().build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(0, &elem("div"), &doc_pos(0), &mut store, &mut save_hits);
        selection.next(
            0,
            &elem("br"),
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 1,
                self_closing: true,
            },
            &mut store,
            &mut save_hits,
        );
        assert!(
            selection
                .cursors
                .iter()
                .any(|c| c.is_blocked() && c.unwind_depth() == Some(1)),
            "intermediate br prefix must block until synthetic close"
        );
        assert!(
            selection.cursors.iter().all(|c| !c.is_complete()),
            "void cannot satisfy span suffix; prefix must not stay Complete"
        );
        selection.back(
            0,
            "br",
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 1,
                self_closing: true,
            },
            &mut store,
        );
        assert!(
            selection.cursors.iter().any(|c| c.is_active()),
            "blocked void prefix must reactivate after synthetic close"
        );
        assert_eq!(
            store.elements.len(),
            0,
            "self-closing br cannot host span descendants"
        );
    }

    #[test]
    fn first_compound_void_then_early_exit_at_synthetic_close() {
        let filler = "<span>filler</span>".repeat(100);
        let html = format!("<div><br>{filler}</div><div>tail</div>");
        let html_len = html.len();

        let query = Query::first("div br", Save::all())
            .unwrap()
            .then(|br| Ok([br.all("span", Save::all())?]))
            .unwrap()
            .build();
        let queries = [query];
        let reader = &mut Reader::new(&html);
        let manager = QueryMultiplexer::new(&queries);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        assert!(
            reader.get_position() < html_len,
            "compound void First+.then() must stop at br synthetic close, not </div> or tail"
        );
        let store = parser.matches();
        let brs: Vec<_> = store.get("div br").unwrap().collect();
        assert_eq!(brs.len(), 1);
        let span_count = brs[0].get(&store, "span").map(|it| it.count()).unwrap_or(0);
        assert_eq!(
            span_count, 0,
            "void br cannot contain span descendants; sibling filler must not attach"
        );
    }

    #[test]
    fn first_compound_then_early_exit_after_selected_close() {
        let filler = "<div>filler</div>".repeat(100);
        let html = format!("<article><p>hit<span>inner</span></p>tail</article>{filler}");
        let html_len = html.len();

        let query = Query::first("article p", Save::all())
            .unwrap()
            .then(|p| Ok([p.all("span", Save::all())?]))
            .unwrap()
            .build();
        let queries = [query];
        let reader = &mut Reader::new(&html);
        let manager = QueryMultiplexer::new(&queries);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        assert!(
            reader.get_position() < html_len,
            "compound First+.then() must stop before article filler and trailing filler divs"
        );
        let store = parser.matches();
        let ps: Vec<_> = store.get("article p").unwrap().collect();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].inner_html, Some("hit<span>inner</span>"));
        let spans: Vec<_> = ps[0].get(&store, "span").unwrap().collect();
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn first_compound_early_exit_after_selected_close_without_then() {
        let filler = "<div>filler</div>".repeat(100);
        let html = format!("<article><p>hit</p>tail</article>{filler}");
        let html_len = html.len();

        let query = Query::first("article p", Save::all()).unwrap().build();
        let queries = [query];
        let reader = &mut Reader::new(&html);
        let manager = QueryMultiplexer::new(&queries);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        assert!(
            reader.get_position() < html_len,
            "compound First must stop after </p>, before article tail and filler divs"
        );
        let store = parser.matches();
        let ps: Vec<_> = store.get("article p").unwrap().collect();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].inner_html, Some("hit"));
    }

    #[test]
    fn parser_early_stop_before_filler_content() {
        let filler = "<div>filler</div>".repeat(100);
        let article_html = format!("<article><p>hit</p></article>{filler}");
        let article_len = article_html.len();

        let query = Query::first("article", Save::all())
            .unwrap()
            .then(|a| Ok([a.all("p", Save::all())?]))
            .unwrap()
            .build();
        let queries = [query];
        let reader = &mut Reader::new(&article_html);
        let manager = QueryMultiplexer::new(&queries);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        assert!(
            reader.get_position() < article_len,
            "early exit must stop before filler divs"
        );
        let store = parser.matches();
        let articles: Vec<_> = store.get("article").unwrap().collect();
        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].inner_html, Some("<p>hit</p>"));
        let ps: Vec<_> = articles[0].get(&store, "p").unwrap().collect();
        assert_eq!(ps.len(), 1);

        let flat_html = format!("<article>content</article>{filler}");
        let flat_len = flat_html.len();
        let flat_query = Query::first("article", Save::all()).unwrap().build();
        let flat_queries = [flat_query];
        let reader2 = &mut Reader::new(&flat_html);
        let manager2 = QueryMultiplexer::new(&flat_queries);
        let mut parser2 = XHtmlParser::new(manager2);
        while parser2.next(reader2) {}
        assert!(reader2.get_position() < flat_len);

        let br_html = format!("<br>{filler}");
        let br_len = br_html.len();
        let br_query = Query::first("br", Save::all()).unwrap().build();
        let br_queries = [br_query];
        let reader3 = &mut Reader::new(&br_html);
        let manager3 = QueryMultiplexer::new(&br_queries);
        let mut parser3 = XHtmlParser::new(manager3);
        while parser3.next(reader3) {}
        assert!(reader3.get_position() < br_len);
    }

    #[test]
    fn first_scope_cancels_nested_alternate_prefixes() {
        let html = r#"
            <div>
                <span id="first">
                    <div>
                        <span id="second"></span>
                    </div>
                </span>
            </div>
        "#;

        let queries = &[Query::first("div > span", Save::all()).unwrap().build()];
        let store = parse(html, queries);

        let spans: Vec<_> = store.get("div > span").unwrap().collect();
        assert_eq!(
            spans.len(),
            1,
            "First scope must cancel nested alternate prefixes"
        );
        assert_eq!(spans[0].id, Some("first"));
    }

    #[test]
    fn then_first_scope_selects_once_per_parent() {
        let html = r#"
            <article>
                <div><p id="first"></p></div>
                <div><p id="second"></p></div>
            </article>
        "#;

        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| Ok([article.first("div > p", Save::all())?]))
            .unwrap()
            .build();
        let queries = [query];
        let store = parse(html, &queries);

        let articles: Vec<_> = store.get("article").unwrap().collect();
        assert_eq!(articles.len(), 1);
        let ps: Vec<_> = articles[0].get(&store, "div > p").unwrap().collect();
        assert_eq!(
            ps.len(),
            1,
            "then first('div > p') must select once per parent"
        );
        assert_eq!(ps[0].id, Some("first"));
    }

    #[test]
    fn then_first_scopes_independent_per_parent() {
        let html = r#"
            <article id="a">
                <div><p id="a-first"></p></div>
                <div><p id="a-second"></p></div>
            </article>
            <article id="b">
                <div><p id="b-first"></p></div>
                <div><p id="b-second"></p></div>
            </article>
        "#;

        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| Ok([article.first("div > p", Save::all())?]))
            .unwrap()
            .build();
        let queries = [query];
        let store = parse(html, &queries);

        let articles: Vec<_> = store.get("article").unwrap().collect();
        assert_eq!(articles.len(), 2);

        let article_a = articles
            .iter()
            .find(|a| a.id == Some("a"))
            .expect("article a");
        let article_b = articles
            .iter()
            .find(|a| a.id == Some("b"))
            .expect("article b");

        let a_ps: Vec<_> = article_a.get(&store, "div > p").unwrap().collect();
        assert_eq!(a_ps.len(), 1);
        assert_eq!(a_ps[0].id, Some("a-first"));

        let b_ps: Vec<_> = article_b.get(&store, "div > p").unwrap().collect();
        assert_eq!(b_ps.len(), 1);
        assert_eq!(b_ps[0].id, Some("b-first"));
    }

    #[test]
    fn then_first_scope_preserves_sibling_all_section() {
        let html = r#"
            <article>
                <div><p id="first"></p><a id="a1"></a></div>
                <div><p id="second"></p><a id="a2"></a></div>
                <a id="a3"></a>
            </article>
        "#;

        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| {
                Ok([
                    article.first("div > p", Save::all())?,
                    article.all("a", Save::all())?,
                ])
            })
            .unwrap()
            .build();
        let queries = [query];
        let store = parse(html, &queries);

        let articles: Vec<_> = store.get("article").unwrap().collect();
        assert_eq!(articles.len(), 1);

        let ps: Vec<_> = articles[0].get(&store, "div > p").unwrap().collect();
        assert_eq!(ps.len(), 1, "First scope must claim only one p per parent");
        assert_eq!(ps[0].id, Some("first"));

        let links: Vec<_> = articles[0].get(&store, "a").unwrap().collect();
        assert_eq!(
            links.len(),
            3,
            "claiming First scope must not cancel sibling all('a') section"
        );
    }

    #[test]
    fn first_scope_preserves_selected_then_children() {
        let html = r#"
            <div>
                <p id="first">
                    <span id="inner"></span>
                    <div>
                        <p id="second">
                            <span id="nested"></span>
                        </p>
                    </div>
                </p>
            </div>
        "#;

        let query = Query::first("div > p", Save::all())
            .unwrap()
            .then(|p| Ok([p.all("span", Save::all())?]))
            .unwrap()
            .build();
        let queries = [query];
        let store = parse(html, &queries);

        let ps: Vec<_> = store.get("div > p").unwrap().collect();
        assert_eq!(ps.len(), 1, "First scope must select only one p");
        assert_eq!(ps[0].id, Some("first"));

        let spans: Vec<_> = ps[0].get(&store, "span").unwrap().collect();
        assert_eq!(
            spans.len(),
            1,
            "selected p's then-child span section must survive scope cancellation"
        );
        assert_eq!(spans[0].id, Some("inner"));
    }

    #[test]
    fn all_retained_prefix_reactivates_same_transition() {
        let html = r#"
            <main>
                <div><span>no match</span></div>
                <div><p id="hit"></p></div>
            </main>
        "#;

        let queries = &[Query::all("main div > p", Save::all()).unwrap().build()];
        let store = parse(html, queries);

        let hits: Vec<_> = store.get("main div > p").unwrap().collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, Some("hit"));
    }

    #[test]
    fn first_retained_prefix_reactivates_same_transition() {
        let html = r#"
            <main>
                <div><span>no match</span></div>
                <div><p id="hit"></p></div>
            </main>
        "#;

        let queries = &[Query::first("main div > p", Save::all()).unwrap().build()];
        let store = parse(html, queries);

        let hits: Vec<_> = store.get("main div > p").unwrap().collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, Some("hit"));
    }

    #[test]
    fn longer_retained_prefix_reactivates_same_transition() {
        let html = r#"
            <main>
                <section>
                    <div><span>no</span></div>
                    <div><p id="hit"></p></div>
                </section>
            </main>
        "#;

        let queries = &[Query::all("main section div > p", Save::all())
            .unwrap()
            .build()];
        let store = parse(html, queries);

        let hits: Vec<_> = store.get("main section div > p").unwrap().collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, Some("hit"));
    }

    #[test]
    fn descendant_suffix_retained_prefix_survives_close() {
        let html = r#"
            <main>
                <div></div>
                <div><span id="hit"></span></div>
            </main>
        "#;

        let queries = &[Query::all("main div span", Save::all()).unwrap().build()];
        let store = parse(html, queries);

        let hits: Vec<_> = store.get("main div span").unwrap().collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, Some("hit"));
    }

    #[test]
    fn then_retained_prefix_reactivates_same_transition() {
        let html = r#"
            <article>
                <main>
                    <div><span>no</span></div>
                    <div><p id="hit"></p></div>
                </main>
            </article>
        "#;

        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| Ok([article.all("main div > p", Save::all())?]))
            .unwrap()
            .build();
        let queries = [query];
        let store = parse(html, &queries);

        let articles: Vec<_> = store.get("article").unwrap().collect();
        assert_eq!(articles.len(), 1);
        let hits: Vec<_> = articles[0].get(&store, "main div > p").unwrap().collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, Some("hit"));
    }

    #[test]
    fn blocked_cursor_reactivates_at_same_position() {
        let query = Query::all("main div > p", Save::all()).unwrap().build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(0, &elem("main"), &doc_pos(0), &mut store, &mut save_hits);
        selection.next(0, &elem("div"), &doc_pos(1), &mut store, &mut save_hits);

        let blocked_idx = selection
            .cursors
            .iter()
            .position(|c| {
                c.is_moving()
                    && c.is_blocked()
                    && c.unwind_depth() == Some(1)
                    && !query.is_save_point(&c.position)
            })
            .expect("blocked moving cursor awaiting div close (not save point)");
        let before_position = selection.cursors[blocked_idx].position;
        assert!(selection.cursors[blocked_idx].is_blocked());

        selection.back(0, "div", &doc_pos(1), &mut store);

        let reactivated = selection
            .cursors
            .iter()
            .find(|c| c.is_moving() && c.position == before_position)
            .expect("cursor at same position after div close");
        assert!(
            reactivated.is_active(),
            "blocked cursor must reactivate at the same transition"
        );
        assert_eq!(
            reactivated.unwind_depth(),
            None,
            "reactivated retained prefix must not retain unwind depth"
        );
    }

    #[test]
    fn first_single_transition_direct_child_selects_once() {
        let html = r#"
            <article>
                <h1 id="first"></h1>
                <h1 id="second"></h1>
            </article>
        "#;

        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| Ok([article.first("> h1", Save::all())?]))
            .unwrap()
            .build();
        let queries = [query];
        let store = parse(html, &queries);

        let articles: Vec<_> = store.get("article").unwrap().collect();
        assert_eq!(articles.len(), 1);
        let titles: Vec<_> = articles[0].get(&store, "> h1").unwrap().collect();
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].id, Some("first"));
    }

    #[test]
    fn first_single_transition_direct_child_scopes_independent_per_parent() {
        let html = r#"
            <article id="a">
                <h1 id="a-first"></h1>
                <h1 id="a-second"></h1>
            </article>
            <article id="b">
                <h1 id="b-first"></h1>
                <h1 id="b-second"></h1>
            </article>
        "#;

        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| Ok([article.first("> h1", Save::all())?]))
            .unwrap()
            .build();
        let queries = [query];
        let store = parse(html, &queries);

        let articles: Vec<_> = store.get("article").unwrap().collect();
        assert_eq!(articles.len(), 2);

        let article_a = articles
            .iter()
            .find(|a| a.id == Some("a"))
            .expect("article a");
        let article_b = articles
            .iter()
            .find(|a| a.id == Some("b"))
            .expect("article b");

        let a_titles: Vec<_> = article_a.get(&store, "> h1").unwrap().collect();
        assert_eq!(a_titles.len(), 1);
        assert_eq!(a_titles[0].id, Some("a-first"));

        let b_titles: Vec<_> = article_b.get(&store, "> h1").unwrap().collect();
        assert_eq!(b_titles.len(), 1);
        assert_eq!(b_titles[0].id, Some("b-first"));
    }

    #[test]
    fn first_single_transition_preserves_sibling_all_section_across_parents() {
        let html = r#"
            <article id="a">
                <h1 id="a-first"></h1>
                <h1 id="a-second"></h1>
                <p id="a-p1"></p>
                <p id="a-p2"></p>
            </article>
            <article id="b">
                <h1 id="b-first"></h1>
                <h1 id="b-second"></h1>
                <p id="b-p1"></p>
                <p id="b-p2"></p>
            </article>
        "#;

        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| {
                Ok([
                    article.first("> h1", Save::all())?,
                    article.all("> p", Save::all())?,
                ])
            })
            .unwrap()
            .build();
        let queries = [query];
        let store = parse(html, &queries);

        let articles: Vec<_> = store.get("article").unwrap().collect();
        assert_eq!(articles.len(), 2);

        for (article_id, expected_h1, expected_ps) in [
            ("a", "a-first", ["a-p1", "a-p2"]),
            ("b", "b-first", ["b-p1", "b-p2"]),
        ] {
            let article = articles
                .iter()
                .find(|a| a.id == Some(article_id))
                .unwrap_or_else(|| panic!("article {article_id}"));

            let titles: Vec<_> = article.get(&store, "> h1").unwrap().collect();
            assert_eq!(
                titles.len(),
                1,
                "article {article_id}: single-transition First must select one h1"
            );
            assert_eq!(titles[0].id, Some(expected_h1));

            let paragraphs: Vec<_> = article.get(&store, "> p").unwrap().collect();
            assert_eq!(
                paragraphs.len(),
                2,
                "article {article_id}: First fast path must not cancel sibling All"
            );
            assert_eq!(paragraphs[0].id, Some(expected_ps[0]));
            assert_eq!(paragraphs[1].id, Some(expected_ps[1]));
        }
    }

    #[test]
    fn first_single_transition_void_direct_child_scopes_independent_per_parent() {
        let html = r#"
            <article id="a"><br><br></article>
            <article id="b"><br><br></article>
        "#;

        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| Ok([article.first("> br", Save::all())?]))
            .unwrap()
            .build();
        let queries = [query];
        let store = parse(html, &queries);

        let articles: Vec<_> = store.get("article").unwrap().collect();
        assert_eq!(articles.len(), 2);

        for article_id in ["a", "b"] {
            let article = articles
                .iter()
                .find(|a| a.id == Some(article_id))
                .unwrap_or_else(|| panic!("article {article_id}"));
            let breaks: Vec<_> = article.get(&store, "> br").unwrap().collect();
            assert_eq!(
                breaks.len(),
                1,
                "article {article_id}: void direct-child First must select one br"
            );
        }
    }

    #[test]
    fn first_lifecycle_matched_then_complete_after_close() {
        let query = Query::first("div", Save::all()).unwrap().build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(0, &elem("div"), &doc_pos(0), &mut store, &mut save_hits);
        assert!(selection.cursors[0].is_first_winner());
        assert!(selection.cursors[0].is_complete());
        assert_eq!(selection.cursors[0].unwind_depth(), Some(0));
        assert!(!selection.early_exit());

        store.text_content.set_start(0);
        store.text_content.push(&Reader::new("<div>text</div>"), 4);
        selection.back(0, "div", &doc_pos(0), &mut store);
        assert!(selection.cursors[0].is_first_winner());
        assert!(selection.cursors[0].is_complete());
        assert_eq!(selection.cursors[0].unwind_depth(), None);
        assert!(selection.early_exit());

        let hits: Vec<_> = store.get("div").unwrap().collect();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn first_lifecycle_finalizes_content_through_parser() {
        let html = "<div>text</div><span>tail</span>";
        let query = Query::first("div", Save::all()).unwrap().build();
        let queries = [query];
        let store = parse(html, &queries);

        let hits: Vec<_> = store.get("div").unwrap().collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].inner_html, Some("text"));
        assert_eq!(hits[0].text_content(&store), Some("text"));
    }

    #[test]
    fn first_void_lifecycle_matched_then_complete_at_synthetic_close() {
        let query = Query::first("br", Save::all()).unwrap().build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(
            0,
            &elem("br"),
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: true,
            },
            &mut store,
            &mut save_hits,
        );
        assert!(selection.cursors[0].is_first_winner());
        assert!(selection.cursors[0].is_complete());
        assert_eq!(selection.cursors[0].unwind_depth(), Some(0));
        assert!(!selection.early_exit());

        selection.back(
            0,
            "br",
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
                self_closing: true,
            },
            &mut store,
        );
        assert!(selection.cursors[0].is_first_winner());
        assert!(selection.cursors[0].is_complete());
        assert_eq!(selection.cursors[0].unwind_depth(), None);
        assert!(selection.early_exit());
    }

    #[test]
    fn terminal_first_positions_never_require_descendant_anchor() {
        let nested = Query::all("article", Save::all())
            .unwrap()
            .then(|article| Ok([article.first("div > p", Save::all())?]))
            .unwrap()
            .build();
        let queries = [
            Query::first("p", Save::all()).unwrap().build(),
            Query::first("div p", Save::all()).unwrap().build(),
            Query::first("div > p", Save::all()).unwrap().build(),
            Query::first("main div > p", Save::all()).unwrap().build(),
            Query::first("br", Save::all()).unwrap().build(),
            Query::first("div br", Save::all()).unwrap().build(),
            nested,
        ];

        for query in &queries {
            for (section_index, section) in query.queries().iter().enumerate() {
                if !matches!(section.kind, SelectionKind::First) {
                    continue;
                }

                let position = Position {
                    selection: QuerySectionId(section_index),
                    state: TransitionId(section.range.end.index() - 1),
                };

                assert!(query.is_save_point(&position));
                assert!(
                    !query.needs_descendant_anchor(position),
                    "terminal First position must not create an anchor"
                );
            }
        }
    }

    #[test]
    fn claim_first_scope_finds_broadest_peer_in_one_pass() {
        let query = Query::first("p", Save::all()).unwrap().build();
        let query = &query;
        let mut executor = QueryExecutor::new(query);

        let parent = ElementId(1);
        let position = Position {
            selection: QuerySectionId(0),
            state: TransitionId(0),
        };

        // Put the broadest peer after the selected cursor to verify order independence.
        let peer_before = ScopedCursor::new_moving(3, parent, position);
        let selected = ScopedCursor::new_moving(4, parent, position);
        let peer_after = ScopedCursor::new_moving(1, parent, position);
        executor.cursors.extend([peer_before, selected, peer_after]);

        let selected_index = 2;
        let selected_depth = 4;
        assert_eq!(executor.cursors[selected_index].scope_depth, 4);

        executor.claim_first_scope(QuerySectionId(0), parent, selected_index, selected_depth);

        let winner = &executor.cursors[selected_index];
        assert!(winner.is_first_winner());
        assert!(winner.is_complete());
        assert_eq!(winner.scope_depth, 1);
        assert_eq!(winner.unwind_depth(), Some(selected_depth));

        for &peer_index in &[1usize, 3] {
            let peer = &executor.cursors[peer_index];
            assert!(peer.is_complete(), "peer {peer_index} must be canceled");
            assert!(!peer.is_first_winner());
            assert_eq!(peer.unwind_depth(), None);
        }
    }

    #[test]
    fn claim_first_scope_sentinel_peer_after_selected_rebinds_winner() {
        let query = Query::first("p", Save::all()).unwrap().build();
        let query = &query;
        let mut executor = QueryExecutor::new(query);

        let parent = ElementId(1);
        let position = Position {
            selection: QuerySectionId(0),
            state: TransitionId(0),
        };

        let selected = ScopedCursor::new_moving(2, parent, position);
        let mut sentinel_peer = ScopedCursor::new_moving(0, parent, position);
        sentinel_peer.scope_depth = SENTINEL_SCOPE;
        executor.cursors.extend([selected, sentinel_peer]);

        let selected_index = 1;
        let selected_depth = 2;
        executor.claim_first_scope(QuerySectionId(0), parent, selected_index, selected_depth);

        let winner = &executor.cursors[selected_index];
        assert!(winner.is_first_winner());
        assert!(winner.is_complete());
        assert_eq!(winner.scope_depth, SENTINEL_SCOPE);
        assert_eq!(winner.unwind_depth(), Some(selected_depth));

        let peer = &executor.cursors[2];
        assert!(peer.is_complete());
        assert!(!peer.is_first_winner());
        assert_eq!(peer.unwind_depth(), None);
        assert_eq!(peer.scope_depth, SENTINEL_SCOPE);
    }

    #[test]
    fn first_winner_ownership_survives_prefix_close() {
        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| Ok([article.first("div > p", Save::all())?]))
            .unwrap()
            .build();
        let query = &query;
        let mut store = Store::default();
        let mut executor = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        executor.next(0, &elem("article"), &doc_pos(0), &mut store, &mut save_hits);
        let article_id = save_hits[0].element_id;

        executor.next(0, &elem("div"), &doc_pos(1), &mut store, &mut save_hits);
        executor.next(0, &elem("p"), &doc_pos(2), &mut store, &mut save_hits);

        let first_section = QuerySectionId(1);
        let winner = executor
            .cursors
            .iter()
            .find(|cursor| {
                cursor.is_first_winner()
                    && cursor.position.selection == first_section
                    && cursor.parent == article_id
            })
            .expect("p must claim article-scoped First");
        assert_eq!(winner.scope_depth, 0);
        assert_eq!(winner.unwind_depth(), Some(2));

        executor.back(0, "p", &doc_pos(2), &mut store);
        let winner = executor
            .cursors
            .iter()
            .find(|cursor| {
                cursor.is_first_winner()
                    && cursor.position.selection == first_section
                    && cursor.parent == article_id
            })
            .expect("winner must survive selected close");
        assert_eq!(winner.scope_depth, 0);
        assert_eq!(winner.unwind_depth(), None);

        executor.back(0, "div", &doc_pos(1), &mut store);
        assert!(
            executor.cursors.iter().any(|cursor| {
                cursor.is_first_winner()
                    && cursor.position.selection == first_section
                    && cursor.parent == article_id
            }),
            "winner must survive prefix close"
        );

        let terminal = Position {
            selection: first_section,
            state: TransitionId(query.get_selection(first_section).range.end.index() - 1),
        };
        let late_candidate = ScopedCursor::new_moving(1, article_id, terminal);
        assert_eq!(
            executor.try_push_cursor(late_candidate, 0, &mut store, None),
            SpawnOutcome::Dominated,
        );

        executor.back(0, "article", &doc_pos(0), &mut store);
        assert!(
            !executor.cursors.iter().any(|cursor| {
                cursor.is_first_winner()
                    && cursor.position.selection == first_section
                    && cursor.parent == article_id
            }),
            "ownership token should end with output-parent scope"
        );
    }

    #[test]
    fn root_compound_first_winner_owns_sentinel_scope() {
        let query = Query::first("div > p", Save::all()).unwrap().build();
        let query = &query;
        let mut store = Store::default();
        let mut executor = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        executor.next(0, &elem("div"), &doc_pos(0), &mut store, &mut save_hits);
        executor.next(0, &elem("p"), &doc_pos(1), &mut store, &mut save_hits);

        let winner = executor
            .cursors
            .iter()
            .find(|cursor| cursor.is_first_winner())
            .expect("p must claim root First");
        assert_eq!(winner.scope_depth, SENTINEL_SCOPE);
        assert_eq!(winner.unwind_depth(), Some(1));

        executor.back(0, "p", &doc_pos(1), &mut store);
        executor.back(0, "div", &doc_pos(0), &mut store);

        let winner = executor
            .cursors
            .iter()
            .find(|cursor| cursor.is_first_winner())
            .expect("root compound winner must retain sentinel ownership");
        assert_eq!(winner.scope_depth, SENTINEL_SCOPE);
        assert_eq!(winner.unwind_depth(), None);
        assert!(executor.first_scope_is_claimed(&ScopedCursor::new_moving(
            2,
            ElementId::default(),
            winner.position,
        )));
    }

    #[test]
    fn direct_child_first_winner_keeps_output_parent_depth() {
        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| Ok([article.first("> h1", Save::all())?]))
            .unwrap()
            .build();
        let query = &query;
        let mut store = Store::default();
        let mut executor = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        executor.next(0, &elem("article"), &doc_pos(0), &mut store, &mut save_hits);
        let article_id = save_hits[0].element_id;
        executor.next(0, &elem("h1"), &doc_pos(1), &mut store, &mut save_hits);

        let winner = executor
            .cursors
            .iter()
            .find(|cursor| {
                cursor.is_first_winner()
                    && cursor.position.selection == QuerySectionId(1)
                    && cursor.parent == article_id
            })
            .expect("h1 must claim article-scoped First");
        assert_eq!(winner.scope_depth, 0);
        assert_eq!(winner.unwind_depth(), Some(1));
        assert_ne!(winner.scope_depth, 1, "ownership must not be h1 depth");
    }

    #[test]
    fn self_closing_first_winner_retains_output_parent_scope() {
        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| Ok([article.first("div br", Save::all())?]))
            .unwrap()
            .build();
        let query = &query;
        let mut store = Store::default();
        let mut executor = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        executor.next(0, &elem("article"), &doc_pos(0), &mut store, &mut save_hits);
        let article_id = save_hits[0].element_id;
        executor.next(0, &elem("div"), &doc_pos(1), &mut store, &mut save_hits);
        executor.next(
            0,
            &elem("br"),
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 2,
                self_closing: true,
            },
            &mut store,
            &mut save_hits,
        );
        executor.back(
            0,
            "br",
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 2,
                self_closing: true,
            },
            &mut store,
        );

        let first_section = QuerySectionId(1);
        assert!(
            executor.cursors.iter().any(|cursor| {
                cursor.is_first_winner()
                    && cursor.position.selection == first_section
                    && cursor.parent == article_id
                    && cursor.scope_depth == 0
                    && cursor.unwind_depth().is_none()
            }),
            "void winner must keep article ownership after synthetic close"
        );

        executor.back(0, "div", &doc_pos(1), &mut store);
        assert!(
            executor.cursors.iter().any(|cursor| {
                cursor.is_first_winner()
                    && cursor.position.selection == first_section
                    && cursor.parent == article_id
            }),
            "void winner must survive prefix close"
        );

        executor.back(0, "article", &doc_pos(0), &mut store);
        assert!(
            !executor.cursors.iter().any(|cursor| {
                cursor.is_first_winner()
                    && cursor.position.selection == first_section
                    && cursor.parent == article_id
            }),
            "void winner ownership ends at output-parent close"
        );
    }

    #[test]
    fn first_winner_ownership_isolates_independent_output_parents() {
        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| Ok([article.first("div > p", Save::all())?]))
            .unwrap()
            .build();
        let query = &query;
        let mut store = Store::default();
        let mut executor = QueryExecutor::new(query);
        let mut save_hits = Vec::new();
        let first_section = QuerySectionId(1);

        executor.next(0, &elem("article"), &doc_pos(0), &mut store, &mut save_hits);
        let article_outer = save_hits[0].element_id;
        executor.next(0, &elem("div"), &doc_pos(1), &mut store, &mut save_hits);
        executor.next(0, &elem("p"), &doc_pos(2), &mut store, &mut save_hits);
        executor.back(0, "p", &doc_pos(2), &mut store);
        executor.back(0, "div", &doc_pos(1), &mut store);

        executor.next(0, &elem("article"), &doc_pos(1), &mut store, &mut save_hits);
        let article_inner = save_hits.last().expect("inner article").element_id;
        assert_ne!(article_outer, article_inner);
        executor.next(0, &elem("div"), &doc_pos(2), &mut store, &mut save_hits);
        executor.next(0, &elem("p"), &doc_pos(3), &mut store, &mut save_hits);
        executor.back(0, "p", &doc_pos(3), &mut store);
        executor.back(0, "div", &doc_pos(2), &mut store);

        assert!(executor.cursors.iter().any(|cursor| {
            cursor.is_first_winner()
                && cursor.position.selection == first_section
                && cursor.parent == article_outer
                && cursor.scope_depth == 0
        }));
        assert!(executor.cursors.iter().any(|cursor| {
            cursor.is_first_winner()
                && cursor.position.selection == first_section
                && cursor.parent == article_inner
                && cursor.scope_depth == 1
        }));

        executor.back(0, "article", &doc_pos(1), &mut store);
        assert!(
            !executor.cursors.iter().any(|cursor| {
                cursor.is_first_winner()
                    && cursor.position.selection == first_section
                    && cursor.parent == article_inner
            }),
            "closing inner article removes only its owner"
        );
        assert!(
            executor.cursors.iter().any(|cursor| {
                cursor.is_first_winner()
                    && cursor.position.selection == first_section
                    && cursor.parent == article_outer
                    && cursor.scope_depth == 0
            }),
            "outer article owner remains after inner close"
        );

        executor.back(0, "article", &doc_pos(0), &mut store);
        assert!(
            !executor.cursors.iter().any(|cursor| {
                cursor.is_first_winner() && cursor.position.selection == first_section
            }),
            "closing outer article removes its owner"
        );
    }

    #[test]
    fn sequential_first_winners_do_not_accumulate_across_sibling_parents() {
        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| Ok([article.first("div > p", Save::all())?]))
            .unwrap()
            .build();
        let query = &query;
        let mut store = Store::default();
        let mut executor = QueryExecutor::new(query);
        let mut save_hits = Vec::new();
        let first_section = QuerySectionId(1);
        let mut peak_winners = 0usize;

        for _ in 0..3 {
            let before = save_hits.len();
            executor.next(0, &elem("article"), &doc_pos(0), &mut store, &mut save_hits);
            let article_id = save_hits[before].element_id;
            executor.next(0, &elem("div"), &doc_pos(1), &mut store, &mut save_hits);
            executor.next(0, &elem("p"), &doc_pos(2), &mut store, &mut save_hits);
            executor.back(0, "p", &doc_pos(2), &mut store);
            executor.back(0, "div", &doc_pos(1), &mut store);

            let winners = executor
                .cursors
                .iter()
                .filter(|cursor| {
                    cursor.is_first_winner() && cursor.position.selection == first_section
                })
                .count();
            peak_winners = peak_winners.max(winners);
            assert_eq!(winners, 1, "one open article owns one First winner");
            assert!(executor.cursors.iter().any(|cursor| {
                cursor.is_first_winner() && cursor.parent == article_id && cursor.scope_depth == 0
            }));

            executor.back(0, "article", &doc_pos(0), &mut store);
            assert!(
                !executor.cursors.iter().any(|cursor| {
                    cursor.is_first_winner() && cursor.position.selection == first_section
                }),
                "closed article must drop its First ownership token"
            );
        }

        assert_eq!(peak_winners, 1);
    }

    #[test]
    fn claimed_first_scope_suppresses_same_parent_admission() {
        let query = Query::first("p", Save::all()).unwrap().build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        let parent = ElementId::default();
        let position = Position {
            selection: QuerySectionId(0),
            state: TransitionId(0),
        };
        let mut winner = ScopedCursor::new_moving(0, parent, position);
        winner.select_first_until_close(0, 0);
        selection.cursors.push(winner);

        let original_len = selection.cursors.len();
        let candidate = ScopedCursor::new_moving(1, parent, position);
        let outcome = selection.try_push_cursor(candidate, 0, &mut store, None);

        assert_eq!(outcome, SpawnOutcome::Dominated);
        assert_eq!(selection.cursors.len(), original_len);
    }

    #[test]
    fn claimed_first_scope_does_not_suppress_independent_output_parent() {
        let query = Query::first("p", Save::all()).unwrap().build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        let position = Position {
            selection: QuerySectionId(0),
            state: TransitionId(0),
        };
        let parent_a = ElementId::default();
        let parent_b = ElementId(1);
        let mut winner = ScopedCursor::new_moving(0, parent_a, position);
        winner.select_first_until_close(0, 0);
        selection.cursors.push(winner);

        let original_len = selection.cursors.len();
        let candidate = ScopedCursor::new_moving(1, parent_b, position);
        let outcome = selection.try_push_cursor(candidate, 0, &mut store, None);

        assert_eq!(outcome, SpawnOutcome::Inserted);
        assert_eq!(selection.cursors.len(), original_len + 1);
    }

    #[test]
    fn claimed_first_scope_does_not_suppress_different_section() {
        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| {
                Ok([
                    article.first("p", Save::all())?,
                    article.first("span", Save::all())?,
                ])
            })
            .unwrap()
            .build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        let parent = ElementId::default();
        let p_position = Position {
            selection: QuerySectionId(1),
            state: query.get_selection(QuerySectionId(1)).range.start,
        };
        let span_position = Position {
            selection: QuerySectionId(2),
            state: query.get_selection(QuerySectionId(2)).range.start,
        };
        assert!(matches!(
            query.get_section_selection_kind(QuerySectionId(1)),
            SelectionKind::First
        ));
        assert!(matches!(
            query.get_section_selection_kind(QuerySectionId(2)),
            SelectionKind::First
        ));

        let mut winner = ScopedCursor::new_moving(0, parent, p_position);
        winner.select_first_until_close(0, 0);
        selection.cursors.push(winner);

        let original_len = selection.cursors.len();
        let candidate = ScopedCursor::new_moving(1, parent, span_position);
        let outcome = selection.try_push_cursor(candidate, 0, &mut store, None);

        assert_eq!(outcome, SpawnOutcome::Inserted);
        assert_eq!(selection.cursors.len(), original_len + 1);
    }

    #[test]
    fn claimed_first_scope_does_not_suppress_all_section() {
        let query = Query::all("article", Save::all())
            .unwrap()
            .then(|article| {
                Ok([
                    article.first("> h1", Save::all())?,
                    article.all("> p", Save::all())?,
                ])
            })
            .unwrap()
            .build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        let parent = ElementId::default();
        let first_position = Position {
            selection: QuerySectionId(1),
            state: query.get_selection(QuerySectionId(1)).range.start,
        };
        let all_position = Position {
            selection: QuerySectionId(2),
            state: query.get_selection(QuerySectionId(2)).range.start,
        };
        assert!(matches!(
            query.get_section_selection_kind(QuerySectionId(1)),
            SelectionKind::First
        ));
        assert!(matches!(
            query.get_section_selection_kind(QuerySectionId(2)),
            SelectionKind::All
        ));

        let mut winner = ScopedCursor::new_moving(0, parent, first_position);
        winner.select_first_until_close(0, 0);
        selection.cursors.push(winner);

        let original_len = selection.cursors.len();
        let candidate = ScopedCursor::new_moving(1, parent, all_position);
        let outcome = selection.try_push_cursor(candidate, 0, &mut store, None);

        assert_eq!(outcome, SpawnOutcome::Inserted);
        assert_eq!(selection.cursors.len(), original_len + 1);
    }

    #[test]
    fn first_candidate_admitted_before_winner_exists() {
        let query = Query::first("p", Save::all()).unwrap().build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        // Retire the root obligation so admission is not combinator-dominated.
        selection.cursors[0].cancel_complete();

        let position = Position {
            selection: QuerySectionId(0),
            state: TransitionId(0),
        };
        let original_len = selection.cursors.len();
        let candidate = ScopedCursor::new_moving(1, ElementId::default(), position);
        let outcome = selection.try_push_cursor(candidate, 0, &mut store, None);

        assert_eq!(outcome, SpawnOutcome::Inserted);
        assert_eq!(selection.cursors.len(), original_len + 1);
        assert!(!selection.first_scope_is_claimed(&ScopedCursor::new_moving(
            2,
            ElementId::default(),
            position
        )));
    }

    #[test]
    fn delayed_same_scope_admission_after_winner_is_dominated() {
        let query = Query::first("div > p", Save::all()).unwrap().build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(0, &elem("div"), &doc_pos(0), &mut store, &mut save_hits);
        selection.next(0, &elem("p"), &doc_pos(1), &mut store, &mut save_hits);
        assert_eq!(save_hits.len(), 1);

        let winner = selection
            .cursors
            .iter()
            .find(|c| c.is_first_winner())
            .expect("first p must claim the scope");
        let section = winner.position.selection;
        let parent = winner.parent;
        let terminal = winner.position;

        let original_len = selection.cursors.len();
        let late = ScopedCursor::new_moving(2, parent, terminal);
        let outcome = selection.try_push_cursor(late, 0, &mut store, None);
        assert_eq!(outcome, SpawnOutcome::Dominated);
        assert_eq!(selection.cursors.len(), original_len);
        assert!(selection.first_scope_is_claimed(&ScopedCursor::new_moving(3, parent, terminal)));
        assert_eq!(section, QuerySectionId(0));
    }

    #[test]
    fn first_failed_child_candidate_reactivates_prefix() {
        let query = Query::first("div > p", Save::all()).unwrap().build();
        let query = &query;
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);
        let mut save_hits = Vec::new();

        selection.next(0, &elem("div"), &doc_pos(0), &mut store, &mut save_hits);
        let blocked_idx = selection
            .cursors
            .iter()
            .position(|c| c.is_moving() && c.is_blocked() && c.unwind_depth() == Some(0))
            .expect("failed First prefix must block until </div>");
        let before_position = selection.cursors[blocked_idx].position;

        selection.back(0, "div", &doc_pos(0), &mut store);
        let reactivated = selection
            .cursors
            .iter()
            .find(|c| c.is_moving() && c.position == before_position)
            .expect("prefix cursor retained at same transition");
        assert!(reactivated.is_active());
        assert!(!reactivated.is_first_winner());

        selection.next(0, &elem("div"), &doc_pos(0), &mut store, &mut save_hits);
        selection.next(0, &elem("p"), &doc_pos(1), &mut store, &mut save_hits);
        assert_eq!(save_hits.len(), 1);
        let winner = selection
            .cursors
            .iter()
            .find(|c| c.is_first_winner())
            .expect("second candidate must win");
        assert!(winner.is_moving());
        assert!(winner.is_complete());
        assert_eq!(winner.unwind_depth(), Some(1));
    }
}
