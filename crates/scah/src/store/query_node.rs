use super::arena::Arena;
use super::arena::span::Span;
use super::arena::{Node, id};

#[derive(Debug, PartialEq, Default)]
pub struct QueryNode<'query> {
    pub query: &'query str,
    pub next_sibling: Option<id::QueryId>,
    pub elements: Span<id::ElementId>,
    /// Number of matched elements in the result linked list.
    ///
    /// Maintained incrementally on every match insertion so callers can obtain
    /// result length in O(1) without walking `next_sibling`.
    pub match_count: usize,
    /// Contiguous match ids in insertion order — same sequence as the
    /// `next_sibling` walk from [`Self::elements`], for O(n) memcpy fills.
    pub match_ids: Vec<id::ElementId>,
}

impl<'query> Node<id::QueryId> for QueryNode<'query> {
    fn next_sibling(&self) -> Option<id::QueryId> {
        self.next_sibling
    }
}

impl<'query> QueryNode<'query> {
    pub fn iter(
        &self,
        arena: &'query Arena<QueryNode<'query>, id::QueryId>,
    ) -> impl Iterator<Item = &'query QueryNode<'query>> {
        let index = unsafe { arena.index_of(self) };
        arena.iter_from(index)
    }
}
