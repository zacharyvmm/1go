use super::cursor::{CursorMode, CursorOps, ScopedCursor};
use super::multiplexer::{DocumentPosition, SaveHit};
#[cfg(any(debug_assertions, test))]
use crate::debug::{CursorTraceKind, ScopedCursorReason, TraceEvent, TransitionRejectReason};
use crate::store::Store;
use crate::{Position, QuerySectionId, QuerySpec, SelectionKind, TransitionId, XHtmlElement};
use crate::store::ElementId;

/*
 * A Selection works runs the fsm's using a unified ScopedCursor type but
 * splits root (MOVING, mutated in place) from scoped (ANCHORED, in a vec)
 * for performance — avoiding per-element Vec reallocation.
 *
 * 1) The root MOVING cursor is mutated in place (zero allocation per element).
 * 2) ANCHORED cursors live in a Vec, iterated by index. New cursors are pushed
 *    to the end; the pre-computed len ensures they aren't reprocessed in the
 *    current call.
 *
 * Anchored cursors are pruned in back() when scope_depth >= close_depth.
 * The root cursor reactivates or steps backward on close at matching depth.
 */

/// The `QueryExecutor` is an NFA execution engine optimized for streaming StAX events.
///
/// Because CSS selectors like descendant (` `) are non-deterministic (a match can
/// occur at the current depth or any arbitrary depth below it), a single cursor
/// isn't enough.
///
/// ## Execution Model
/// 1. **ROOT MOVING cursor**: Tracks position through the query tree, mutated in
///    place on each match. Forks ANCHORED cursors on Descendant combinator matches.
/// 2. **ANCHORED cursors**: Fixed at a `scope_depth`. When they match, they produce
///    an ANCHORED clone that advances via `next_position` (add_depth is a no-op).
/// 3. **Pruning**: Anchored cursors are pruned in `back()` when their
///    `scope_depth >= close_depth`. The root MOVING cursor reactivates or
///    steps backward when `last_match_depth == close_depth`.
pub struct QueryExecutor<'a, Q> {
    pub(crate) query: &'a Q,
    /// MOVING root cursor. Mutated in-place — no Vec allocation per element.
    pub(crate) root: ScopedCursor,
    /// ANCHORED cursors (descendant forks, sibling forks, and their clones).
    pub(crate) scoped: Vec<ScopedCursor>,
}

impl<'a, 'html, 'query: 'html, Q> QueryExecutor<'a, Q>
where
    Q: QuerySpec<'query>,
{
    pub fn new(query: &'a Q) -> Self {
        let root = ScopedCursor::new_moving(
            0,
            ElementId::default(),
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
        );
        Self {
            query,
            root,
            scoped: Vec::new(),
        }
    }

    /// Advance a cursor's position after a successful match.
    ///
    /// - First tries the next transition in the same query section.
    /// - If none, tries the next child section.
    /// - For child sections with siblings, forks ANCHORED cursors for each
    ///   sibling so they can be tried independently.
    /// - If neither transition nor child is available, sets `end = true`.
    ///
    /// Called for both the root MOVING cursor and ANCHORED cursor clones.
    fn next_position(
        #[cfg_attr(not(any(debug_assertions, test)), allow(unused_variables))] runner_index: usize,
        tree: &Q,
        scoped: &mut Vec<ScopedCursor>,
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
                scoped.push(ScopedCursor::new_anchored(
                    depth,
                    cursor.get_parent(),
                    *cursor.get_position(),
                ));
                #[cfg(any(debug_assertions, test))]
                {
                    let created = scoped.last().unwrap();
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

    /// Process an open-tag event.
    ///
    /// STEP 1: Iterate ANCHORED cursors by index. New cursors are pushed to the
    /// end; the pre-computed len ensures they aren't reprocessed in this call.
    /// STEP 2: Mutate the root MOVING cursor in place.
    pub fn next(
        &mut self,
        runner_index: usize,
        element: &XHtmlElement<'html>,
        document_position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        save_hits: &mut Vec<SaveHit>,
    ) {
        let depth = document_position.element_depth;

        // ── STEP 1: ANCHORED cursors ──
        let scoped_len = self.scoped.len();
        for i in 0..scoped_len {
            // Check match first, collecting all needed data in a block.
            let (matched, position, is_first) = {
                let cursor = &self.scoped[i];
                let matched = cursor.next(self.query, depth, element);
                let position = cursor.position;
                let section_kind =
                    self.query.get_section_selection_kind(cursor.position.selection);
                let is_first = matches!(section_kind, SelectionKind::First);
                (matched, position, is_first)
            };

            if !matched {
                #[cfg(any(debug_assertions, test))]
                {
                    crate::scah_trace!(
                        store,
                        TraceEvent::TransitionRejected {
                            runner_index,
                            cursor: CursorTraceKind::Scoped { index: i },
                            selector: self.query.get_selection(position.selection).source,
                            element: element.name,
                            depth,
                            selection: position.selection,
                            state: position.state,
                            reason: Self::transition_reject_reason(
                                self.query,
                                &position,
                                depth,
                                self.scoped[i].effective_last_depth(),
                                element,
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
                    cursor: CursorTraceKind::Scoped { index: i },
                    selector: self.query.get_selection(position.selection).source,
                    element: element.name,
                    depth,
                    selection: position.selection,
                    state: position.state,
                }
            );

            // ANCHORED cursor matched: clone and advance.
            // For First selections, mark the original anchored cursor as consumed
            // so it won't match again on subsequent elements.
            let mut advanced = self.scoped[i].clone();

            if is_first {
                self.scoped[i].set_end(true);
            }

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
                    &mut self.scoped,
                    depth,
                    &mut advanced,
                    store,
                );
            }

            self.scoped.push(advanced);
        }

        // ── STEP 2: Root MOVING cursor ──
        if self.root.next(self.query, depth, element) {
            crate::scah_trace!(
                store,
                TraceEvent::TransitionMatched {
                    runner_index,
                    cursor: CursorTraceKind::Root,
                    selector: self.query.get_selection(self.root.position.selection).source,
                    element: element.name,
                    depth,
                    selection: self.root.position.selection,
                    state: self.root.position.state,
                }
            );

            let is_descendant = self.query.is_descendant(self.root.position.state);
            let last_save_point = self.query.is_last_save_point(&self.root.position);
            let is_section_end = self.query.is_save_point(&self.root.position);
            let section_kind = self
                .query
                .get_section_selection_kind(self.root.position.selection);
            let is_all = matches!(section_kind, SelectionKind::All);

            // Fork only at section boundaries (save points). For single-transition
            // sections (e.g., standalone "div") the only state IS the section end.
            // Intermediate states within a multi-transition section don't need
            // forks — the root cursor's advancement handles the same path.
            if is_descendant && is_section_end && (!last_save_point || is_all) {
                self.scoped
                    .push(self.root.anchor_clone(depth));
                #[cfg(any(debug_assertions, test))]
                {
                    let created = self.scoped.last().unwrap();
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

            if self.query.is_save_point(&self.root.position) {
                save_hits.push(Self::save_element(
                    runner_index,
                    self.query,
                    store,
                    element.clone(),
                    &mut self.root,
                ));
            }

            if !element.is_self_closing() {
                Self::next_position(
                    runner_index,
                    self.query,
                    &mut self.scoped,
                    depth,
                    &mut self.root,
                    store,
                );
            }
        } else {
            #[cfg(any(debug_assertions, test))]
            {
                let last_depth = self.root.effective_last_depth();
                crate::scah_trace!(
                    store,
                    TraceEvent::TransitionRejected {
                        runner_index,
                        cursor: CursorTraceKind::Root,
                        selector: self.query.get_selection(self.root.position.selection).source,
                        element: element.name,
                        depth,
                        selection: self.root.position.selection,
                        state: self.root.position.state,
                        reason: Self::transition_reject_reason(
                            self.query,
                            &self.root.position,
                            depth,
                            last_depth,
                            element,
                        ),
                    }
                );
            }
        }
    }

    pub fn early_exit(&self) -> bool {
        if let Some(early_exit_section) = self.query.exit_at_section_end() {
            return early_exit_section == self.root.position.selection;
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
        let mut i = self.scoped.len();
        while i > 0 {
            i -= 1;

            if self.scoped[i].scope_depth >= close_depth {
                let pruned = self.scoped.swap_remove(i);
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
        if let (Some(parent), Some(root_mut)) = (
            last_pruned_parent,
            (self.root.scope_depth == 0).then_some(&mut self.root),
        ) {
            root_mut.parent = parent;
        }

        // Handle MOVING root cursor: reactivate / step backward
        if self.root.last_depth == close_depth {
            if self.root.end() {
                // For First selections: step backward (like non-end), cursor
                // moves up the query tree to parent section. The end flag
                // prevents further matching at the current position.
                // For All selections: reactivate in place (clear end, don't
                // move position) so siblings can be matched.
                let section_kind =
                    self.query.get_section_selection_kind(self.root.position.selection);
                if matches!(section_kind, SelectionKind::First) {
                    // For First sections: step backward to the parent query
                    // section. The end flag persists so the parent's close
                    // event will trigger the final step and early_exit at
                    // the right depth (after content capture).
                    self.root.step_backward(self.query);
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
                    return true;
                }

                // Reactivate for siblings (All selection): clear end,
                // pop match_stack, restore last_depth.
                if let CursorMode::Moving {
                    ref mut match_stack,
                    end: ref mut end_flag,
                } = self.root.mode
                {
                    *end_flag = false;
                    match_stack.pop();
                    self.root.last_depth =
                        *match_stack.last().unwrap_or(&self.root.scope_depth);
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
                return true;
            } else {
                // Step backward: pop match_stack, call position.back
                self.root.step_backward(self.query);
                return true;
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

        assert!(store.get("div a").is_none());

        // Root cursor advanced past div (next_transition)
        assert_eq!(selection.root.position.state, TransitionId(1));
        // Fork only at section boundaries; state 0 is not a section end for multi-state "div a"
        assert_eq!(selection.scoped.len(), 0);

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

        // Root cursor matches a at depth 1 via descendant combinator
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

        // Set up anchored cursors at various scope depths
        selection.scoped = vec![
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

        // scope_depth >= 2 pruned: 3, 2 pruned. 1, 1, 0 kept.
        let retained = &selection.scoped;
        assert_eq!(retained.len(), 3);
        assert!(retained.iter().all(|c| c.scope_depth < 2));

        let mut retained_parents: Vec<usize> = retained.iter().map(|c| c.parent.index()).collect();
        retained_parents.sort_unstable();
        assert_eq!(retained_parents, vec![10, 30, 50]);
    }

    #[test]
    fn test_simple_open_close() {
        // Use All selection so that back() reactivates the cursor for sibling matching.
        // The anchored fork from the descendant combinator is pruned on close.
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
            },
            &mut store,
            &mut Vec::new(),
        );
        store.text_content.set_start(4);

        // After descendant match, an anchored fork is created (All + descendant)
        assert_eq!(selection.scoped.len(), 1);
        assert!(selection.root.end());

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

        // Reactivated for All selection, anchored cursor pruned
        assert!(reactivated);
        assert!(!selection.root.end());
        assert!(selection.scoped.is_empty());
    }

    // ─── Targeted tests (Phase 5.3) ───

    #[test]
    fn test_descendant_forking_with_anchoring_model() {
        // Forks only happen at section boundaries. Single-transition sections
        // (e.g., standalone "div") fork at their lone state. Multi-transition
        // sections fork at the last state only.
        let query = &Query::all("div", Save::none()).unwrap().build();
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

        // Single-transition section: the only state IS the section end.
        // Forks for All to enable nested descendant matching.
        let anchored_count = selection.scoped.iter().filter(|c| c.is_anchored()).count();
        assert_eq!(anchored_count, 1, "Expected 1 anchored fork after div match");

        let anchored = selection.scoped.iter().find(|c| c.is_anchored()).unwrap();
        assert_eq!(anchored.scope_depth, 0);
        assert_eq!(anchored.position.state, TransitionId(0));
    }

    #[test]
    fn test_child_combinator_sibling_rematching() {
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
        let html =
            r#"<section><div class="product"><h1>P1</h1><img src="p1.png" /><p>Desc</p></div></section>"#;
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
        let query = &[Query::all("body div ul li a", Save::all())
            .unwrap()
            .build()];
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
        // BUG: first selections in a then() callback used to act like all selections.
        // This test ensures that .first() inside .then() only matches one element.
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

        // Access children through parent element
        let articles: Vec<_> = store.get("article").unwrap().collect();
        assert_eq!(articles.len(), 1);

        let h1s: Vec<_> = articles[0].get(&store, "h1").unwrap().collect();
        assert_eq!(h1s.len(), 1, "first('h1') should match only one h1, not all");
        assert_eq!(h1s[0].name, "h1");

        let links: Vec<_> = articles[0].get(&store, "a[href]").unwrap().collect();
        assert_eq!(links.len(), 2, "all('a[href]') should match all links");
    }

    #[test]
    fn test_store_push_then_pattern() {
        // Verify that store.push links child queries correctly through parent.
        let query = &Query::all("div", Save::none())
            .unwrap()
            .then(|div| Ok([div.first("p", Save::all())?]))
            .unwrap()
            .build();

        let mut store = Store::default();

        // Push the root element (div)
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

        // Push the child element (p) with div as parent
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

        // Child queries are accessed through the parent element, not store.get directly
        let divs: Vec<_> = store.get("div").unwrap().collect();
        let div = divs[0];
        let ps: Vec<_> = div.get(&store, "p").unwrap().collect();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].name, "p");
    }

    #[test]
    fn test_then_single_first_child_direct_executor() {
        // Direct executor test: verify that first() in then() saves elements correctly.
        let query = &Query::all("div", Save::none())
            .unwrap()
            .then(|div| Ok([div.first("p", Save::all())?]))
            .unwrap()
            .build();

        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        // Match <div>
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

        // Verify root cursor advanced to child section (p)
        assert_eq!(selection.root.position.selection, QuerySectionId(1));
        assert!(store.get("div").is_some(), "div should be in store even with Save::none()");

        // Match first <p>
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
            },
            &mut store,
            &mut save_hits,
        );

        // p is saved as child of div; verify through save_hits and store
        assert!(!save_hits.is_empty(), "p should have save hits");
        assert_eq!(save_hits[0].element_id, ElementId(1));

        // Root cursor should now be in end state
        assert!(selection.root.end(), "root cursor should be at end after matching first p");

        // Second <p> should NOT match (end=true, First section)
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
            },
            &mut store,
            &mut save_hits2,
        );

        assert!(save_hits2.is_empty(), "Second p should NOT be saved");

        // Access children through parent element (after all mutations)
        let divs: Vec<_> = store.get("div").unwrap().collect();
        let div = divs[0];
        let ps: Vec<_> = div.get(&store, "p").unwrap().collect();
        assert_eq!(ps.len(), 1, "Only one p should be saved");
    }

    #[test]
    fn test_then_single_first_child_no_descendant() {
        // Full parser test: one first() child inside then() with nested HTML
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

        // Child queries are accessed through parent element
        let divs: Vec<_> = store.get("div").unwrap().collect();
        assert_eq!(divs.len(), 1, "Should match one div");
        let ps: Vec<_> = divs[0].get(&store, "p").unwrap().collect();
        assert_eq!(ps.len(), 1, "first('p') should match only one <p>");
    }

    #[test]
    fn test_then_multiple_product_cards_first_h1() {
        // Multiple product cards, each with an h1. first() should match one h1 per card.
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
}
