use super::executor::QueryExecutor;
use crate::Position;
use crate::XHtmlElement;
use crate::store::ElementId;
use crate::store::Store;
use crate::{Combinator, QuerySpec, Reader};

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

/// Stable identity for a query executor slot in [`QueryMultiplexer`].
///
/// Slot indices never move or reuse during a parse, so deferred sibling
/// callbacks remain valid even after earlier `First` runners retire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

type Runners<'query, Q> = Vec<QueryExecutor<'query, Q>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct QueryFeatures {
    pub(crate) has_sibling: bool,
    pub(crate) has_adjacent: bool,
}

fn query_features<'query, Q: QuerySpec<'query>>(query: &Q) -> QueryFeatures {
    let mut features = QueryFeatures::default();
    for transition in query.states() {
        match transition.guard {
            Combinator::NextSibling => {
                features.has_sibling = true;
                features.has_adjacent = true;
            }
            Combinator::SubsequentSibling => features.has_sibling = true,
            _ => {}
        }
    }
    features
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MultiplexerFeatures {
    pub(crate) has_sibling_queries: bool,
    pub(crate) has_adjacent_queries: bool,
    pub(crate) has_retiring_runners: bool,
}

// `None` is Dense. `Some(empty)` is Empty and `Some(nonempty)` is Sparse. The
// extra allocation happens only on first retirement and keeps the much hotter
// Dense representation pointer-sized instead of embedding a 24-byte Vec.
#[allow(clippy::box_collection)]
type ActiveRunnerSet = Option<Box<Vec<RunnerId>>>;

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
    /// Permanent dense slots indexed by [`RunnerId`]. Slots never shift or
    /// disappear, so deferred callbacks retain stable identity.
    runners: Runners<'query, Q>,
    active: ActiveRunnerSet,
    features: MultiplexerFeatures,
    #[cfg(feature = "bench-internals")]
    cursor_stats: Option<CursorStats>,
}

impl<'html, 'query: 'html, Q> QueryMultiplexer<'query, Q>
where
    Q: QuerySpec<'query>,
{
    fn build_runners(queries: &'query [Q]) -> Runners<'query, Q> {
        queries.iter().map(QueryExecutor::new).collect()
    }

    fn collect_features(queries: &'query [Q]) -> MultiplexerFeatures {
        queries
            .iter()
            .fold(MultiplexerFeatures::default(), |mut aggregate, query| {
                let features = query_features(query);
                aggregate.has_sibling_queries |= features.has_sibling;
                aggregate.has_adjacent_queries |= features.has_adjacent;
                aggregate.has_retiring_runners |= query.exit_at_section_end().is_some();
                aggregate
            })
    }

    #[inline]
    fn all_runners_retired(&self) -> bool {
        self.runners.is_empty() || self.active.as_ref().is_some_and(|ids| ids.is_empty())
    }

    pub fn new(queries: &'query [Q]) -> Self {
        let runners = Self::build_runners(queries);
        Self {
            runners,
            active: None,
            features: Self::collect_features(queries),
            #[cfg(feature = "bench-internals")]
            cursor_stats: None,
        }
    }

    #[cfg(feature = "bench-internals")]
    pub(crate) fn new_with_cursor_stats(queries: &'query [Q]) -> Self {
        let runners = Self::build_runners(queries);
        Self {
            runners,
            active: None,
            features: Self::collect_features(queries),
            cursor_stats: Some(CursorStats::default()),
        }
    }

    #[cfg(feature = "bench-internals")]
    #[allow(dead_code)]
    pub(crate) fn cursor_stats_enabled(&self) -> bool {
        self.cursor_stats.is_some()
    }

    /// Update peak cursor counts from currently live executors only.
    #[cfg(feature = "bench-internals")]
    #[inline]
    fn track_cursor_stats(&mut self) {
        let Some(stats) = self.cursor_stats.as_mut() else {
            return;
        };

        let mut resident = 0;
        let mut active = 0;
        match &self.active {
            None => {
                for session in &self.runners {
                    resident += session.cursors.len();
                    active += session
                        .cursors
                        .iter()
                        .filter(|cursor| cursor.is_active())
                        .count();
                }
            }
            Some(ids) => {
                for runner in ids.iter() {
                    let session = &self.runners[runner.index()];
                    resident += session.cursors.len();
                    active += session
                        .cursors
                        .iter()
                        .filter(|cursor| cursor.is_active())
                        .count();
                }
            }
        }

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

    #[inline]
    pub(crate) fn features(&self) -> MultiplexerFeatures {
        self.features
    }

    pub(crate) fn next_plain_into(
        &mut self,
        xhtml_element: &XHtmlElement<'html>,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        save_hits: &mut Vec<SaveHit>,
    ) {
        match &self.active {
            None => self.next_plain_dense_into(xhtml_element, position, store, save_hits),
            Some(ids) => {
                let len = store.elements.len();
                save_hits.clear();
                Self::next_plain_sparse_into(
                    &mut self.runners,
                    ids,
                    xhtml_element,
                    position,
                    store,
                    save_hits,
                );
                #[cfg(feature = "bench-internals")]
                self.track_cursor_stats();
                if len == store.elements.len() {
                    xhtml_element.remove_attributes(&mut store.attributes);
                }
            }
        }
    }

    #[inline(never)]
    pub(crate) fn next_plain_dense_into(
        &mut self,
        xhtml_element: &XHtmlElement<'html>,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        save_hits: &mut Vec<SaveHit>,
    ) {
        debug_assert!(
            self.active.is_none(),
            "dense dispatch after runner retirement"
        );
        let len = store.elements.len();
        save_hits.clear();
        for (index, session) in self.runners.iter_mut().enumerate() {
            session.next_plain(RunnerId(index), xhtml_element, position, store, save_hits);
        }
        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();
        if len == store.elements.len() {
            xhtml_element.remove_attributes(&mut store.attributes);
        }
    }

    #[inline(never)]
    fn next_plain_sparse_into(
        runners: &mut Runners<'query, Q>,
        ids: &[RunnerId],
        xhtml_element: &XHtmlElement<'html>,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        save_hits: &mut Vec<SaveHit>,
    ) {
        for runner in ids.iter().copied() {
            runners[runner.index()].next_plain(runner, xhtml_element, position, store, save_hits);
        }
    }

    #[inline(never)]
    pub(crate) fn next_with_siblings_into(
        &mut self,
        xhtml_element: &XHtmlElement<'html>,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        save_hits: &mut Vec<SaveHit>,
        sibling_callbacks: &mut Vec<SiblingCallback>,
    ) {
        let len = store.elements.len();
        save_hits.clear();
        match &self.active {
            None => {
                for (index, session) in self.runners.iter_mut().enumerate() {
                    session.next_with_siblings(
                        RunnerId(index),
                        xhtml_element,
                        position,
                        store,
                        save_hits,
                        sibling_callbacks,
                    );
                }
            }
            Some(ids) => {
                for runner in ids.iter().copied() {
                    self.runners[runner.index()].next_with_siblings(
                        runner,
                        xhtml_element,
                        position,
                        store,
                        save_hits,
                        sibling_callbacks,
                    );
                }
            }
        }
        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();
        if len == store.elements.len() {
            xhtml_element.remove_attributes(&mut store.attributes);
        }
    }

    pub(crate) fn activate_sibling_callback(
        &mut self,
        callback: SiblingCallback,
        source_depth: crate::engine::DepthSize,
        store: &mut Store<'html, 'query>,
    ) {
        if !self.is_runner_active(callback.runner) {
            return;
        }
        if let Some(session) = self.runners.get_mut(callback.runner.index()) {
            let _ = session.activate_sibling(callback.runner, callback, source_depth, store);
        } else {
            debug_assert!(false, "sibling callback references unknown runner");
        }
        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();
    }

    pub(crate) fn activate_sibling_callbacks(
        &mut self,
        callbacks: &[SiblingCallback],
        source_depth: crate::engine::DepthSize,
        store: &mut Store<'html, 'query>,
    ) {
        for callback in callbacks {
            self.activate_sibling_callback(*callback, source_depth, store);
        }
    }

    #[inline(never)]
    fn back_sparse(
        runners: &mut Runners<'query, Q>,
        active_ids: &mut Vec<RunnerId>,
        xhtml_element: &'html str,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
    ) {
        active_ids.retain(|runner| {
            let session = &mut runners[runner.index()];
            let significant_close = session.back(*runner, xhtml_element, position, store);
            !(significant_close && session.early_exit())
        });
    }

    pub(crate) fn back(
        &mut self,
        xhtml_element: &'html str,
        position: &DocumentPosition,
        reader: &Reader<'html>,
        store: &mut Store<'html, 'query>,
    ) -> bool {
        if let Some(active_ids) = self.active.as_mut() {
            Self::back_sparse(
                &mut self.runners,
                active_ids,
                xhtml_element,
                position,
                store,
            );
        } else {
            let mut any_retired = false;
            for (index, session) in self.runners.iter_mut().enumerate() {
                let significant_close =
                    session.back(RunnerId(index), xhtml_element, position, store);
                any_retired |= significant_close && session.early_exit();
            }
            if any_retired {
                let remaining = self
                    .runners
                    .iter()
                    .enumerate()
                    .filter_map(|(index, runner)| (!runner.early_exit()).then_some(RunnerId(index)))
                    .collect();
                self.active = Some(Box::new(remaining));
            }
        }
        let _ = reader;

        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();

        self.all_runners_retired()
    }

    #[inline]
    pub(crate) fn back_dense_nonretiring(
        &mut self,
        xhtml_element: &'html str,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
    ) {
        debug_assert!(self.active.is_none(), "dense close after runner retirement");
        debug_assert!(!self.features.has_retiring_runners);
        for (index, session) in self.runners.iter_mut().enumerate() {
            let _ = session.back(RunnerId(index), xhtml_element, position, store);
        }
        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();
    }

    #[cfg(test)]
    pub(crate) fn runner_slot_count(&self) -> usize {
        self.runners.len()
    }

    #[cfg(test)]
    pub(crate) fn active_set_is_dense(&self) -> bool {
        self.active.is_none()
    }

    #[cfg(test)]
    pub(crate) fn active_runner_ids(&self) -> &[RunnerId] {
        match &self.active {
            None => panic!("dense active set has implicit IDs"),
            Some(ids) => ids,
        }
    }

    #[cfg(test)]
    pub(crate) fn runner_slot_occupied(&self, runner: RunnerId) -> bool {
        self.is_runner_active(runner)
    }

    #[cfg(test)]
    pub(crate) fn retire_runner_for_test(&mut self, runner: RunnerId) {
        let mut ids: Vec<_> = match &self.active {
            None => (0..self.runners.len()).map(RunnerId).collect(),
            Some(ids) => ids.as_ref().clone(),
        };
        ids.retain(|active| *active != runner);
        self.active = Some(Box::new(ids));
    }

    #[cfg(test)]
    pub(crate) fn all_runners_retired_for_test(&self) -> bool {
        self.all_runners_retired()
    }

    #[cfg(test)]
    pub(crate) fn runner_query_source(&self, runner: RunnerId) -> Option<&'query str> {
        self.runners.get(runner.index()).map(|session| {
            session
                .query()
                .get_selection(crate::QuerySectionId(0))
                .source
        })
    }

    fn is_runner_active(&self, runner: RunnerId) -> bool {
        if runner.index() >= self.runners.len() {
            return false;
        }
        match &self.active {
            None => true,
            Some(ids) => ids.binary_search(&runner).is_ok(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActiveRunnerSet, QueryFeatures, QueryMultiplexer, RunnerId, SiblingCallback, query_features,
    };
    use crate::Position;
    use crate::store::{ElementId, Store};
    use crate::{Query, Reader, Save, XHtmlParser};

    fn features(selector: &str) -> QueryFeatures {
        let query = Query::all(selector, Save::none()).unwrap().build();
        query_features(&query)
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn multiplexer_dense_state_does_not_embed_sparse_vector() {
        assert_eq!(std::mem::size_of::<ActiveRunnerSet>(), 8);
        #[cfg(not(feature = "bench-internals"))]
        assert_eq!(std::mem::size_of::<QueryMultiplexer<'_, Query>>(), 40);
        #[cfg(feature = "bench-internals")]
        assert_eq!(std::mem::size_of::<QueryMultiplexer<'_, Query>>(), 64);
    }

    #[test]
    fn query_features_distinguish_plain_and_sibling_combinators() {
        assert_eq!(features("article"), QueryFeatures::default());
        assert_eq!(features("main article > p"), QueryFeatures::default());
        assert_eq!(
            features("h1 + p"),
            QueryFeatures {
                has_sibling: true,
                has_adjacent: true,
            }
        );
        assert_eq!(
            features("h1 ~ p"),
            QueryFeatures {
                has_sibling: true,
                has_adjacent: false,
            }
        );
    }

    #[test]
    fn multiplexer_features_aggregate_mixed_query_slice() {
        let queries = [
            Query::all("main > article", Save::none()).unwrap().build(),
            Query::all("h1 ~ p", Save::none()).unwrap().build(),
        ];
        let mux = QueryMultiplexer::new(&queries);

        assert!(mux.features().has_sibling_queries);
        assert!(!mux.features().has_adjacent_queries);
        assert!(!mux.features().has_retiring_runners);
    }

    #[test]
    fn multiplexer_features_detect_first_and_mixed_retirement() {
        let all_queries = [Query::all("p", Save::none()).unwrap().build()];
        let first_queries = [Query::first("p", Save::none()).unwrap().build()];
        let mixed_queries = [
            Query::all("p", Save::none()).unwrap().build(),
            Query::first("h1", Save::none()).unwrap().build(),
        ];

        assert!(
            !QueryMultiplexer::new(&all_queries)
                .features()
                .has_retiring_runners
        );
        assert!(
            QueryMultiplexer::new(&first_queries)
                .features()
                .has_retiring_runners
        );
        assert!(
            QueryMultiplexer::new(&mixed_queries)
                .features()
                .has_retiring_runners
        );
    }

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
        assert_eq!(mux.runner_query_source(RunnerId(0)), Some("h1"));
    }

    #[test]
    fn active_list_removes_only_retired_runner() {
        let queries = [
            Query::first("h1", Save::none()).unwrap().build(),
            Query::all("section + p", Save::none()).unwrap().build(),
            Query::all("aside + footer", Save::none()).unwrap().build(),
        ];
        let mut mux = QueryMultiplexer::new(&queries);

        mux.retire_runner_for_test(RunnerId(1));

        assert_eq!(mux.active_runner_ids(), &[RunnerId(0), RunnerId(2)]);
        assert_eq!(mux.runner_slot_count(), 3);
        assert!(!mux.runner_slot_occupied(RunnerId(1)));
        assert!(mux.runner_slot_occupied(RunnerId(2)));
        assert_eq!(mux.runner_query_source(RunnerId(2)), Some("aside + footer"));
    }

    #[test]
    fn simultaneous_retirement_transitions_dense_to_ordered_sparse() {
        let queries = [
            Query::first("h1", Save::none()).unwrap().build(),
            Query::first("footer", Save::none()).unwrap().build(),
            Query::first("h1", Save::none()).unwrap().build(),
            Query::first("footer", Save::none()).unwrap().build(),
        ];
        let mut parser = XHtmlParser::new(QueryMultiplexer::new(&queries));
        let mut reader = Reader::new("<main><h1></h1><footer></footer></main>");

        assert!(parser.next(&mut reader)); // <main>
        assert!(parser.next(&mut reader)); // <h1>
        assert!(parser.next(&mut reader)); // </h1>: runners 0 and 2 retire together

        assert_eq!(
            parser.selectors.active_runner_ids(),
            &[RunnerId(1), RunnerId(3)]
        );
        assert_eq!(parser.selectors.runner_slot_count(), 4);
        assert_eq!(
            parser.selectors.runner_query_source(RunnerId(0)),
            Some("h1")
        );
        assert_eq!(
            parser.selectors.runner_query_source(RunnerId(3)),
            Some("footer")
        );
    }

    #[test]
    fn isolated_first_retirement_close_compacts_dense_active_set() {
        let queries: Vec<_> = (0..64)
            .map(|_| Query::first(".early", Save::none()).unwrap().build())
            .collect();
        let mut parser = XHtmlParser::new(QueryMultiplexer::new(&queries));
        let mut reader = Reader::new("<main><h1 class=\"early\"></h1></main>");

        assert!(parser.selectors.active_set_is_dense());
        assert_eq!(parser.selectors.runner_slot_count(), 64);

        assert!(parser.next(&mut reader)); // <main>
        assert!(parser.next(&mut reader)); // <h1>
        assert!(parser.selectors.active_set_is_dense());

        // Closing </h1> retires every First runner; next() returns false because
        // early-exit drains the remainder of the document.
        assert!(!parser.next(&mut reader));

        assert!(!parser.selectors.active_set_is_dense());
        assert!(parser.selectors.active_runner_ids().is_empty());
        assert!(parser.selectors.all_runners_retired_for_test());
        assert_eq!(parser.selectors.runner_slot_count(), 64);
        assert_eq!(
            parser.selectors.runner_query_source(RunnerId(0)),
            Some(".early")
        );
        assert_eq!(
            parser.selectors.runner_query_source(RunnerId(63)),
            Some(".early")
        );
    }

    #[test]
    fn callback_to_retired_runner_is_ignored() {
        let queries = [
            Query::first("h1", Save::none()).unwrap().build(),
            Query::all("section + p", Save::none()).unwrap().build(),
        ];
        let mut mux = QueryMultiplexer::new(&queries);
        let mut store = Store::with_capacity(0);

        mux.retire_runner_for_test(RunnerId(0));

        mux.activate_sibling_callback(
            SiblingCallback {
                runner: RunnerId(0),
                output_parent: ElementId(0),
                continuation: Position {
                    selection: crate::QuerySectionId(0),
                    state: crate::TransitionId(0),
                },
            },
            0,
            &mut store,
        );

        assert!(!mux.runner_slot_occupied(RunnerId(0)));
        assert!(mux.runner_slot_occupied(RunnerId(1)));
        assert_eq!(store.elements.len(), 0);
    }

    #[test]
    fn all_runners_retired_when_active_list_empty() {
        let queries = [
            Query::first("h1", Save::none()).unwrap().build(),
            Query::first("p", Save::none()).unwrap().build(),
        ];
        let mut mux = QueryMultiplexer::new(&queries);

        assert!(!mux.all_runners_retired_for_test());
        mux.retire_runner_for_test(RunnerId(0));
        assert!(!mux.all_runners_retired_for_test());
        mux.retire_runner_for_test(RunnerId(1));
        assert!(mux.active_runner_ids().is_empty());
        assert!(mux.all_runners_retired_for_test());
    }

    #[test]
    fn empty_multiplexer_uses_safe_dense_representation() {
        let queries: [Query<'static>; 0] = [];
        let mux = QueryMultiplexer::new(&queries);

        assert!(mux.active.is_none());
        assert!(mux.all_runners_retired_for_test());
    }
}
