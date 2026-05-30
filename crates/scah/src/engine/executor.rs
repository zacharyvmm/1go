use super::cursor::{CursorMode, CursorOps, ScopedCursor};
use super::multiplexer::{DocumentPosition, SaveHit};
#[cfg(any(debug_assertions, test))]
use crate::debug::{CursorTraceKind, ScopedCursorReason, TraceEvent, TransitionRejectReason};
use crate::store::Store;
use crate::{Position, QuerySectionId, QuerySpec, SelectionKind, TransitionId, XHtmlElement};
use crate::store::ElementId;

/*
 * A Selection works runs the fsm's using 2 types of cursors:
 * 1) MOVING cursors — actively advance position on each match.
 *    The root cursor is a MOVING cursor with scope_depth = 0 (never pruned).
 * 2) ANCHORED cursors — fixed at a scope_depth. They never move; when they
 *    match, they advance an ANCHORED clone (same as old scoped cursor behavior).
 *
 * All cursors live in a single Vec<ScopedCursor>, processed in a unified loop.
 *
 * Anchored cursors are pruned in back() when scope_depth >= close_depth.
 * Moving cursors reactivate (clear end) or step backward on close at matching depth.
 */

/// The `QueryExecutor` is an NFA execution engine optimized for streaming StAX events.
///
/// Because CSS selectors like descendant (` `) are non-deterministic (a match can
/// occur at the current depth or any arbitrary depth below it), a single cursor
/// isn't enough.
///
/// ## Execution Model
///
/// All cursors (root MOVING, ANCHORED forks, and MOVING children) live in a single
/// vector, processed in one loop:
///
/// 1. **MOVING cursors**: Try to match the current element. On match:
///    - If the transition is a Descendant combinator: fork an ANCHORED clone
///      at the current depth (to re-match at deeper levels).
///    - Advance the MOVING cursor via `next_position` (consume it; don't keep
///      the original to avoid re-matching).
/// 2. **ANCHORED cursors**: Try to match the current element. On match:
///    - Keep the ANCHORED original (it stays to match future elements).
///    - Advance an ANCHORED clone via `next_position` (add_depth is no-op for
///      ANCHORED cursors, so the clone stays anchored at the original's scope).
/// 3. **Pruning**: Anchored cursors are pruned in `back()` when their
///    `scope_depth >= close_depth`. The root MOVING cursor reactivates or
///    steps backward when `last_match_depth == close_depth`.
pub struct QueryExecutor<'a, Q> {
    pub(crate) query: &'a Q,
    pub(crate) cursors: Vec<ScopedCursor>,
}

impl<'a, 'html, 'query: 'html, Q> QueryExecutor<'a, Q>
where
    Q: QuerySpec<'query>,
{
    pub fn new(query: &'a Q) -> Self {
        let root_cursor = ScopedCursor::new_moving(
            0,
            ElementId::default(),
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
        );
        Self {
            query,
            cursors: vec![root_cursor],
        }
    }

    /// Advance a cursor's position after a successful match.
    ///
    /// - First tries the next transition in the same query section.
    /// - If none, tries the next child section.
    /// - For child sections with siblings, forks ANCHORED cursors for each
    ///   sibling so they can be tried independently.
    /// - If neither transition nor child is available, sets `end = true`.
    fn next_position(
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] runner_index: usize,
        tree: &Q,
        cursors: &mut Vec<ScopedCursor>,
        depth: super::DepthSize,
        cursor: &mut ScopedCursor,
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] store: &mut Store<
            'html,
            'query,
        >,
    ) {
        cursor.add_depth(depth);
        if let Some(next_transition) = cursor.get_position().next_transition(tree) {
            cursor.set_state(next_transition);
            cursor.set_end(false);
        } else if let Some(child) = cursor.get_position().next_child(tree) {
            cursor.set_position(child);
            cursor.set_end(false);

            let mut has_sibling = cursor.get_position().next_sibling(tree);
            while let Some(sibling) = has_sibling {
                cursors.push(ScopedCursor::new_anchored(
                    depth,
                    cursor.get_parent(),
                    *cursor.get_position(),
                ));
                #[cfg(any(debug_assertions, test))]
                {
                    let created = cursors.last().unwrap();
                    crate::scah_trace!(
                        store,
                        TraceEvent::ScopedCursorCreated {
                            runner_index,
                            depth,
                            scope_depth: created.scope_depth,
                            parent: created.parent,
                            selection: created.position.selection,
                            state: created.position.state,
                            reason: ScopedCursorReason::BranchSibling,
                        }
                    );
                }

                cursor.set_position(sibling);
                has_sibling = sibling.next_sibling(tree);
            }
        } else {
            cursor.set_end(true);
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

    /// Process an open-tag event against all cursors.
    ///
    /// Drains all current cursors, evaluates each one against the element, and
    /// produces new cursors (advanced MOVING, spawned children, etc.) into a
    /// fresh vector. This avoids the complexity of mutating the cursor list
    /// in-place during iteration.
    pub fn next(
        &mut self,
        runner_index: usize,
        element: &XHtmlElement<'html>,
        document_position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        save_hits: &mut Vec<SaveHit>,
    ) {
        let depth = document_position.element_depth;
        let old_cursors = std::mem::take(&mut self.cursors);
        let mut new_cursors = Vec::with_capacity(old_cursors.len().max(1) * 2);

        let mut cursor_index = 0usize;

        for cursor in old_cursors {
            let is_moving = cursor.is_moving();
            let is_anchored = cursor.is_anchored();

            if !cursor.next(self.query, depth, element) {
                #[cfg(any(debug_assertions, test))]
                {
                    let kind = if cursor_index == 0 && is_moving {
                        CursorTraceKind::Root
                    } else {
                        CursorTraceKind::Scoped { index: cursor_index }
                    };
                    crate::scah_trace!(
                        store,
                        TraceEvent::TransitionRejected {
                            runner_index,
                            cursor: kind,
                            selector: self.query.get_selection(cursor.position.selection).source,
                            element: element.name,
                            depth,
                            selection: cursor.position.selection,
                            state: cursor.position.state,
                            reason: Self::transition_reject_reason(
                                self.query,
                                &cursor.position,
                                depth,
                                cursor.effective_last_depth(),
                                element,
                            ),
                        }
                    );
                }
                // Keep this cursor for future elements
                new_cursors.push(cursor);
                cursor_index += 1;
                continue;
            }

            crate::scah_trace!(
                store,
                TraceEvent::TransitionMatched {
                    runner_index,
                    cursor: if cursor_index == 0 && is_moving {
                        CursorTraceKind::Root
                    } else {
                        CursorTraceKind::Scoped { index: cursor_index }
                    },
                    selector: self
                        .query
                        .get_selection(cursor.position.selection)
                        .source,
                    element: element.name,
                    depth,
                    selection: cursor.position.selection,
                    state: cursor.position.state,
                }
            );

            // --- Descendant combinator fork (MOVING cursors only) ---
            if is_moving && self.query.is_descendant(cursor.position.state) {
                let last_save_point = self.query.is_last_save_point(&cursor.position);
                let section_kind = self
                    .query
                    .get_section_selection_kind(cursor.position.selection);
                let is_all = matches!(section_kind, SelectionKind::All);

                if !last_save_point || is_all {
                    new_cursors.push(cursor.anchor_clone(depth));
                    #[cfg(any(debug_assertions, test))]
                    {
                        let created = new_cursors.last().unwrap();
                        crate::scah_trace!(
                            store,
                            TraceEvent::ScopedCursorCreated {
                                runner_index,
                                depth,
                                scope_depth: created.scope_depth,
                                parent: created.parent,
                                selection: created.position.selection,
                                state: created.position.state,
                                reason: ScopedCursorReason::DescendantFork,
                            }
                        );
                    }
                }
            }

            if is_anchored {
                // ANCHORED: keep the original (it stays to match future elements),
                // advance an ANCHORED clone (same as old scoped cursor behavior).
                // add_depth is a no-op for ANCHORED cursors, so the clone stays
                // anchored at the original's scope_depth and is pruned normally.
                new_cursors.push(cursor.clone());

                let mut advanced = cursor.clone();

                if self.query.is_save_point(&advanced.position) {
                    save_hits.push(Self::save_element(
                        runner_index,
                        self.query,
                        store,
                        element.clone(),
                        &mut advanced,
                    ));
                }

                if !element.is_self_closing() {
                    Self::next_position(
                        runner_index,
                        self.query,
                        &mut new_cursors,
                        depth,
                        &mut advanced,
                        store,
                    );
                }

                new_cursors.push(advanced);
            } else {
                // MOVING: consume the original (advance it in place),
                // do NOT keep the pre-advance state.
                let mut advanced = cursor;

                if self.query.is_save_point(&advanced.position) {
                    save_hits.push(Self::save_element(
                        runner_index,
                        self.query,
                        store,
                        element.clone(),
                        &mut advanced,
                    ));
                }

                if !element.is_self_closing() {
                    Self::next_position(
                        runner_index,
                        self.query,
                        &mut new_cursors,
                        depth,
                        &mut advanced,
                        store,
                    );
                }

                new_cursors.push(advanced);
            }

            cursor_index += 1;
        }

        self.cursors = new_cursors;
    }

    pub fn early_exit(&self) -> bool {
        if let Some(early_exit_section) = self.query.exit_at_section_end() {
            // Check the root cursor (first MOVING cursor with scope_depth == 0)
            for cursor in &self.cursors {
                if cursor.is_moving() && cursor.scope_depth == 0 {
                    return early_exit_section == cursor.position.selection;
                }
            }
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

        // Walk backwards so swap_remove only moves already-visited retained cursors.
        // Prune ANCHORED cursors where scope_depth >= close_depth.
        let mut i = self.cursors.len();
        while i > 0 {
            i -= 1;

            let cursor = &self.cursors[i];

            if cursor.is_anchored() && cursor.scope_depth >= close_depth {
                let pruned = self.cursors.swap_remove(i);
                last_pruned_parent = Some(pruned.parent);
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

        // Update root parent from last pruned cursor
        if let (Some(parent), Some(root)) = (
            last_pruned_parent,
            self.cursors
                .iter_mut()
                .find(|c| c.is_moving() && c.scope_depth == 0),
        ) {
            root.parent = parent;
        }

        // Handle MOVING root cursor: reactivate / step backward
        // Uses match_stack logic matching the old Cursor behavior.
        if let Some(root_idx) = self
            .cursors
            .iter()
            .position(|c| c.is_moving() && c.scope_depth == 0)
        {
            let root = &self.cursors[root_idx];
            let effective_last = root.effective_last_depth();
            if effective_last == close_depth {
                let mut root = self.cursors.swap_remove(root_idx);
                if root.end() {
                    // Reactivate for siblings: clear end, pop match_stack
                    if let CursorMode::Moving {
                        ref mut match_stack,
                        end: ref mut end_flag,
                    } = root.mode
                    {
                        *end_flag = false;
                        match_stack.pop();
                    }
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
                    self.cursors.push(root);
                    return true;
                } else {
                    // Step backward: pop match_stack, call position.back
                    root.step_backward(self.query);
                    self.cursors.push(root);
                    return true;
                }
            }
        }

        false
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
            },
            &mut store,
            &mut Vec::new(),
        );

        // After matching "div" at depth 0:
        // - Root cursor matched, descendant fork created ANCHORED cursor
        // - Root cursor was advanced to the next transition ("a")
        // Order in cursors: [anchored_fork, advanced_root]
        assert!(store.get("div a").is_none());

        // We should have 2 cursors: the anchored fork at depth 0, and the advanced root
        assert_eq!(selection.cursors.len(), 2);

        // Anchored fork at depth 0 (pushed first)
        let anchored = &selection.cursors[0];
        assert!(anchored.is_anchored());
        assert_eq!(anchored.scope_depth, 0);
        assert_eq!(anchored.position.state, TransitionId(0));

        // Advanced root cursor: state moved to the "a" transition (pushed second)
        let root = &selection.cursors[1];
        assert!(root.is_moving());
        assert_eq!(root.scope_depth, 0);
        assert_eq!(root.position.state, TransitionId(1));

        // Now match "a" at depth 1
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
            },
            &mut store,
            &mut Vec::new(),
        );

        // Should have saved the "a" element (last state of section is save point)
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
            },
            &mut store,
            &mut Vec::new(),
        );

        assert!(store.get("div p.class").is_none());

        // After matching div: root advanced (section changed via .then()),
        // plus anchored fork at depth 0
        assert!(selection.cursors.len() >= 2);

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

        // Set up a mix of anchored and moving cursors
        selection.cursors = vec![
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
            },
            &mut store,
        );

        // Cursors with scope_depth >= 2 should be pruned: (3, 2, 0? no, 0 < 2)
        // scope_depth 3: pruned
        // scope_depth 2: pruned (2 >= 2)
        // scope_depth 1: kept (1 < 2)
        // scope_depth 0: kept (0 < 2)
        // Retained: 1(10), 1(30), 0(50)
        let retained = &selection.cursors;
        assert_eq!(retained.len(), 3);
        assert!(
            retained
                .iter()
                .all(|c| c.scope_depth < 2)
        );

        let mut retained_parents: Vec<usize> = retained
            .iter()
            .map(|c| c.parent.index())
            .collect();
        retained_parents.sort_unstable();
        assert_eq!(retained_parents, vec![10, 30, 50]);
    }

    #[test]
    fn test_simple_open_close() {
        let query = Query::first("div", Save::none()).unwrap().build();

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
            },
            &mut store,
            &mut Vec::new(),
        );
        store.text_content.set_start(4);

        // After matching div at depth 0 (simple no-descendant query):
        // Root cursor was advanced, set to end (no more transitions)
        // Should have 1 cursor (the advanced root)
        assert_eq!(selection.cursors.len(), 1);
        let root = &selection.cursors[0];
        assert!(root.is_moving());
        assert_eq!(root.scope_depth, 0);
        assert!(root.end());

        store.text_content.push(&Reader::new("<div></div>"), 4);
        let reactivated = selection.back(
            0,
            "div",
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
            },
            &mut store,
        );

        // Should reactivate (end cleared, last_match_depth reset)
        assert!(reactivated);
        let root = &selection.cursors[0];
        assert!(root.is_moving());
        assert!(!root.end());
    }

    // ─── New targeted tests (Phase 5.3) ───

    #[test]
    fn test_descendant_forking_with_anchoring_model() {
        // div a — descendant combinator creates anchored fork at match depth
        let query = &Query::all("div a", Save::none()).unwrap().build();
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        // Match "div" at depth 0
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
            },
            &mut store,
            &mut Vec::new(),
        );

        // Should have anchored fork at depth 0
        let anchored_count = selection
            .cursors
            .iter()
            .filter(|c| c.is_anchored())
            .count();
        assert_eq!(anchored_count, 1, "Expected 1 anchored fork after div match");

        let anchored = selection
            .cursors
            .iter()
            .find(|c| c.is_anchored())
            .unwrap();
        assert_eq!(anchored.scope_depth, 0);
        assert_eq!(anchored.position.state, TransitionId(0)); // still at "div" transition

        // Match "a" at depth 1
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
            },
            &mut store,
            &mut Vec::new(),
        );

        // Should have saved "a"
        assert_eq!(
            store.get("div a").unwrap().count(),
            1,
            "Should have saved one 'a' element"
        );
    }

    #[test]
    fn test_child_combinator_sibling_rematching() {
        // main > section with multiple <section> elements
        // Child combinator should re-activate the cursor for sibling matching.
        let html = "<main><section>A</section><section>B</section></main>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("main > section", Save::all())
            .unwrap()
            .build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let sections: Vec<_> = store.get("main > section").unwrap().collect();
        assert_eq!(sections.len(), 2, "Expected 2 section matches");
        assert_eq!(sections[0].name, "section");
        assert_eq!(sections[1].name, "section");
    }

    #[test]
    fn test_nested_descendant_matching() {
        // div div a — nested descendant matching
        let html = "<div><div><a>link</a></div></div>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("div div a", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let links: Vec<_> = store.get("div div a").unwrap().collect();
        assert_eq!(links.len(), 1, "Expected 1 nested descendant match");
        assert_eq!(links[0].name, "a");
    }

    #[test]
    fn test_mixed_child_and_descendant() {
        // main > section div a — child then descendant
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
        assert_eq!(links.len(), 1, "Expected 1 mixed combinator match");
        assert_eq!(links[0].name, "a");
    }

    #[test]
    fn test_then_branching_with_anchoring_model() {
        // section .product h1 | img | p (then branching)
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

        // Check each branch
        let products: Vec<_> = store.get("section .product").unwrap().collect();
        assert_eq!(products.len(), 1);

        let product = products[0];
        let h1s: Vec<_> = product.get(&store, "h1").unwrap().collect();
        assert_eq!(h1s.len(), 1);
        assert_eq!(h1s[0].name, "h1");

        let imgs: Vec<_> = product.get(&store, "img").unwrap().collect();
        assert_eq!(imgs.len(), 1);
        assert_eq!(imgs[0].name, "img");

        let ps: Vec<_> = product.get(&store, "p").unwrap().collect();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].name, "p");
    }

    #[test]
    fn test_self_closing_elements_preserved() {
        // Self-closing <br> should be matched correctly without affecting sibling matches
        let html = "<div><br /><span>text</span></div>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("div span", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let spans: Vec<_> = store.get("div span").unwrap().collect();
        assert_eq!(spans.len(), 1, "Expected 1 span match despite self-closing br");
        assert_eq!(spans[0].name, "span");
    }

    #[test]
    fn test_implicit_li_close() {
        // Implicit <li> auto-close should not affect cursor matching
        let html = "<ul><li>Item 1<li>Item 2</ul>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("ul li", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let items: Vec<_> = store.get("ul li").unwrap().collect();
        assert_eq!(
            items.len(),
            2,
            "Expected 2 li matches with implicit close"
        );
        assert_eq!(items[0].name, "li");
        assert_eq!(items[1].name, "li");
    }

    #[test]
    fn test_multiple_nested_descendant_levels() {
        // Deep nesting: body div ul li a
        let html =
            "<body><div><ul><li><a href='#'>link</a></li></ul></div></body>";
        let reader = &mut Reader::new(html);
        let query = &[Query::all("body div ul li a", Save::all())
            .unwrap()
            .build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let links: Vec<_> = store.get("body div ul li a").unwrap().collect();
        assert_eq!(links.len(), 1, "Expected 1 deeply nested match");
        assert_eq!(links[0].name, "a");
    }
}
