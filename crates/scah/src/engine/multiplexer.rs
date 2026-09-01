use super::attribute_interest::AttributeInterest;
use super::executor::QueryExecutor;
use crate::__private::ascii_case_insensitive_hash;
use crate::Position;
use crate::XHtmlElement;
use crate::store::ElementId;
use crate::store::Store;
use crate::{QuerySpec, Reader};
use smallvec::SmallVec;

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
    pub save_attributes: bool,
    pub save_inner_html: bool,
    pub save_text_content: bool,
}

/// Query work selected while the parser still has only the opening tag name.
///
/// The parser reuses this allocation between elements. Besides deciding which
/// attributes to tokenize, it carries the viable runner set into execution so
/// the query frontier is not traversed twice for every opening tag.
#[derive(Debug, Default)]
pub(crate) struct ElementPreflight<'query> {
    pub attribute_interest: AttributeInterest<'query>,
    runner_indices: SmallVec<[usize; 8]>,
    runner_len: usize,
}

/// Stable identity for a query executor slot in [`QueryMultiplexer`].
///
/// Slot indices never move or reuse during a parse, so deferred sibling
/// callbacks remain valid even after earlier `First` runners retire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunnerId(pub(crate) usize);

impl RunnerId {
    #[inline(always)]
    pub(crate) const fn index(self) -> usize {
        self.0
    }
}

/// Deferred close-time activation of a CSS `+` / `~` right-hand cursor.
///
/// Created when the left-hand transition matches; activated when that element
/// closes (or immediately for void/self-closing sources). Lifetime is derived
/// from the continuation transition's combinator, not stored here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SiblingCallback {
    pub runner: RunnerId,
    pub output_parent: ElementId,
    pub continuation: Position,
}

type Runner<'query, Q> = Vec<Option<QueryExecutor<'query, 'query, Q>>>;

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
        queries
            .iter()
            .map(|query| Some(QueryExecutor::new(query)))
            .collect()
    }

    #[inline]
    fn all_runners_retired(&self) -> bool {
        self.runners.iter().all(Option::is_none)
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

        let resident = self
            .runners
            .iter()
            .filter_map(Option::as_ref)
            .map(|runner| runner.cursors.len())
            .sum();

        let active = self
            .runners
            .iter()
            .filter_map(Option::as_ref)
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
            .filter_map(Option::as_ref)
            .any(|runner| runner.query().requires_text_content())
    }

    pub(crate) fn requires_attribute_storage(&self) -> bool {
        self.runners
            .iter()
            .filter_map(Option::as_ref)
            .any(|runner| runner.query().requires_attribute_storage())
    }

    pub(crate) fn requires_attribute_parsing(&self) -> bool {
        self.runners
            .iter()
            .filter_map(Option::as_ref)
            .any(|runner| runner.query().requires_attribute_parsing())
    }

    pub(crate) fn allows_early_exit(&self) -> bool {
        self.runners
            .iter()
            .filter_map(Option::as_ref)
            .all(|runner| runner.query().exit_at_section_end().is_some())
    }

    /// Whether the active query frontier may inspect or save attributes for
    /// this element name.
    #[inline]
    pub(crate) fn prepare_element(&self, name: &str, preflight: &mut ElementPreflight<'query>) {
        preflight.attribute_interest.clear();
        preflight.runner_indices.clear();
        preflight.runner_len = self.runners.len();
        let name_hash = ascii_case_insensitive_hash(name);
        for (runner_index, runner) in self.runners.iter().enumerate() {
            let Some(runner) = runner.as_ref() else {
                continue;
            };
            if runner.extend_attribute_interest_for(
                name,
                name_hash,
                &mut preflight.attribute_interest,
            ) {
                preflight.runner_indices.push(runner_index);
            }
        }
    }

    pub(crate) fn next_prepared_into(
        &mut self,
        xhtml_element: &XHtmlElement<'html>,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        save_hits: &mut Vec<SaveHit>,
        preflight: &ElementPreflight<'query>,
        sibling_callbacks: &mut Vec<SiblingCallback>,
    ) {
        debug_assert_eq!(self.runners.len(), preflight.runner_len);
        save_hits.clear();
        sibling_callbacks.clear();
        for &runner_index in &preflight.runner_indices {
            let Some(session) = self.runners[runner_index].as_mut() else {
                continue;
            };
            let runner = RunnerId(runner_index);
            session.next(
                runner,
                xhtml_element,
                position,
                store,
                save_hits,
                sibling_callbacks,
            );
        }
        #[cfg(any(debug_assertions, test))]
        for (runner_index, runner) in self.runners.iter().enumerate() {
            if !preflight.runner_indices.contains(&runner_index) {
                let Some(runner) = runner.as_ref() else {
                    continue;
                };
                runner.trace_name_rejections(
                    runner_index,
                    xhtml_element,
                    position.element_depth,
                    store,
                );
            }
        }
        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();
    }

    pub(crate) fn activate_sibling_callback(
        &mut self,
        callback: SiblingCallback,
        source_depth: crate::engine::DepthSize,
        store: &mut Store<'html, 'query>,
    ) {
        match self.runners.get_mut(callback.runner.index()) {
            Some(Some(session)) => {
                let _ = session.activate_sibling(callback.runner, callback, source_depth, store);
            }
            // The callback belongs to a runner that already completed.
            Some(None) => {}
            // Internal corruption: callback contains an impossible ID.
            None => {
                debug_assert!(false, "sibling callback references unknown runner");
            }
        }
        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();
    }

    pub(crate) fn activate_sibling_callbacks(
        &mut self,
        callbacks: &mut Vec<SiblingCallback>,
        source_depth: crate::engine::DepthSize,
        store: &mut Store<'html, 'query>,
    ) {
        for callback in callbacks.drain(..) {
            self.activate_sibling_callback(callback, source_depth, store);
        }
    }

    pub(crate) fn back(
        &mut self,
        xhtml_element: &'html str,
        position: &DocumentPosition,
        reader: &Reader<'html>,
        store: &mut Store<'html, 'query>,
    ) -> bool {
        for (index, slot) in self.runners.iter_mut().enumerate() {
            let retire = match slot.as_mut() {
                Some(session) => {
                    let runner = RunnerId(index);
                    let significant_close = session.back(runner, xhtml_element, position, store);
                    // A First runner can exit only after close handling finalizes its winner.
                    significant_close && session.early_exit()
                }
                None => false,
            };

            if retire {
                *slot = None;
            }
        }
        let _ = reader;

        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();

        self.all_runners_retired()
    }

    #[cfg(test)]
    pub(crate) fn runner_slot_count(&self) -> usize {
        self.runners.len()
    }

    #[cfg(test)]
    pub(crate) fn runner_slot_occupied(&self, runner: RunnerId) -> bool {
        matches!(self.runners.get(runner.index()), Some(Some(_)))
    }

    #[cfg(test)]
    pub(crate) fn retire_runner_for_test(&mut self, runner: RunnerId) {
        if let Some(slot) = self.runners.get_mut(runner.index()) {
            *slot = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn runner_query_source(&self, runner: RunnerId) -> Option<&'query str> {
        self.runners
            .get(runner.index())
            .and_then(Option::as_ref)
            .map(|session| {
                session
                    .query()
                    .get_selection(crate::QuerySectionId(0))
                    .source
            })
    }
}

#[cfg(test)]
mod runner_slot_tests {
    use super::{QueryMultiplexer, RunnerId};
    use crate::{Query, Save};

    #[test]
    fn retiring_earlier_runner_does_not_shift_later_slots() {
        let queries = [
            Query::first("h1", Save::none()).unwrap().build(),
            Query::all("section + p", Save::none()).unwrap().build(),
            Query::all("aside + footer", Save::none()).unwrap().build(),
        ];
        let mut mux = QueryMultiplexer::new(&queries);

        assert_eq!(mux.runner_slot_count(), 3);
        assert!(mux.runner_slot_occupied(RunnerId(0)));
        assert!(mux.runner_slot_occupied(RunnerId(1)));
        assert!(mux.runner_slot_occupied(RunnerId(2)));
        assert_eq!(mux.runner_query_source(RunnerId(1)), Some("section + p"));
        assert_eq!(mux.runner_query_source(RunnerId(2)), Some("aside + footer"));

        mux.retire_runner_for_test(RunnerId(0));

        assert_eq!(mux.runner_slot_count(), 3);
        assert!(!mux.runner_slot_occupied(RunnerId(0)));
        assert!(mux.runner_slot_occupied(RunnerId(1)));
        assert!(mux.runner_slot_occupied(RunnerId(2)));
        assert_eq!(mux.runner_query_source(RunnerId(1)), Some("section + p"));
        assert_eq!(mux.runner_query_source(RunnerId(2)), Some("aside + footer"));
        assert_eq!(mux.runner_query_source(RunnerId(0)), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Query, Save};

    #[test]
    fn preflight_carries_only_name_viable_runners_and_their_attributes() {
        let queries = [
            Query::all("a[href]", Save::name_only()).unwrap().build(),
            Query::all("span.hero", Save::name_only()).unwrap().build(),
            Query::all("[data-x]", Save::name_only()).unwrap().build(),
        ];
        let selectors = QueryMultiplexer::new(&queries);
        let mut preflight = ElementPreflight::default();

        selectors.prepare_element("span", &mut preflight);

        assert_eq!(preflight.runner_indices.as_slice(), &[1, 2]);
        assert!(preflight.attribute_interest.includes_class());
        assert!(preflight.attribute_interest.includes_attribute("data-x"));
        assert!(!preflight.attribute_interest.includes_attribute("href"));
    }

    #[test]
    fn indexing_policy_requires_every_query_to_allow_early_exit() {
        let first = [Query::first("a", Save::name_only()).unwrap().build()];
        assert!(QueryMultiplexer::new(&first).allows_early_exit());

        let mixed = [
            Query::first("a", Save::name_only()).unwrap().build(),
            Query::all("span", Save::name_only()).unwrap().build(),
        ];
        assert!(!QueryMultiplexer::new(&mixed).allows_early_exit());
    }

    #[test]
    fn indexing_policy_tracks_whether_queries_may_parse_attributes() {
        let tag_only = [Query::all("a", Save::name_only()).unwrap().build()];
        assert!(!QueryMultiplexer::new(&tag_only).requires_attribute_parsing());

        let selector_attributes = [Query::all("a[href]", Save::name_only()).unwrap().build()];
        assert!(QueryMultiplexer::new(&selector_attributes).requires_attribute_parsing());

        let saved_attributes = [Query::all("a", Save::all()).unwrap().build()];
        assert!(QueryMultiplexer::new(&saved_attributes).requires_attribute_parsing());
    }
}
