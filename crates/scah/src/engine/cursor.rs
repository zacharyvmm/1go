use std::fmt::Debug;

use crate::{Position, QuerySpec, XHtmlElement};
use smallvec::SmallVec;

use crate::store::ElementId;

/// Scope depth reserved for the root cursor, which is handled separately from
/// depth-scoped cursor pruning.
pub const SENTINEL_SCOPE: super::DepthSize = super::DepthSize::MAX;

/// The operational mode of a [`ScopedCursor`].
///
/// Moving cursors represent query progress. Anchored cursors stay at a
/// descendant-search scope and can spawn moving continuations when they match.
#[derive(PartialEq, Clone, Debug)]
pub enum CursorMode {
    Moving {
        /// The Dᵢ value used to evaluate the current transition.
        match_base_depth: super::DepthSize,
        /// The matched element whose close should update this cursor.
        /// None when no close callback is pending.
        unwind_depth: Option<super::DepthSize>,
        /// Prevents a completed cursor from matching again until close handling
        /// either reactivates it or steps it back.
        end: bool,
    },
    Anchored {
        end: bool,
    },
}

/// A cursor that either advances through matches or remains anchored at a scope.
#[derive(PartialEq, Clone, Debug)]
pub struct ScopedCursor {
    /// Scope that bounds this cursor's lifetime; `SENTINEL_SCOPE` marks root.
    pub scope_depth: super::DepthSize,
    pub parent: ElementId,
    pub position: Position,
    pub mode: CursorMode,
}

impl ScopedCursor {
    pub fn new_moving(
        scope_depth: super::DepthSize,
        parent: ElementId,
        position: Position,
    ) -> Self {
        Self::new_moving_with_match_base(scope_depth, scope_depth, parent, position)
    }

    pub fn new_moving_with_match_base(
        scope_depth: super::DepthSize,
        match_base_depth: super::DepthSize,
        parent: ElementId,
        position: Position,
    ) -> Self {
        Self {
            scope_depth,
            parent,
            position,
            mode: CursorMode::Moving {
                match_base_depth,
                unwind_depth: None,
                end: false,
            },
        }
    }

    #[cfg(test)]
    pub fn new_moving_with_last(
        scope_depth: super::DepthSize,
        parent: ElementId,
        position: Position,
        match_base_depth: super::DepthSize,
    ) -> Self {
        Self {
            scope_depth,
            parent,
            position,
            mode: CursorMode::Moving {
                match_base_depth,
                unwind_depth: None,
                end: false,
            },
        }
    }

    pub fn new_root(parent: ElementId, position: Position) -> Self {
        Self {
            scope_depth: SENTINEL_SCOPE,
            parent,
            position,
            mode: CursorMode::Moving {
                match_base_depth: 0,
                unwind_depth: None,
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

    /// Depth used by combinators when deciding whether this cursor may match.
    pub fn match_base_depth(&self) -> super::DepthSize {
        match &self.mode {
            CursorMode::Moving {
                match_base_depth, ..
            } => *match_base_depth,
            CursorMode::Anchored { .. } => self.scope_depth,
        }
    }

    /// Depth whose close should reactivate this cursor, if any.
    pub fn unwind_depth(&self) -> Option<super::DepthSize> {
        match &self.mode {
            CursorMode::Moving { unwind_depth, .. } => *unwind_depth,
            CursorMode::Anchored { .. } => None,
        }
    }

    /// Depth used by combinators when deciding whether this cursor may match.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn effective_last_depth(&self) -> super::DepthSize {
        self.match_base_depth()
    }

    pub fn end(&self) -> bool {
        match &self.mode {
            CursorMode::Moving { end, .. } => *end,
            CursorMode::Anchored { end } => *end,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn last_match_depth(&self) -> super::DepthSize {
        self.match_base_depth()
    }

    #[cfg(test)]
    pub fn spawn_moving(&self, at_depth: super::DepthSize, next_position: Position) -> Self {
        Self {
            scope_depth: at_depth,
            parent: self.parent,
            position: next_position,
            mode: CursorMode::Moving {
                match_base_depth: at_depth,
                unwind_depth: None,
                end: false,
            },
        }
    }

    pub fn anchor_clone(&self, depth: super::DepthSize) -> Self {
        Self {
            scope_depth: depth,
            parent: self.parent,
            position: self.position,
            mode: CursorMode::Anchored { end: false },
        }
    }

    /// Record which element close should notify this cursor, without blocking.
    pub fn set_unwind_depth(&mut self, depth: super::DepthSize) {
        if let CursorMode::Moving { unwind_depth, .. } = &mut self.mode {
            *unwind_depth = Some(depth);
        }
    }

    /// Pause matching until the element at `depth` closes.
    pub fn block_until_close(&mut self, depth: super::DepthSize) {
        if let CursorMode::Moving {
            end,
            unwind_depth,
            ..
        } = &mut self.mode
        {
            *end = true;
            *unwind_depth = Some(depth);
        }
    }

    /// Resume matching after the blocked element closes.
    pub fn reactivate_after_close(&mut self) {
        if let CursorMode::Moving {
            end,
            unwind_depth,
            ..
        } = &mut self.mode
        {
            *end = false;
            *unwind_depth = None;
        }
    }

    /// Mark this cursor complete (e.g. after a `First` terminal match).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn mark_complete(&mut self) {
        self.set_end(true);
    }
}

impl<'query> ScopedCursor {
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
        fsm.next(element, depth, self.match_base_depth())
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
}

impl<'query> ScopedCursor {
    /// Continuations to spawn after matching at the current position.
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
        assert_eq!(spawned.match_base_depth(), 5);
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
        let cursor = root_cursor();

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
        let query = Query::all("div", Save::none()).unwrap().build();
        let cursor = root_cursor();

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

        let mut cursor = root_cursor();
        cursor.position.state = TransitionId(0);

        let positions = cursor.next_positions(&query);
        assert_eq!(positions.len(), 2, "Should have two .then() children");

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
    fn test_root_cursor_match_base_depth() {
        let root = root_cursor();
        assert_eq!(root.match_base_depth(), 0);
        assert_eq!(root.unwind_depth(), None);
    }

    #[test]
    fn test_moving_cursor_match_base_depth() {
        let cursor = ScopedCursor::new_moving_with_last(
            3,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
            7,
        );
        assert_eq!(cursor.match_base_depth(), 7);
        assert_eq!(cursor.unwind_depth(), None);
    }

    #[test]
    fn test_anchored_cursor_match_base_depth() {
        let cursor = ScopedCursor::new_anchored(
            3,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
        );
        assert_eq!(cursor.match_base_depth(), 3);
        assert_eq!(cursor.unwind_depth(), None);
    }

    #[test]
    fn test_block_until_close_and_reactivate() {
        let mut cursor = ScopedCursor::new_moving_with_last(
            5,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
            2,
        );
        cursor.block_until_close(4);
        assert!(cursor.end());
        assert_eq!(cursor.unwind_depth(), Some(4));
        assert_eq!(cursor.match_base_depth(), 2);

        cursor.reactivate_after_close();
        assert!(!cursor.end());
        assert_eq!(cursor.unwind_depth(), None);
        assert_eq!(cursor.match_base_depth(), 2);
    }

    #[test]
    fn test_new_moving_with_last() {
        let cursor = ScopedCursor::new_moving_with_last(
            5,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(1),
                state: TransitionId(3),
            },
            2,
        );
        assert!(cursor.is_moving());
        assert_eq!(cursor.scope_depth, 5);
        match &cursor.mode {
            CursorMode::Moving {
                match_base_depth,
                unwind_depth,
                end,
            } => {
                assert_eq!(*match_base_depth, 2);
                assert_eq!(*unwind_depth, None);
                assert!(!end);
            }
            _ => panic!("expected Moving"),
        }
    }
}
