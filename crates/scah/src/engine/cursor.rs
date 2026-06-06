use std::fmt::Debug;

use crate::{Position, QuerySpec, XHtmlElement};
use smallvec::SmallVec;

use crate::store::ElementId;

/// Sentinel scope depth for root cursors that must never be pruned.
/// Root cursors use `DepthSize::MAX` so the pruning condition
/// `scope_depth >= close_depth` never fires for them.
pub const SENTINEL_SCOPE: super::DepthSize = super::DepthSize::MAX;

/// The operational mode of a [`ScopedCursor`].
///
/// In the spawn model, a cursor never advances through more than one state
/// transition. State advancement is expressed by spawning new MOVING cursors.
#[derive(PartialEq, Clone, Debug)]
pub enum CursorMode {
    Moving {
        /// Depth of the single match that created this cursor.
        /// Immutable after construction.
        last_match_depth: super::DepthSize,
        /// True when this cursor has completed its match and should
        /// not evaluate further transitions (e.g. First selection).
        end: bool,
    },
    Anchored {
        end: bool,
    },
}

/// A cursor that either advances through matches or remains anchored at a scope.
#[derive(PartialEq, Clone, Debug)]
pub struct ScopedCursor {
    /// Depth at which this cursor was spawned (or SENTINEL_SCOPE for root).
    pub scope_depth: super::DepthSize,
    pub parent: ElementId,
    pub position: Position,
    pub mode: CursorMode,
}

impl ScopedCursor {
    /// Create a new MOVING cursor with an explicit last_match_depth.
    pub fn new_moving(
        scope_depth: super::DepthSize,
        parent: ElementId,
        position: Position,
    ) -> Self {
        Self {
            scope_depth,
            parent,
            position,
            mode: CursorMode::Moving {
                last_match_depth: scope_depth,
                end: false,
            },
        }
    }

    /// Create a new MOVING cursor with explicit last_match_depth.
    #[cfg(test)]
    pub fn new_moving_with_last(
        scope_depth: super::DepthSize,
        parent: ElementId,
        position: Position,
        last_match_depth: super::DepthSize,
    ) -> Self {
        Self {
            scope_depth,
            parent,
            position,
            mode: CursorMode::Moving {
                last_match_depth,
                end: false,
            },
        }
    }

    /// Create the root MOVING cursor (sentinel scope, never pruned).
    pub fn new_root(parent: ElementId, position: Position) -> Self {
        Self {
            scope_depth: SENTINEL_SCOPE,
            parent,
            position,
            mode: CursorMode::Moving {
                last_match_depth: 0,
                end: false,
            },
        }
    }

    #[cfg(test)]
    pub fn new_anchored(
        scope_depth: super::DepthSize,
        parent: ElementId,
        position: Position,
    ) -> Self {
        Self {
            scope_depth,
            parent,
            position,
            mode: CursorMode::Anchored { end: false },
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_moving(&self) -> bool {
        matches!(self.mode, CursorMode::Moving { .. })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_anchored(&self) -> bool {
        matches!(self.mode, CursorMode::Anchored { .. })
    }

    /// Effective `last_depth` for combinator evaluation.
    /// - Moving: returns `last_match_depth`
    /// - Anchored: returns `scope_depth`
    pub fn effective_last_depth(&self) -> super::DepthSize {
        match &self.mode {
            CursorMode::Moving {
                last_match_depth, ..
            } => *last_match_depth,
            CursorMode::Anchored { .. } => self.scope_depth,
        }
    }

    pub fn end(&self) -> bool {
        match &self.mode {
            CursorMode::Moving { end, .. } => *end,
            CursorMode::Anchored { end } => *end,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn last_match_depth(&self) -> super::DepthSize {
        self.effective_last_depth()
    }

    /// Spawn a new MOVING cursor continuing from `next_position`.
    /// `at_depth` is the depth of the element that triggered the spawn.
    #[cfg(test)]
    pub fn spawn_moving(&self, at_depth: super::DepthSize, next_position: Position) -> Self {
        Self {
            scope_depth: at_depth,
            parent: self.parent,
            position: next_position,
            mode: CursorMode::Moving {
                last_match_depth: at_depth,
                end: false,
            },
        }
    }

    /// Spawn an ANCHORED fork at the current position (descendant combinator).
    pub fn anchor_clone(&self, depth: super::DepthSize) -> Self {
        Self {
            scope_depth: depth,
            parent: self.parent,
            position: self.position,
            mode: CursorMode::Anchored { end: false },
        }
    }
}

impl<'query> ScopedCursor {
    /// Evaluate whether the transition at this cursor's state matches the element.
    pub fn next<'html, Q: QuerySpec<'query>>(
        &self,
        tree: &Q,
        depth: super::DepthSize,
        element: &XHtmlElement<'html>,
    ) -> bool {
        if self.end() {
            return false;
        }
        let fsm = tree.get_transition(self.position.state);
        fsm.next(element, depth, self.effective_last_depth())
    }

    pub fn get_position(&self) -> &Position {
        &self.position
    }

    pub fn get_parent(&self) -> ElementId {
        self.parent
    }

    pub fn set_parent(&mut self, value: ElementId) {
        self.parent = value;
    }

    pub fn set_end(&mut self, end: bool) {
        match &mut self.mode {
            CursorMode::Moving { end: e, .. } => *e = end,
            CursorMode::Anchored { end: e } => *e = end,
        }
    }

    /// Update the last_match_depth (Moving mode only).
    /// Called when a Moving cursor successfully matches an element at the given depth.
    pub fn set_last_match_depth(&mut self, depth: super::DepthSize) {
        if let CursorMode::Moving {
            last_match_depth, ..
        } = &mut self.mode
        {
            *last_match_depth = depth;
        }
    }

    /// The set of positions that should be explored after a match at `self.position`.
    /// Returns 0–N positions:
    /// - 1 if there is a next transition within the same section
    /// - N if the cursor is at the end of a section with `.then()` children
    ///   (one position per child section, including siblings)
    /// - 0 if end of path (no next transition and no child sections)
    pub fn next_positions<Q: QuerySpec<'query> + ?Sized>(
        &self,
        tree: &Q,
    ) -> SmallVec<[Position; 4]> {
        let mut positions = SmallVec::new();
        if let Some(next) = self.position.next_transition(tree) {
            positions.push(Position {
                state: next,
                selection: self.position.selection,
            });
        } else {
            // End of section — check for child sections (.then())
            let mut child = self.position.next_child(tree);
            while let Some(c) = child {
                positions.push(c);
                child = c.next_sibling(tree);
            }
        }
        positions
    }
}

#[cfg(test)]
mod tests {
    use super::{CursorMode, SENTINEL_SCOPE, ScopedCursor};
    use crate::html::element::builder::XHtmlElement;
    use crate::store::ElementId;
    use crate::{Position, Query, QuerySectionId, Save, TransitionId};

    const NULL_PARENT: ElementId = ElementId(usize::MAX);

    fn root_cursor() -> ScopedCursor {
        ScopedCursor::new_root(
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
        )
    }

    #[test]
    fn test_unified_cursor_next_descendant() {
        let query = Query::all("div a", Save::none()).unwrap().build();
        let mut state = root_cursor();

        let matched = state.next(
            &query,
            0,
            &XHtmlElement {
                name: "div",
                id: None,
                class: None,
                attributes: &[],
            },
        );
        assert!(matched);

        // advance to next transition
        let position = state.position.next_transition(&query);
        state.position.state = position.unwrap();

        let matched = state.next(
            &query,
            1,
            &XHtmlElement {
                name: "a",
                id: None,
                class: None,
                attributes: &[],
            },
        );
        assert!(matched);
    }

    #[test]
    fn test_moving_cursor_set_end() {
        let mut cursor = root_cursor();
        assert!(!cursor.end());

        cursor.set_end(true);
        assert!(cursor.end());

        cursor.set_end(false);
        assert!(!cursor.end());
    }

    #[test]
    fn test_anchored_cursor_set_end() {
        let mut cursor = ScopedCursor::new_anchored(
            0,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
        );
        assert!(!cursor.end());
        cursor.set_end(true);
        assert!(cursor.end());
    }

    #[test]
    fn test_spawn_moving_preserves_parent() {
        let root = root_cursor();
        let spawned = root.spawn_moving(
            5,
            Position {
                selection: QuerySectionId(1),
                state: TransitionId(2),
            },
        );
        assert!(spawned.is_moving());
        assert_eq!(spawned.scope_depth, 5);
        assert_eq!(spawned.parent, root.parent);
        assert_eq!(
            spawned.position,
            Position {
                selection: QuerySectionId(1),
                state: TransitionId(2),
            }
        );
        assert_eq!(spawned.last_match_depth(), 5);
        assert!(!spawned.end());
    }

    #[test]
    fn test_anchor_clone() {
        let moving = root_cursor();
        let anchored = moving.anchor_clone(5);
        assert!(anchored.is_anchored());
        assert_eq!(anchored.scope_depth, 5);
        assert_eq!(anchored.parent, moving.parent);
        assert_eq!(anchored.position, moving.position);
    }

    #[test]
    fn test_next_positions_single_transition() {
        let query = Query::all("div a", Save::none()).unwrap().build();
        let cursor = root_cursor(); // state 0 in a 2-state section

        let positions = cursor.next_positions(&query);
        assert_eq!(positions.len(), 1);
        assert_eq!(
            positions[0],
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(1),
            }
        );
    }

    #[test]
    fn test_next_positions_end_of_path() {
        // Single-state query: all("div")
        let query = Query::all("div", Save::none()).unwrap().build();
        let cursor = root_cursor(); // state 0, section end

        let positions = cursor.next_positions(&query);
        assert!(positions.is_empty());
    }

    #[test]
    fn test_next_positions_then_children() {
        let query = Query::all("div", Save::none())
            .unwrap()
            .then(|div| Ok([div.first("h1", Save::all())?, div.all("p", Save::all())?]))
            .unwrap()
            .build();

        // Cursor at state 0 (end of section 0, which has .then() children)
        let mut cursor = root_cursor();
        cursor.position.state = TransitionId(0);

        // Advance cursor to the end of section 0 (section 0 has 1 state)
        // state 0 is already the save point for section 0
        let positions = cursor.next_positions(&query);
        assert_eq!(positions.len(), 2, "Should have two .then() children");

        // Should have positions for h1 (section 1) and p (section 2)
        let selections: Vec<_> = positions.iter().map(|p| p.selection).collect();
        assert!(
            selections.contains(&QuerySectionId(1)),
            "Should include h1 section"
        );
        assert!(
            selections.contains(&QuerySectionId(2)),
            "Should include p section"
        );
    }

    #[test]
    fn test_sentinel_scope_is_max() {
        assert_eq!(SENTINEL_SCOPE, u16::MAX);
        let root = root_cursor();
        assert_eq!(root.scope_depth, SENTINEL_SCOPE);
    }

    #[test]
    fn test_root_cursor_effective_last_depth() {
        let root = root_cursor();
        assert_eq!(root.effective_last_depth(), 0);
    }

    #[test]
    fn test_moving_cursor_effective_last_depth() {
        let cursor = ScopedCursor::new_moving_with_last(
            3,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
            7, // explicit last_match_depth
        );
        assert_eq!(cursor.effective_last_depth(), 7);
    }

    #[test]
    fn test_anchored_cursor_effective_last_depth() {
        let cursor = ScopedCursor::new_anchored(
            3,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
        );
        assert_eq!(cursor.effective_last_depth(), 3); // uses scope_depth
    }

    #[test]
    fn test_new_moving_with_last() {
        let cursor = ScopedCursor::new_moving_with_last(
            5, // scope_depth
            NULL_PARENT,
            Position {
                selection: QuerySectionId(1),
                state: TransitionId(3),
            },
            2, // last_match_depth (different from scope)
        );
        assert!(cursor.is_moving());
        assert_eq!(cursor.scope_depth, 5);
        match &cursor.mode {
            CursorMode::Moving {
                last_match_depth,
                end,
            } => {
                assert_eq!(*last_match_depth, 2);
                assert!(!end);
            }
            _ => panic!("expected Moving"),
        }
    }
}
