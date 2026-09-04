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
const FLAG_FIRST_WINNER: u8 = 1 << 3;
/// Adjacent-sibling (`+`) watcher: expire after the next same-depth element.
///
/// Packed into moving-cursor flags so ordinary `Scope` cursors stay 32 bytes.
/// The adjacent-sibling watcher consumes the first future element opened at its
/// sibling depth. A future `:nth-*` implementation may generalize this
/// representation after its state and performance requirements are known.
const FLAG_ADJACENT_REMAINING: u8 = 1 << 4;
/// Adjacent watcher consumed by the current element. It remains present until
/// the executor finishes its snapshot loop so the current element can match,
/// but must not dominate a replacement watcher spawned by that element.
const FLAG_ADJACENT_EXPIRING: u8 = 1 << 5;

/// Bounds how long a cursor remains eligible to match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorLifetime {
    /// Lives until normal scope pruning or explicit lifecycle completion.
    Scope,

    /// Adjacent-sibling (`+`) watcher: consumes the first future element opened
    /// at `match_base_depth`, then expires.
    AdjacentSibling,
}

/// Result of attempting to consume an adjacent-sibling lifetime tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SiblingLifetimeResult {
    NotApplicable,
    /// The current same-depth element is the final candidate; process it, then
    /// remove the watcher.
    ExpiresAfterCurrentElement,
}

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

#[inline]
const fn flags_is_first_winner(flags: u8) -> bool {
    flags & FLAG_FIRST_WINNER != 0
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
        /// Depth used to evaluate the current transition.
        match_base_depth: super::DepthSize,
        /// Pending close depth when [`FLAG_HAS_UNWIND`] is set.
        unwind_depth: super::DepthSize,
        /// Packed activity and lifecycle flags.
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

    /// Create a cursor that watches later element siblings under a shared parent.
    ///
    /// For a left-hand match at depth `D`:
    /// - `parent_scope_depth` is `D - 1` (the common parent)
    /// - `sibling_depth` is `D` (later siblings open at this depth)
    ///
    /// Lifetime is packed into moving-cursor flags: `Scope` for `~`, and
    /// `AdjacentSibling` for `+`.
    pub(crate) fn new_sibling(
        parent_scope_depth: super::DepthSize,
        sibling_depth: super::DepthSize,
        parent: ElementId,
        position: Position,
        lifetime: CursorLifetime,
    ) -> Self {
        debug_assert!(
            sibling_depth == parent_scope_depth.saturating_add(1)
                || parent_scope_depth == SENTINEL_SCOPE,
            "sibling cursor match_base_depth must be scope_depth + 1"
        );
        let mut flags = CursorActivity::Active.to_flags();
        match lifetime {
            CursorLifetime::Scope => {}
            CursorLifetime::AdjacentSibling => {
                flags |= FLAG_ADJACENT_REMAINING;
            }
        }
        Self {
            scope_depth: parent_scope_depth,
            parent,
            position,
            mode: CursorMode::Moving {
                match_base_depth: sibling_depth,
                unwind_depth: 0,
                flags,
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

    /// Depth whose close advances this cursor's lifecycle.
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

    #[cfg(all(test, debug_assertions))]
    fn set_activity_flags(&mut self, flags: u8) {
        match &mut self.mode {
            CursorMode::Moving { flags: f, .. } | CursorMode::Anchored { flags: f } => *f = flags,
        }
    }

    #[cfg(debug_assertions)]
    fn debug_assert_moving_invariants(&self) {
        match &self.mode {
            CursorMode::Moving { flags, .. } => {
                if flags_is_blocked(*flags) {
                    debug_assert!(
                        flags_has_unwind(*flags),
                        "blocked moving cursor must have pending unwind depth"
                    );
                    debug_assert!(
                        !flags_is_first_winner(*flags),
                        "blocked cursor cannot be a First winner"
                    );
                } else if flags_is_active(*flags) {
                    debug_assert!(
                        !flags_has_unwind(*flags),
                        "active moving cursor must not have pending unwind depth"
                    );
                    debug_assert!(
                        !flags_is_first_winner(*flags),
                        "active cursor cannot be a First winner"
                    );
                }
                debug_assert!(
                    !(*flags & FLAG_BLOCKED != 0 && *flags & FLAG_COMPLETE != 0),
                    "cursor cannot be both blocked and complete"
                );
                if flags_is_first_winner(*flags) {
                    debug_assert!(flags_is_complete(*flags), "FIRST_WINNER implies COMPLETE");
                }
            }
            CursorMode::Anchored { flags } => {
                debug_assert!(
                    !flags_is_first_winner(*flags),
                    "anchored cursor cannot be a First winner"
                );
            }
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

    /// Whether this cursor owns a terminal `First` selection for its scope.
    #[inline]
    pub fn is_first_winner(&self) -> bool {
        flags_is_first_winner(self.activity_flags())
    }

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

    /// Logical lifetime for sibling-stream watchers.
    ///
    /// Adjacent (`+`) state is packed into moving-cursor flags so ordinary
    /// selectors keep a 32-byte [`ScopedCursor`].
    #[inline]
    pub(crate) fn lifetime(&self) -> CursorLifetime {
        match &self.mode {
            CursorMode::Moving { flags, .. } if *flags & FLAG_ADJACENT_REMAINING != 0 => {
                CursorLifetime::AdjacentSibling
            }
            _ => CursorLifetime::Scope,
        }
    }

    #[inline(always)]
    pub(crate) fn consume_sibling_at(&mut self, depth: super::DepthSize) -> SiblingLifetimeResult {
        let CursorMode::Moving {
            flags,
            match_base_depth,
            ..
        } = &mut self.mode
        else {
            return SiblingLifetimeResult::NotApplicable;
        };
        // Ordinary Scope cursors hit this bit-clear check only.
        if *flags & FLAG_ADJACENT_REMAINING == 0 {
            return SiblingLifetimeResult::NotApplicable;
        }
        if *match_base_depth != depth || !flags_is_active(*flags) {
            return SiblingLifetimeResult::NotApplicable;
        }
        *flags &= !FLAG_ADJACENT_REMAINING;
        *flags |= FLAG_ADJACENT_EXPIRING;
        SiblingLifetimeResult::ExpiresAfterCurrentElement
    }

    #[inline]
    pub(crate) fn is_adjacent_expiring(&self) -> bool {
        matches!(
            self.mode,
            CursorMode::Moving { flags, .. } if flags & FLAG_ADJACENT_EXPIRING != 0
        )
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

    /// Clear a completed cursor's pending close while preserving winner ownership.
    pub fn complete_after_close(&mut self) {
        if let CursorMode::Moving {
            flags,
            unwind_depth,
            ..
        } = &mut self.mode
        {
            let retained = *flags & FLAG_FIRST_WINNER;
            *flags = retained | CursorActivity::Complete.to_flags();
            *unwind_depth = 0;
            self.debug_assert_moving_invariants();
        }
    }

    /// Mark this cursor as the `First` winner.
    ///
    /// The selected element close finalizes its content, while
    /// `ownership_scope_depth` keeps later matches suppressed until the output
    /// parent closes.
    pub fn select_first_until_close(
        &mut self,
        selected_depth: super::DepthSize,
        ownership_scope_depth: super::DepthSize,
    ) {
        debug_assert!(
            selected_depth <= super::MAX_ELEMENT_DEPTH,
            "selected depth must not exceed MAX_ELEMENT_DEPTH"
        );
        debug_assert!(
            ownership_scope_depth == SENTINEL_SCOPE || ownership_scope_depth <= selected_depth,
            "First ownership scope must contain selected element"
        );
        debug_assert!(
            self.is_moving(),
            "select_first_until_close requires moving cursor"
        );
        debug_assert!(
            self.is_active(),
            "select_first_until_close requires active cursor"
        );

        self.scope_depth = ownership_scope_depth;

        if let CursorMode::Moving {
            flags,
            unwind_depth,
            ..
        } = &mut self.mode
        {
            *unwind_depth = selected_depth;
            *flags = CursorActivity::Complete.to_flags() | FLAG_HAS_UNWIND | FLAG_FIRST_WINNER;
        }

        self.debug_assert_moving_invariants();
    }

    /// Permanently cancel a non-winning cursor in a claimed `First` scope.
    pub fn cancel_complete(&mut self) {
        match &mut self.mode {
            CursorMode::Moving {
                flags,
                unwind_depth,
                ..
            } => {
                *flags = CursorActivity::Complete.to_flags();
                *unwind_depth = 0;
            }
            CursorMode::Anchored { flags } => {
                *flags = CursorActivity::Complete.to_flags();
            }
        }
        self.debug_assert_moving_invariants();
    }
}

impl<'query> ScopedCursor {
    #[allow(dead_code)]
    #[inline(always)]
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

    #[inline(always)]
    pub fn next_with_context<'html, Q: QuerySpec<'query>>(
        &self,
        tree: &Q,
        depth: super::DepthSize,
        element: &XHtmlElement<'html>,
        structural: Option<&crate::StructuralMatchContext>,
    ) -> bool {
        if !self.is_active() {
            return false;
        }
        let fsm = tree.get_transition(self.position.state);
        fsm.next_with_context(element, depth, self.match_base_depth(), structural)
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
            positions.extend(tree.child_positions(&self.position));
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
    fn select_first_until_close_marks_moving_winner() {
        let mut cursor = moving_cursor();

        cursor.select_first_until_close(4, 2);

        assert!(cursor.is_moving());
        assert!(cursor.is_first_winner());
        assert!(cursor.is_complete());
        assert!(!cursor.is_active());
        assert!(!cursor.is_blocked());
        assert_eq!(cursor.scope_depth, 2);
        assert_eq!(cursor.unwind_depth(), Some(4));
    }

    #[test]
    fn select_first_rebinds_cursor_to_ownership_scope() {
        let mut cursor = ScopedCursor::new_moving_with_last(
            4,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
            2,
        );

        cursor.select_first_until_close(5, 1);

        assert_eq!(cursor.scope_depth, 1);
        assert_eq!(cursor.unwind_depth(), Some(5));
        assert!(cursor.is_complete());
        assert!(cursor.is_first_winner());

        cursor.complete_after_close();

        assert_eq!(cursor.scope_depth, 1);
        assert_eq!(cursor.unwind_depth(), None);
        assert!(cursor.is_first_winner());
    }

    #[test]
    fn select_first_preserves_sentinel_ownership_scope() {
        let mut cursor = moving_cursor();

        cursor.select_first_until_close(5, SENTINEL_SCOPE);

        assert_eq!(cursor.scope_depth, SENTINEL_SCOPE);
        assert_eq!(cursor.unwind_depth(), Some(5));
        assert!(cursor.is_first_winner());
        assert!(cursor.is_complete());
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "First ownership scope must contain selected element")]
    fn select_first_rejects_ownership_narrower_than_selected() {
        let mut cursor = moving_cursor();
        cursor.select_first_until_close(3, 4);
    }

    #[test]
    fn first_winner_preserves_flag_after_close() {
        let mut cursor = moving_cursor();
        cursor.select_first_until_close(4, 2);

        cursor.complete_after_close();

        assert!(cursor.is_moving());
        assert!(cursor.is_first_winner());
        assert!(cursor.is_complete());
        assert_eq!(cursor.scope_depth, 2);
        assert_eq!(cursor.unwind_depth(), None);
    }

    #[test]
    fn cancel_complete_clears_first_winner() {
        let mut cursor = moving_cursor();
        cursor.select_first_until_close(4, 2);
        assert!(cursor.is_first_winner());

        cursor.cancel_complete();

        assert!(cursor.is_complete());
        assert!(!cursor.is_first_winner());
        assert_eq!(cursor.unwind_depth(), None);
    }

    #[test]
    fn cancel_complete_on_active_blocked_and_anchored() {
        let mut active = moving_cursor();
        active.cancel_complete();
        assert!(active.is_complete());
        assert_eq!(active.unwind_depth(), None);

        let mut blocked = moving_cursor();
        blocked.block_until_close(4);
        blocked.cancel_complete();
        assert!(blocked.is_complete());
        assert_eq!(blocked.unwind_depth(), None);

        let mut anchored = ScopedCursor::new_anchored(
            0,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
        );
        anchored.cancel_complete();
        assert!(anchored.is_complete());
        assert_eq!(anchored.unwind_depth(), None);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "cannot reactivate a permanently complete cursor")]
    fn complete_reactivate_panics_in_debug() {
        let mut cursor = moving_cursor();
        cursor.cancel_complete();
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
    fn adjacent_sibling_lifetime_packs_and_expires_once() {
        use super::{CursorLifetime, SiblingLifetimeResult};

        let mut cursor = ScopedCursor::new_sibling(
            1,
            2,
            NULL_PARENT,
            Position {
                selection: QuerySectionId(0),
                state: TransitionId(0),
            },
            CursorLifetime::AdjacentSibling,
        );
        assert_eq!(cursor.lifetime(), CursorLifetime::AdjacentSibling);
        assert_eq!(
            cursor.consume_sibling_at(2),
            SiblingLifetimeResult::ExpiresAfterCurrentElement
        );
        assert_eq!(cursor.lifetime(), CursorLifetime::Scope);
        assert_eq!(
            cursor.consume_sibling_at(2),
            SiblingLifetimeResult::NotApplicable
        );
    }

    #[test]
    fn scoped_cursor_size_is_stable() {
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
