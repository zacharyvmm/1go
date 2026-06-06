use super::cursor::{ScopedCursor, SENTINEL_SCOPE};
use super::multiplexer::{DocumentPosition, SaveHit};
#[cfg(any(debug_assertions, test))]
use crate::debug::{CursorTraceKind, ScopedCursorReason, TraceEvent, TransitionRejectReason};
use crate::store::ElementId;
use crate::store::Store;
use crate::{Position, QuerySectionId, QuerySpec, SelectionKind, TransitionId, XHtmlElement};

/// The `QueryExecutor` is an NFA execution engine optimized for streaming StAX events.
///
/// ## Spawn Model
/// A single unified `cursors` vec replaces the old root+scoped split.
/// - The cursor at index 0 is the **root** (SENTINEL scope, never pruned).
///   It never advances its own position — state advancement is expressed by
///   spawning new MOVING cursors.
/// - **MOVING** cursors: own one `last_match_depth` (immutable). They evaluate
///   transitions and spawn children for each next position.
/// - **ANCHORED** cursors: fixed at a scope depth. When they match, they spawn
///   MOVING cursors that advance the query. The anchored cursor stays.
/// - **Pruning** in `back()`: non-SENTINEL cursors are pruned when
///   `scope_depth >= close_depth`. SENTINEL cursors react when
///   `effective_last_depth() == close_depth`.
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

    /// Produce a `CursorTraceKind` for the cursor at the given index.
    /// Index 0 with SENTINEL scope is `Root`; all others are `Scoped`.
    #[cfg(any(debug_assertions, test))]
    fn trace_kind(&self, index: usize) -> CursorTraceKind {
        if index == 0 && self.cursors[0].scope_depth == SENTINEL_SCOPE {
            CursorTraceKind::Root
        } else {
            CursorTraceKind::Scoped { index }
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

        for i in 0..snapshot_len {
            if self.cursors[i].end() {
                continue;
            }

            let position = self.cursors[i].position;
            let matched = self.cursors[i].next(self.query, depth, element);

            if !matched {
                #[cfg(any(debug_assertions, test))]
                {
                    let last_depth = self.cursors[i].effective_last_depth();
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
                                self.query,
                                &position,
                                depth,
                                last_depth,
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
            let section_kind = self
                .query
                .get_section_selection_kind(position.selection);
            let is_first = matches!(section_kind, SelectionKind::First);
            // Whether the element is self-closing (no interior, no close event)
            let self_closing = element.is_self_closing();
            let terminal_all = is_section_end
                && matches!(section_kind, SelectionKind::All)
                && position.next_child(self.query).is_none();

            // Compute next positions lazily — only needed for non-self-closing
            // elements where we actually spawn children.
            let spawned_positions;

            match &self.cursors[i].mode {
                super::cursor::CursorMode::Moving { .. } => {
                    // Capture the original parent before save (save_element may mutate it).
                    // We restore it after spawning so that the cursor can re-match
                    // future elements with the correct parent.
                    let original_parent = self.cursors[i].parent;

                    // Descendant combinator → anchor fork
                    // (created BEFORE save so the fork captures the original parent)
                    if is_descendant && !terminal_all {
                        let do_fork = if is_section_end {
                            let is_all = matches!(section_kind, SelectionKind::All);
                            let last_save_point = self.query.is_last_save_point(&position);
                            !last_save_point || is_all
                        } else {
                            true
                        };
                        if do_fork {
                            let anchor = self.cursors[i].anchor_clone(depth);
                            #[cfg(any(debug_assertions, test))]
                            {
                                crate::scah_trace!(
                                    store,
                                    TraceEvent::ScopedCursorCreated {
                                        runner_index,
                                        depth,
                                        scope_depth: anchor.scope_depth,
                                        parent: anchor.parent,
                                        selection: anchor.position.selection,
                                        state: anchor.position.state,
                                        reason: ScopedCursorReason::DescendantFork,
                                    }
                                );
                            }
                            self.cursors.push(anchor);
                        }
                    }

                    // Save point — may update the cursor's parent
                    let saved_parent = if is_save_point {
                        save_hits.push(Self::save_element(
                            runner_index,
                            self.query,
                            store,
                            element.clone(),
                            &mut self.cursors[i],
                        ));
                        let sp = self.cursors[i].parent;
                        // Restore the original parent so future matches use the correct one
                        self.cursors[i].parent = original_parent;
                        sp
                    } else {
                        original_parent
                    };

                    if self_closing {
                        // Self-closing: no depth update, no end set, no spawn.
                        // The cursor can re-match subsequent elements at the same depth.
                        continue;
                    }

                    if terminal_all {
                        // End-state `All` cursors have no continuation to spawn.
                        // Keep the cursor fixed at its existing scope so it can
                        // match later siblings/descendants without creating an
                        // anchored cursor that would only be pruned on close.
                        continue;
                    }

                    // Update last_match_depth for back() detection
                    self.cursors[i].set_last_match_depth(depth);

                    // End logic:
                    // - Descendant combinator: the anchored fork handles deeper
                    //   re-matching, so deactivate this cursor now (end=true).
                    //   back() reactivates it for sibling re-matching (All) or
                    //   steps backward for early exit (First).
                    // - Non-descendant (Child/NextSibling): only deactivate at
                    //   section end; the cursor must stay active for sibling
                    //   re-matching at the same depth.
                    if is_descendant || is_section_end {
                        self.cursors[i].set_end(true);
                    }

                    // Spawn continuations
                    spawned_positions = self.cursors[i].next_positions(self.query);
                    for pos in &spawned_positions {
                        self.cursors.push(ScopedCursor::new_moving(
                            depth,
                            saved_parent,
                            *pos,
                        ));
                    }
                }
                super::cursor::CursorMode::Anchored { .. } => {
                    if self_closing {
                        // Self-closing: save if at save point, but no spawn.
                        if is_save_point {
                            let mut base = ScopedCursor::new_moving(
                                depth,
                                self.cursors[i].parent,
                                self.cursors[i].position,
                            );
                            save_hits.push(Self::save_element(
                                runner_index,
                                self.query,
                                store,
                                element.clone(),
                                &mut base,
                            ));
                            // base is not pushed (self-closing → no children)
                        }
                        continue;
                    }

                    // Anchored cursor matched. Compute next positions for spawning
                    // MOVING children.
                    spawned_positions = self.cursors[i].next_positions(self.query);

                    // Save if at save point, tracking the saved parent for children
                    let saved_parent = if is_save_point {
                        let mut base = ScopedCursor::new_moving(
                            depth,
                            self.cursors[i].parent,
                            self.cursors[i].position,
                        );
                        save_hits.push(Self::save_element(
                            runner_index,
                            self.query,
                            store,
                            element.clone(),
                            &mut base,
                        ));
                        base.parent
                    } else {
                        self.cursors[i].parent
                    };

                    // First selection at section end → stop the anchored cursor
                    // from matching again at deeper depths.
                    if is_first && is_section_end {
                        self.cursors[i].set_end(true);
                    }

                    for pos in &spawned_positions {
                        self.cursors.push(ScopedCursor::new_moving(
                            depth,
                            saved_parent,
                            *pos,
                        ));
                    }
                }
            }
        }

    }

    pub fn early_exit(&self) -> bool {
        if let Some(exit_section) = self.query.exit_at_section_end() {
            // The SENTINEL cursor must be at the exit section with end=true,
            // AND all other cursors must also be at end=true (meaning all
            // .then() children have completed and stepped back).
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
        let mut siginificant_close = false;

        // Walk backwards so swap_remove only moves already-visited retained cursors.
        let mut i = self.cursors.len();
        while i > 0 {
            i -= 1;
            let cur = &self.cursors[i];

            if cur.scope_depth == SENTINEL_SCOPE {
                // Root (SENTINEL) cursor — never prune, but may need to react
                if cur.effective_last_depth() == close_depth {
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
                            // All selection with end: reactivate and reset depth
                            self.cursors[i].set_end(false);
                            // Reset last_match_depth to 0 (initial sentinel depth)
                            self.cursors[i].set_last_match_depth(0);
                        }
                    } else {
                        self.cursors[i].position.back(self.query);
                        // Reset last_match_depth for the step backward
                        self.cursors[i].set_last_match_depth(0);
                    }
                    siginificant_close = true;
                }
            } else if cur.scope_depth >= close_depth {
                let pruned = self.cursors.swap_remove(i);
                last_pruned_parent = Some(pruned.parent);
                siginificant_close = true;

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
            } else if cur.is_moving()
                && cur.effective_last_depth() == close_depth
            {
                // Non-SENTINEL Moving cursor: its match depth matches the close.
                // Reset last_match_depth to scope_depth (unwind the match stack)
                // and handle end/reactivation.
                let sd = self.cursors[i].scope_depth;
                if cur.end() {
                    let section_kind = self
                        .query
                        .get_section_selection_kind(cur.position.selection);
                    if matches!(section_kind, SelectionKind::First) {
                        // For First selection, step backward so early_exit can detect
                        self.cursors[i].position.back(self.query);
                    } else {
                        // All: reactivate for next match
                        self.cursors[i].set_end(false);
                    }
                }
                // Always reset last_match_depth (pop the match stack)
                self.cursors[i].set_last_match_depth(sd);
                siginificant_close = true;
            }
        }

        // Parent restoration: set the root (SENTINEL) cursor's parent to the
        // last pruned cursor's parent.
        if let Some(parent) = last_pruned_parent
            && let Some(root) = self.cursors.first_mut()
            && root.scope_depth == SENTINEL_SCOPE
        {
            root.parent = parent;
        }

        siginificant_close
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

        // Root cursor at index 0 stays at state 0
        assert_eq!(selection.cursors[0].position.state, TransitionId(0));
        // Cursors: root (SENTINEL), anchored fork at state 0, spawned MOVING at state 1
        assert_eq!(selection.cursors.len(), 3);
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

        // Build a cursors vec with mixed scope depths
        selection.cursors = vec![
            // Root (SENTINEL) at index 0
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
            },
            &mut store,
        );

        // Cursors with scope < 2 should be retained, plus the SENTINEL root
        let retained = &selection.cursors;
        assert_eq!(retained.len(), 4, "Should retain root + 3 scoped < 2");
        assert!(retained[0].scope_depth == SENTINEL_SCOPE, "Root should be kept");
        assert!(retained[1..].iter().all(|c| c.scope_depth < 2));

        let mut retained_parents: Vec<usize> = retained[1..].iter().map(|c| c.parent.index()).collect();
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
            },
            &mut store,
            &mut Vec::new(),
        );
        store.text_content.set_start(4);

        // Root is still at state 0 (doesn't advance). It should have end=false
        // for All selection.
        assert_eq!(selection.cursors[0].position.state, TransitionId(0));
        // Spawned cursor exists
        assert!(!selection.cursors.is_empty());

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

        assert!(reactivated);
        // Root cursor should be reactivated (end cleared if it was set)
        assert!(!selection.cursors[0].end());
        // Non-sentinel cursors with scope >= 0 are pruned
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
            },
            &mut store,
            &mut Vec::new(),
        );

        let anchored_count = selection
            .cursors
            .iter()
            .filter(|c| c.is_anchored())
            .count();
        assert_eq!(
            anchored_count, 1,
            "Expected 1 anchored fork after div match"
        );

        let anchored = selection.cursors.iter().find(|c| c.is_anchored()).unwrap();
        assert_eq!(anchored.scope_depth, 0);
        assert_eq!(anchored.position.state, TransitionId(0));
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
            },
            &mut store,
            &mut Vec::new(),
        );

        // Root cursor stays at section 0
        assert_eq!(
            selection.cursors[0].position.selection,
            QuerySectionId(0)
        );
        assert!(
            store.get("div").is_some(),
            "div should be in store even with Save::none()"
        );

        // Should have spawned cursors for .then() children (one for first("p"))
        let p_cursors: Vec<_> = selection
            .cursors
            .iter()
            .filter(|c| c.position.selection == QuerySectionId(1))
            .collect();
        assert!(!p_cursors.is_empty(), "Should have spawned cursor for p section");

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

        assert!(!save_hits.is_empty(), "p should have save hits");
        assert_eq!(save_hits[0].element_id, ElementId(1));

        // The p cursor (First) should now have end=true
        let p_cursor = selection
            .cursors
            .iter()
            .find(|c| c.position.selection == QuerySectionId(1) && c.is_moving())
            .unwrap();
        assert!(p_cursor.end(), "First p cursor should be at end after match");

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

    // --- New spawn-model-specific tests ---

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
            },
            &mut store,
            &mut Vec::new(),
        );

        // Root cursor (index 0) must NOT advance position.
        // The transition at state 0 is Descendant, so the root cursor
        // deactivates (end=true) after spawning — the anchored fork handles
        // deeper re-matching. back() reactivates it for sibling matching.
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
        // Tests that All selection can match multiple elements
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
        // Tests that First selection sets end=true and stops matching
        let html = "<div><span>A</span><span>B</span></div>";
        let reader = &mut Reader::new(html);
        let query = &[Query::first("div span", Save::all()).unwrap().build()];
        let manager = QueryMultiplexer::new(query);
        let mut parser = XHtmlParser::new(manager);
        while parser.next(reader) {}
        let store = parser.matches();

        let spans: Vec<_> = store.get("div span").unwrap().collect();
        assert_eq!(
            spans.len(),
            1,
            "First selection should match only 1 span"
        );
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

        // Close at depth 0 — non-sentinel should be pruned, sentinel stays
        let _ = selection.back(
            0,
            "div",
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 0,
            },
            &mut store,
        );

        assert_eq!(selection.cursors.len(), 1, "Only root should remain");
        assert_eq!(
            selection.cursors[0].scope_depth,
            SENTINEL_SCOPE,
            "Root cursor must not be pruned"
        );
    }

    #[test]
    fn test_spawn_model_early_exit_first_selection() {
        let query = &Query::first("div", Save::all()).unwrap().build();
        let mut store = Store::default();
        let mut selection = QueryExecutor::new(query);

        // Match div — root (SENTINEL) cursor should set end=true for First
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

        // After matching, early_exit should be true (root at exit section with end=true)
        assert!(
            selection.early_exit(),
            "early_exit should be true after First match completes"
        );

        // On close, back() should return true (SENTINEL reacts)
        store.text_content.set_start(4);
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
            },
            &mut store,
            &mut Vec::new(),
        );

        // Should have spawned 3 MOVING cursors (one per sibling)
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
        assert!(selections.contains(&QuerySectionId(1)), "h1 section missing");
        assert!(selections.contains(&QuerySectionId(2)), "p section missing");
        assert!(selections.contains(&QuerySectionId(3)), "span section missing");
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

        // Create cursors with various scope depths
        selection.cursors = vec![
            ScopedCursor::new_root(ElementId(0), position),
            anchored_cursor(1, ElementId(10), position),
            anchored_cursor(2, ElementId(20), position),
            anchored_cursor(3, ElementId(30), position),
            anchored_cursor(0, ElementId(40), position),
        ];

        // Close at depth 2 — cursors with scope >= 2 should be pruned
        let _ = selection.back(
            0,
            "div",
            &DocumentPosition {
                reader_position: 0,
                text_content_position: 0,
                element_depth: 2,
            },
            &mut store,
        );

        let remaining_scopes: Vec<_> = selection
            .cursors
            .iter()
            .filter(|c| c.scope_depth != SENTINEL_SCOPE)
            .map(|c| c.scope_depth)
            .collect();

        // scope 0 and 1 remain; scope 2 and 3 are pruned
        assert_eq!(remaining_scopes.len(), 2);
        assert!(remaining_scopes.contains(&0));
        assert!(remaining_scopes.contains(&1));
        assert!(!remaining_scopes.contains(&2));
        assert!(!remaining_scopes.contains(&3));
    }
}
