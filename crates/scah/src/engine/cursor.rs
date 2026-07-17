use std::fmt::Debug;

use crate::{Position, QuerySpec, XHtmlElement};
use smallvec::SmallVec;

use crate::store::ElementId;

/// Scope depth reserved for the root cursor, which is handled separately from
/// depth-scoped cursor pruning.
pub const SENTINEL_SCOPE: super::DepthSize = super::DepthSize::MAX;

const FLAG_BLOCKED: u8 = 1 << 0;
const FLAG_COMPLETE: u8 = 1 << 1;
const FLAG_HAS_UNWIND: u8 = 1 << 2;

#[inline]
const fn flags_is_active(flags: u8) -> bool {
    flags & (FLAG_BLOCKED | FLAG_COMPLETE) == 0
}

#[inline]
const fn flags_is_blocked(flags: u8) -> bool {
    flags & FLAG_BLOCKED != 0 && flags & FLAG_COMPLETE == 0
}

#[inline]
const fn flags_is_complete(flags: u8) -> bool {
    flags & FLAG_COMPLETE != 0
}

#[inline]
const fn flags_has_unwind(flags: u8) -> bool {
    flags & FLAG_HAS_UNWIND != 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CursorActivity {
    Active,
    Blocked,
    Complete,
}

impl CursorActivity {
    #[inline]
    const fn to_flags(self) -> u8 {
        match self {
            Self::Active => 0,
            Self::Blocked => FLAG_BLOCKED,
            Self::Complete => FLAG_COMPLETE,
        }
    }
}

#[inline]
const fn optional_unwind_depth(flags: u8, raw: super::DepthSize) -> Option<super::DepthSize> {
    if flags_has_unwind(flags) {
        Some(raw)
    } else {
        None
    }
}

/// The operational mode of a [`ScopedCursor`].
///
/// Moving cursors represent query progress. Anchored cursors stay at a
/// descendant-search scope and can spawn moving continuations when they match.
#[derive(PartialEq, Clone, Debug)]
pub enum CursorMode {
    Moving {
        /// The Dᵢ value used to evaluate the current transition.
        match_base_depth: super::DepthSize,
        /// Depth whose close should update this cursor when [`FLAG_HAS_UNWIND`] is set.
        unwind_depth: super::DepthSize,
        /// Activity, unwind-presence, and mode flags; see [`FLAG_BLOCKED`],
        /// [`FLAG_COMPLETE`], and [`FLAG_HAS_UNWIND`].
        flags: u8,
    },
    Anchored {
        flags: u8,
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
                unwind_depth: 0,
                flags: CursorActivity::Active.to_flags(),
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
                unwind_depth: 0,
                flags: CursorActivity::Active.to_flags(),
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
                unwind_depth: 0,
                flags: CursorActivity::Active.to_flags(),
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
            mode: CursorMode::Anchored {
                flags: CursorActivity::Active.to_flags(),
            },
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
            CursorMode::Moving {
                flags,
                unwind_depth,
                ..
            } => optional_unwind_depth(*flags, *unwind_depth),
            CursorMode::Anchored { .. } => None,
        }
    }

    fn activity_flags(&self) -> u8 {
        match &self.mode {
            CursorMode::Moving { flags, .. } | CursorMode::Anchored { flags } => *flags,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn set_activity_flags(&mut self, flags: u8) {
        match &mut self.mode {
            CursorMode::Moving { flags: f, .. } | CursorMode::Anchored { flags: f } => *f = flags,
        }
    }

    #[cfg(debug_assertions)]
    fn debug_assert_moving_invariants(&self) {
        if let CursorMode::Moving {
            flags,
            unwind_depth: _,
            ..
        } = &self.mode
        {
            if flags_is_blocked(*flags) {
                debug_assert!(
                    flags_has_unwind(*flags),
                    "blocked moving cursor must have pending unwind depth"
                );
            } else if flags_is_active(*flags) {
                debug_assert!(
                    !flags_has_unwind(*flags),
                    "active moving cursor must not have pending unwind depth"
                );
            }
            debug_assert!(
                !(*flags & FLAG_BLOCKED != 0 && *flags & FLAG_COMPLETE != 0),
                "cursor cannot be both blocked and complete"
            );
        }
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn debug_assert_moving_invariants(&self) {}

    pub fn is_active(&self) -> bool {
        flags_is_active(self.activity_flags())
    }

    pub fn is_blocked(&self) -> bool {
        flags_is_blocked(self.activity_flags())
    }

    pub fn is_complete(&self) -> bool {
        flags_is_complete(self.activity_flags())
    }

    /// Whether matching should skip this cursor (blocked or complete).
    pub fn end(&self) -> bool {
        !self.is_active()
    }

    #[cfg(test)]
    pub fn spawn_moving(&self, at_depth: super::DepthSize, next_position: Position) -> Self {
        Self {
            scope_depth: at_depth,
            parent: self.parent,
            position: next_position,
            mode: CursorMode::Moving {
                match_base_depth: at_depth,
                unwind_depth: 0,
                flags: CursorActivity::Active.to_flags(),
            },
        }
    }

    pub fn anchor_clone(&self, depth: super::DepthSize) -> Self {
        Self {
            scope_depth: depth,
            parent: self.parent,
            position: self.position,
            mode: CursorMode::Anchored {
                flags: CursorActivity::Active.to_flags(),
            },
        }
    }

    /// Pause matching until the element at `depth` closes.
    pub fn block_until_close(&mut self, depth: super::DepthSize) {
        debug_assert!(
            depth <= super::MAX_ELEMENT_DEPTH,
            "element depth must not exceed MAX_ELEMENT_DEPTH"
        );
        debug_assert!(
            !self.is_complete(),
            "cannot block a permanently complete cursor"
        );
        if let CursorMode::Moving {
            flags,
            unwind_depth,
            ..
        } = &mut self.mode
        {
            *unwind_depth = depth;
            *flags = CursorActivity::Blocked.to_flags() | FLAG_HAS_UNWIND;
            self.debug_assert_moving_invariants();
        }
    }

    /// Resume matching after the blocked element closes.
    pub fn reactivate_after_close(&mut self) {
        debug_assert!(
            !self.is_complete(),
            "cannot reactivate a permanently complete cursor"
        );
        debug_assert!(self.is_blocked(), "reactivate requires blocked cursor");
        if let CursorMode::Moving {
            flags,
            unwind_depth,
            ..
        } = &mut self.mode
        {
            *flags = CursorActivity::Active.to_flags();
            *unwind_depth = 0;
            self.debug_assert_moving_invariants();
        }
    }

    /// Keep the cursor complete after its awaited close; clear pending unwind
    /// without reactivating matching.
    pub fn complete_after_close(&mut self) {
        if let CursorMode::Moving {
            flags,
            unwind_depth,
            ..
        } = &mut self.mode
        {
            *flags = CursorActivity::Complete.to_flags();
            *unwind_depth = 0;
            self.debug_assert_moving_invariants();
        }
    }

    /// Mark matching complete while optionally waiting for the selected element
    /// to close.
    pub fn complete_until_close(&mut self, depth: super::DepthSize) {
        debug_assert!(
            depth <= super::MAX_ELEMENT_DEPTH,
            "element depth must not exceed MAX_ELEMENT_DEPTH"
        );
        debug_assert!(
            self.is_active(),
            "complete_until_close requires active cursor"
        );
        if let CursorMode::Moving {
            flags,
            unwind_depth,
            ..
        } = &mut self.mode
        {
            *unwind_depth = depth;
            *flags = CursorActivity::Complete.to_flags() | FLAG_HAS_UNWIND;
            self.debug_assert_moving_invariants();
        }
    }

    /// Mark this cursor complete (e.g. after a `First` terminal match).
    pub fn mark_complete(&mut self) {
        let keep_unwind = self.unwind_depth().is_some();
        match &mut self.mode {
            CursorMode::Moving {
                flags,
                unwind_depth,
                ..
            } => {
                if keep_unwind {
                    *flags = CursorActivity::Complete.to_flags() | FLAG_HAS_UNWIND;
                } else {
                    *flags = CursorActivity::Complete.to_flags();
                    *unwind_depth = 0;
                }
                self.debug_assert_moving_invariants();
            }
            CursorMode::Anchored { flags } => {
                *flags = CursorActivity::Complete.to_flags();
            }
        }
    }
}

impl<'query> ScopedCursor {
    pub fn next<'html, Q: QuerySpec<'query>>(
        &self,
        tree: &Q,
        depth: super::DepthSize,
        element: &XHtmlElement<'html>,
    ) -> bool {
        if !self.is_active() {
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

    fn moving_cursor() -> ScopedCursor {
        ScopedCursor::new_moving_with_last(
            5,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
            2,
        )
    }

    #[test]
    fn active_block_until_close_becomes_blocked() {
        let mut cursor = moving_cursor();
        assert!(cursor.is_active());
        assert!(!cursor.is_blocked());
        assert!(!cursor.is_complete());

        cursor.block_until_close(4);

        assert!(!cursor.is_active());
        assert!(cursor.is_blocked());
        assert!(!cursor.is_complete());
        assert!(cursor.end());
        assert_eq!(cursor.unwind_depth(), Some(4));
    }

    #[test]
    fn blocked_reactivate_after_close_becomes_active() {
        let mut cursor = moving_cursor();
        cursor.block_until_close(4);

        cursor.reactivate_after_close();

        assert!(cursor.is_active());
        assert!(!cursor.is_blocked());
        assert!(!cursor.is_complete());
        assert!(!cursor.end());
        assert_eq!(cursor.unwind_depth(), None);
    }

    #[test]
    fn active_complete_until_close_becomes_complete_with_unwind() {
        let mut cursor = moving_cursor();

        cursor.complete_until_close(4);

        assert!(!cursor.is_active());
        assert!(!cursor.is_blocked());
        assert!(cursor.is_complete());
        assert_eq!(cursor.unwind_depth(), Some(4));
    }

    #[test]
    fn complete_with_unwind_clears_unwind_after_close() {
        let mut cursor = moving_cursor();
        cursor.complete_until_close(4);
        assert_eq!(cursor.unwind_depth(), Some(4));

        cursor.complete_after_close();

        assert!(cursor.is_complete());
        assert_eq!(cursor.unwind_depth(), None);
    }

    #[test]
    fn mark_complete_on_moving_and_anchored() {
        let mut moving = moving_cursor();
        moving.mark_complete();
        assert!(moving.is_complete());
        assert!(!moving.is_blocked());

        let mut anchored = ScopedCursor::new_anchored(
            0,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
        );
        anchored.mark_complete();
        assert!(anchored.is_complete());
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "cannot reactivate a permanently complete cursor")]
    fn complete_reactivate_panics_in_debug() {
        let mut cursor = moving_cursor();
        cursor.mark_complete();
        cursor.reactivate_after_close();
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "active moving cursor must not have pending unwind depth")]
    fn active_with_pending_unwind_panics_in_debug() {
        let mut cursor = moving_cursor();
        if let CursorMode::Moving {
            unwind_depth,
            flags,
            ..
        } = &mut cursor.mode
        {
            *unwind_depth = 4;
            *flags |= super::FLAG_HAS_UNWIND;
        }
        cursor.debug_assert_moving_invariants();
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "blocked moving cursor must have pending unwind depth")]
    fn blocked_without_pending_unwind_panics_in_debug() {
        let mut cursor = moving_cursor();
        cursor.set_activity_flags(super::CursorActivity::Blocked.to_flags());
        cursor.debug_assert_moving_invariants();
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
        assert!(cursor.is_blocked());
        assert!(cursor.end());
        assert_eq!(cursor.unwind_depth(), Some(4));
        assert_eq!(cursor.match_base_depth(), 2);

        cursor.reactivate_after_close();
        assert!(cursor.is_active());
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
                flags,
                ..
            } => {
                assert_eq!(*match_base_depth, 2);
                assert_eq!(cursor.unwind_depth(), None);
                assert!(super::flags_is_active(*flags));
            }
            _ => panic!("expected Moving"),
        }
    }

    #[test]
    fn test_complete_after_close() {
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
        assert!(cursor.is_blocked());
        assert_eq!(cursor.unwind_depth(), Some(4));

        cursor.complete_after_close();
        assert!(cursor.is_complete());
        assert!(cursor.end());
        assert_eq!(cursor.unwind_depth(), None);
        assert_eq!(cursor.match_base_depth(), 2);
    }

    #[test]
    fn scoped_cursor_size_is_stable() {
        // FLAG_HAS_UNWIND tracks pending unwind depth instead of a sentinel value.
        let cursor_size = std::mem::size_of::<ScopedCursor>();
        let mode_size = std::mem::size_of::<CursorMode>();
        let position_size = std::mem::size_of::<Position>();
        assert!(
            cursor_size <= 32,
            "ScopedCursor={cursor_size} CursorMode={mode_size} Position={position_size} exceeds 32-byte budget"
        );
        assert_eq!(
            cursor_size, 32,
            "ScopedCursor should remain exactly 32 bytes"
        );
        assert_eq!(mode_size, 6, "CursorMode should remain exactly 6 bytes");
        assert!(position_size > 0);
    }
}
