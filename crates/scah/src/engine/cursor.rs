use std::fmt::Debug;

use crate::{Position, QuerySpec, TransitionId, XHtmlElement};
use smallvec::SmallVec;

use crate::store::ElementId;

/// The operational mode of a [`ScopedCursor`].
///
/// - **Moving**: Actively advances its position on each match.
///   Uses a match stack to track nesting depths (like the old `Cursor`).
/// - **Anchored**: Fixed at a specific `scope_depth`. Never advances position;
///   only spawns moving clones when it matches a transition.
///   The `end` flag is used to mark the cursor as consumed (e.g., after first
///   match for a `SelectionKind::First` section).
#[derive(PartialEq, Clone, Debug)]
pub enum CursorMode {
    Moving {
        /// Stack of element depths, used for combinator evaluation.
        /// Last element is the `last_depth` for the current transition.
        match_stack: SmallVec<[super::DepthSize; 10]>,
        end: bool,
    },
    Anchored {
        /// When `true`, this anchored cursor has been consumed (e.g., matched
        /// for a `First` selection) and should not match again.
        end: bool,
    },
}

/// A single unified cursor that replaces the old `Cursor` + `ScopedCursor` split.
///
/// Every cursor has an immutable `scope_depth` (the depth at which it was anchored,
/// or 0 for the root cursor). The `mode` determines whether the cursor moves
/// forward on each match (`Moving`) or stays fixed and spawns children (`Anchored`).
#[derive(PartialEq, Clone, Debug)]
pub struct ScopedCursor {
    /// Immutable for the lifetime of this cursor.
    /// - Root: 0.
    /// - Anchored cursors: depth at which anchoring occurred.
    pub scope_depth: super::DepthSize,
    pub parent: ElementId,
    pub position: Position,
    pub mode: CursorMode,
    /// Cached effective last depth for combinator evaluation.
    /// Avoids enum match + Option unwrap in the hot `next()` path.
    /// - Moving: `match_stack.last().unwrap_or(scope_depth)`
    /// - Anchored: `scope_depth` (never changes)
    pub last_depth: super::DepthSize,
}

impl ScopedCursor {
    /// Create a new MOVING cursor.
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
                match_stack: SmallVec::new(),
                end: false,
            },
            last_depth: scope_depth,
        }
    }

    /// Create a new ANCHORED cursor.
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
            last_depth: scope_depth,
        }
    }

    /// Returns `true` if this cursor is in `Moving` mode.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_moving(&self) -> bool {
        matches!(self.mode, CursorMode::Moving { .. })
    }

    /// Returns `true` if this cursor is in `Anchored` mode.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_anchored(&self) -> bool {
        matches!(self.mode, CursorMode::Anchored { .. })
    }

    /// Returns the effective `last_depth` used for combinator evaluation.
    /// Uses the cached `last_depth` field — zero-overhead in hot path.
    pub fn effective_last_depth(&self) -> super::DepthSize {
        self.last_depth
    }

    /// Returns the `end` flag for both Moving and Anchored cursors.
    pub fn end(&self) -> bool {
        match &self.mode {
            CursorMode::Moving { end, .. } => *end,
            CursorMode::Anchored { end } => *end,
        }
    }

    /// For Moving cursors: returns `last_match_depth`.
    /// For Anchored cursors: returns `scope_depth`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn last_match_depth(&self) -> super::DepthSize {
        self.last_depth
    }

    /// Clone this cursor, converting an Anchored cursor into a Moving cursor.
    ///
    /// The new Moving cursor takes `scope_depth = current_depth` (the depth
    /// at which the anchored cursor matched) and sets an empty match_stack.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn spawn_moving(&self, current_depth: super::DepthSize) -> Self {
        Self {
            scope_depth: current_depth,
            parent: self.parent,
            position: self.position,
            mode: CursorMode::Moving {
                match_stack: SmallVec::new(),
                end: false,
            },
            last_depth: current_depth,
        }
    }

    /// Create an ANCHORED clone at the given `depth`.
    ///
    /// Used when a Moving cursor encounters a Descendant combinator match —
    /// the anchored clone stays at the current position to match deeper descendants.
    pub fn anchor_clone(&self, depth: super::DepthSize) -> Self {
        Self {
            scope_depth: depth,
            parent: self.parent,
            position: self.position,
            mode: CursorMode::Anchored { end: false },
            last_depth: depth,
        }
    }
}

impl ScopedCursor {
    /// Evaluate the transition at the cursor's current position against `element`.
    pub fn next<'query, 'html, Q: QuerySpec<'query>>(
        &self,
        tree: &Q,
        depth: super::DepthSize,
        element: &XHtmlElement<'html>,
    ) -> bool {
        if self.end() {
            return false;
        }
        let fsm = tree.get_transition(self.position.state);
        fsm.next(element, depth, self.last_depth)
    }

    /// Step backward: for Moving cursors, rewinds position and pops match_stack.
    /// For Anchored cursors, this is a no-op.
    pub fn step_backward<'query, Q: QuerySpec<'query>>(&mut self, tree: &Q) {
        if let CursorMode::Moving {
            ref mut match_stack,
            ..
        } = self.mode
        {
            match_stack.pop();
            self.last_depth = *match_stack.last().unwrap_or(&self.scope_depth);
            self.position.back(tree);
        }
    }

    pub fn get_position(&self) -> &Position {
        &self.position
    }

    pub fn set_position(&mut self, value: Position) {
        self.position = value;
    }

    pub fn set_state(&mut self, value: TransitionId) {
        self.position.state = value;
    }

    pub fn get_parent(&self) -> ElementId {
        self.parent
    }

    pub fn set_parent(&mut self, value: ElementId) {
        self.parent = value;
    }

    /// Set the `end` flag on Moving cursors. No-op for Anchored.
    pub fn set_end(&mut self, end: bool) {
        match &mut self.mode {
            CursorMode::Moving { end: e, .. } => *e = end,
            CursorMode::Anchored { end: e } => *e = end,
        }
    }

    /// Record a depth match. For Moving cursors, pushes onto match_stack.
    /// For Anchored cursors, this is a no-op.
    pub fn add_depth(&mut self, depth: super::DepthSize) {
        if let CursorMode::Moving {
            ref mut match_stack,
            ..
        } = self.mode
        {
            match_stack.push(depth);
            self.last_depth = depth;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CursorMode, ScopedCursor};
    use crate::html::element::builder::XHtmlElement;
    use crate::store::ElementId;
    use crate::{Position, Query, QuerySectionId, Save, TransitionId};

    const NULL_PARENT: ElementId = ElementId(usize::MAX);

    fn root_cursor() -> ScopedCursor {
        ScopedCursor::new_moving(
            0,
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
    fn test_moving_cursor_add_depth_pushes_to_stack() {
        let mut cursor = root_cursor();
        assert_eq!(cursor.last_match_depth(), 0); // empty stack -> scope_depth

        cursor.add_depth(3);
        assert_eq!(cursor.last_match_depth(), 3); // stack top

        cursor.add_depth(5);
        assert_eq!(cursor.last_match_depth(), 5);
    }

    #[test]
    fn test_anchored_cursor_add_depth_is_noop() {
        let mut cursor = ScopedCursor::new_anchored(
            2,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
        );
        assert_eq!(cursor.last_match_depth(), 2);

        cursor.add_depth(5);
        // Anchored: returns scope_depth, unchanged
        assert_eq!(cursor.last_match_depth(), 2);
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
        // Anchored cursors now support the end flag (used to consume First selections)
        assert!(cursor.end());
    }

    #[test]
    fn test_into_moving_from_anchored() {
        let anchored = ScopedCursor::new_anchored(
            3,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(1),
                state: TransitionId(2),
            },
        );
        assert!(anchored.is_anchored());

        let moving = anchored.spawn_moving(5);
        assert!(moving.is_moving());
        // scope_depth = current_depth (5)
        assert_eq!(moving.scope_depth, 5);
        assert_eq!(moving.parent, anchored.parent);
        assert_eq!(moving.position, anchored.position);
        // match_stack is empty, so last_match_depth = scope_depth = 5
        assert_eq!(moving.last_match_depth(), 5);
        assert!(!moving.end());
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
    fn test_moving_cursor_step_backward() {
        let query = Query::all("div a", Save::none()).unwrap().build();
        let mut cursor = ScopedCursor::new_moving(
            0,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(1), // after matching "a" (second transition)
            },
        );
        cursor.add_depth(0);
        assert_eq!(cursor.last_match_depth(), 0);

        cursor.step_backward(&query);
        // After back(), position.state should be 0 (back to first transition)
        assert_eq!(cursor.position.state, TransitionId(0));
        // match_stack was popped
        match &cursor.mode {
            CursorMode::Moving { match_stack, .. } => {
                assert!(match_stack.is_empty());
            }
            _ => panic!("expected Moving"),
        }
    }

    #[test]
    fn test_anchored_cursor_step_backward_is_noop() {
        let query = Query::first("div", Save::none()).unwrap().build();
        let mut cursor = ScopedCursor::new_anchored(
            2,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(1),
            },
        );
        let original_position = cursor.position;
        cursor.step_backward(&query);
        // Anchored: no-op, position unchanged
        assert_eq!(cursor.position, original_position);
    }
}
