use super::executor::QueryExecutor;
use crate::Position;
use crate::XHtmlElement;
use crate::store::ElementId;
use crate::store::Store;
use crate::{QuerySpec, Reader};

pub(crate) struct DocumentPosition {
    pub reader_position: usize,
    pub text_content_position: usize,
    pub element_depth: crate::engine::DepthSize,
    /// Precomputed by the parser once per open tag so the executor
    /// never calls `XHtmlElement::is_self_closing()` in its hot loop.
    pub self_closing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SaveHit {
    pub element_id: ElementId,
    pub save_inner_html: bool,
    pub save_text_content: bool,
}

/// Deferred close-time activation of a CSS `+` / `~` right-hand cursor.
///
/// Created when the left-hand transition matches; activated when that element
/// closes (or immediately for void/self-closing sources). Lifetime is derived
/// from the continuation transition's combinator, not stored here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SiblingCallback {
    pub runner_index: usize,
    pub output_parent: ElementId,
    pub continuation: Position,
}

type Runner<'query, Q> = Vec<QueryExecutor<'query, Q>>;

#[cfg(feature = "bench-internals")]
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CursorStats {
    peak_resident_cursor_slots: usize,
    peak_active_obligations: usize,
}

#[cfg(feature = "bench-internals")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorStatsSnapshot {
    pub peak_resident_cursor_slots: usize,
    pub peak_active_obligations: usize,
}

pub struct QueryMultiplexer<'query, Q> {
    runners: Runner<'query, Q>,
    #[cfg(feature = "bench-internals")]
    cursor_stats: Option<CursorStats>,
}

impl<'html, 'query: 'html, Q> QueryMultiplexer<'query, Q>
where
    Q: QuerySpec<'query>,
{
    fn build_runners(queries: &'query [Q]) -> Runner<'query, Q> {
        #[allow(clippy::redundant_closure)]
        queries
            .iter()
            .map(|query| QueryExecutor::new(query))
            .collect()
    }

    pub fn new(queries: &'query [Q]) -> Self {
        Self {
            runners: Self::build_runners(queries),
            #[cfg(feature = "bench-internals")]
            cursor_stats: None,
        }
    }

    #[cfg(feature = "bench-internals")]
    pub(crate) fn new_with_cursor_stats(queries: &'query [Q]) -> Self {
        Self {
            runners: Self::build_runners(queries),
            cursor_stats: Some(CursorStats::default()),
        }
    }

    #[cfg(feature = "bench-internals")]
    #[allow(dead_code)]
    pub(crate) fn cursor_stats_enabled(&self) -> bool {
        self.cursor_stats.is_some()
    }

    /// Update peak cursor counts from the current runner state.
    #[cfg(feature = "bench-internals")]
    #[inline]
    fn track_cursor_stats(&mut self) {
        let Some(stats) = self.cursor_stats.as_mut() else {
            return;
        };

        let resident = self.runners.iter().map(|runner| runner.cursors.len()).sum();

        let active = self
            .runners
            .iter()
            .flat_map(|runner| runner.cursors.iter())
            .filter(|cursor| cursor.is_active())
            .count();

        stats.peak_resident_cursor_slots = stats.peak_resident_cursor_slots.max(resident);
        stats.peak_active_obligations = stats.peak_active_obligations.max(active);
    }

    #[cfg(feature = "bench-internals")]
    pub(crate) fn cursor_stats_snapshot(&self) -> CursorStatsSnapshot {
        let stats = self.cursor_stats.unwrap_or_default();
        CursorStatsSnapshot {
            peak_resident_cursor_slots: stats.peak_resident_cursor_slots,
            peak_active_obligations: stats.peak_active_obligations,
        }
    }

    #[cfg(feature = "bench-internals")]
    pub(crate) fn sample_cursor_stats(&mut self) {
        self.track_cursor_stats();
    }

    pub(crate) fn requires_text_content(&self) -> bool {
        self.runners
            .iter()
            .any(|runner| runner.query().requires_text_content())
    }

    pub(crate) fn next_into(
        &mut self,
        xhtml_element: &XHtmlElement<'html>,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        save_hits: &mut Vec<SaveHit>,
        sibling_callbacks: &mut Vec<SiblingCallback>,
    ) {
        let len = store.elements.len();
        save_hits.clear();
        sibling_callbacks.clear();
        for (runner_index, session) in self.runners.iter_mut().enumerate() {
            session.next(
                runner_index,
                xhtml_element,
                position,
                store,
                save_hits,
                sibling_callbacks,
            );
        }
        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();
        if len == store.elements.len() {
            xhtml_element.remove_attributes(&mut store.attributes);
        }
    }

    pub(crate) fn activate_sibling_callbacks(
        &mut self,
        callbacks: &mut Vec<SiblingCallback>,
        source_depth: crate::engine::DepthSize,
        store: &mut Store<'html, 'query>,
    ) {
        for callback in callbacks.drain(..) {
            let Some(session) = self.runners.get_mut(callback.runner_index) else {
                continue;
            };
            let _ = session.activate_sibling(callback.runner_index, callback, source_depth, store);
        }
        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();
    }

    pub(crate) fn back(
        &mut self,
        xhtml_element: &'html str,
        position: &DocumentPosition,
        reader: &Reader<'html>,
        store: &mut Store<'html, 'query>,
    ) -> bool {
        let mut remove_indices = vec![];
        for (index, session) in self.runners.iter_mut().enumerate() {
            let significant_close = session.back(index, xhtml_element, position, store);
            // A First runner can exit only after close handling finalizes its winner.
            if significant_close && session.early_exit() {
                remove_indices.push(index);
            }
        }
        let _ = reader;
        for idx in remove_indices.into_iter().rev() {
            self.runners.remove(idx);
        }

        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();

        self.runners.is_empty()
    }
}
