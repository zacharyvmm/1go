use super::attribute_interest::AttributeInterest;
use super::executor::QueryExecutor;
use crate::__private::ascii_case_insensitive_hash;
use crate::StructuralMatchContext;
use crate::XHtmlElement;
use crate::store::ElementId;
use crate::store::Store;
use crate::{LocalSelectorList, Position, QuerySectionId, StructuralPredicate};
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

type Runners<'query, Q> = Vec<QueryExecutor<'query, 'query, Q>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MultiplexerFeatures {
    pub(crate) has_sibling_queries: bool,
    pub(crate) has_selector_lists: bool,
    pub(crate) has_structural_queries: bool,
    pub(crate) needs_child_ordinals: bool,
    pub(crate) needs_type_ordinals: bool,
    pub(crate) needs_filtered_ordinals: bool,
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
                aggregate.has_sibling_queries |= query.has_sibling_combinator();
                aggregate.has_selector_lists |= query
                    .queries()
                    .iter()
                    .enumerate()
                    .any(|(index, _)| query.selection_ranges(QuerySectionId(index)).len() > 1);
                aggregate.has_structural_queries |= query.has_structural_queries();
                for transition in query.states() {
                    for predicate in transition.predicate().structural.as_slice() {
                        match predicate {
                            StructuralPredicate::FirstChild | StructuralPredicate::NthChild(_) => {
                                aggregate.needs_child_ordinals = true;
                            }
                            StructuralPredicate::FirstOfType
                            | StructuralPredicate::NthOfType(_) => {
                                aggregate.needs_type_ordinals = true;
                            }
                            StructuralPredicate::NthChildOf(_, _) => {
                                aggregate.needs_filtered_ordinals = true;
                            }
                            StructuralPredicate::Root | StructuralPredicate::Scope => {}
                        }
                    }
                }
                aggregate.has_retiring_runners |= query.exit_at_section_end().is_some();
                aggregate
            })
    }

    pub(crate) fn structural_attribute_interest(&self) -> Option<AttributeInterest<'query>> {
        let mut interest = AttributeInterest::default();
        for runner in &self.runners {
            for transition in runner.query().states() {
                for structural in transition.predicate().structural.as_slice() {
                    if let StructuralPredicate::NthChildOf(_, filter) = structural {
                        for predicate in filter.as_slice() {
                            interest.add_predicate(predicate);
                        }
                    }
                }
            }
        }
        (!interest.is_empty()).then_some(interest)
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

    /// Update peak cursor counts from all resident executor storage.
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

    pub(crate) fn requires_attribute_storage(&self) -> bool {
        self.runners
            .iter()
            .any(|runner| runner.query().requires_attribute_storage())
    }

    pub(crate) fn requires_attribute_parsing(&self) -> bool {
        self.runners
            .iter()
            .any(|runner| runner.query().requires_attribute_parsing())
    }

    pub(crate) fn allows_early_exit(&self) -> bool {
        self.runners
            .iter()
            .all(|runner| runner.query().exit_at_section_end().is_some())
    }

    /// Whether an active runner may inspect or save attributes for this name.
    #[inline(always)]
    pub(crate) fn prepare_element<const SIBLINGS: bool, const RETIREMENT: bool>(
        &self,
        name: &str,
        preflight: &mut ElementPreflight<'query>,
    ) {
        preflight.attribute_interest.clear();
        preflight.runner_indices.clear();
        preflight.runner_len = self.runners.len();
        let name_hash = ascii_case_insensitive_hash(name);

        if RETIREMENT && let Some(ids) = &self.active {
            for runner_id in ids.iter().copied() {
                let runner_index = runner_id.index();
                if self.runners[runner_index].extend_attribute_interest_for::<SIBLINGS>(
                    name,
                    name_hash,
                    &mut preflight.attribute_interest,
                ) {
                    preflight.runner_indices.push(runner_index);
                }
            }
            return;
        }

        debug_assert!(RETIREMENT || self.active.is_none());
        for (runner_index, runner) in self.runners.iter().enumerate() {
            if runner.extend_attribute_interest_for::<SIBLINGS>(
                name,
                name_hash,
                &mut preflight.attribute_interest,
            ) {
                preflight.runner_indices.push(runner_index);
            }
        }
    }

    #[inline(always)]
    pub(crate) fn prepare_element_with_structural_interest<
        const SIBLINGS: bool,
        const RETIREMENT: bool,
    >(
        &self,
        name: &str,
        preflight: &mut ElementPreflight<'query>,
        structural_interest: &AttributeInterest<'query>,
    ) {
        self.prepare_element::<SIBLINGS, RETIREMENT>(name, preflight);
        preflight.attribute_interest.merge(structural_interest);
    }

    #[inline]
    pub(crate) fn features(&self) -> MultiplexerFeatures {
        self.features
    }

    pub(crate) fn structural_filters(&self) -> Vec<&'query LocalSelectorList<'query>> {
        let mut filters: Vec<&'query LocalSelectorList<'query>> = Vec::new();
        for runner in &self.runners {
            for filter in runner.query.structural_filters() {
                if !filters
                    .iter()
                    .any(|existing| std::ptr::eq(*existing, filter))
                {
                    filters.push(filter);
                }
            }
        }
        filters
    }

    pub(crate) fn type_ordinal_names(&self) -> Option<SmallVec<[&'query str; 4]>> {
        let mut names: SmallVec<[&'query str; 4]> = SmallVec::new();
        for runner in &self.runners {
            for transition in runner.query.states() {
                let needs_type_ordinal =
                    transition
                        .predicate()
                        .structural
                        .as_slice()
                        .iter()
                        .any(|predicate| {
                            matches!(
                                predicate,
                                StructuralPredicate::FirstOfType
                                    | StructuralPredicate::NthOfType(_)
                            )
                        });
                if !needs_type_ordinal {
                    continue;
                }
                let name = transition.predicate().name?;
                if !names
                    .iter()
                    .any(|existing| name.eq_ignore_ascii_case(existing))
                {
                    names.push(name);
                }
            }
        }
        Some(names)
    }

    // Preserve the ordinary executor's inlining across this thin dispatch
    // layer; otherwise x86-64 keeps the wrapper in the parser hot loop.
    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn next_plain_into(
        &mut self,
        xhtml_element: &XHtmlElement<'html>,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        save_hits: &mut Vec<SaveHit>,
        preflight: &ElementPreflight<'query>,
    ) {
        debug_assert_eq!(self.runners.len(), preflight.runner_len);
        save_hits.clear();
        for &runner_index in &preflight.runner_indices {
            self.runners[runner_index].next_plain(
                RunnerId(runner_index),
                xhtml_element,
                position,
                store,
                save_hits,
            );
        }
        #[cfg(any(debug_assertions, test))]
        self.trace_preflight_rejections(xhtml_element, position, store, preflight);
        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();
    }

    #[inline(always)]
    pub(crate) fn next_plain_into_with_context(
        &mut self,
        xhtml_element: &XHtmlElement<'html>,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        save_hits: &mut Vec<SaveHit>,
        preflight: &ElementPreflight<'query>,
        structural: Option<&StructuralMatchContext<'query>>,
    ) {
        debug_assert_eq!(self.runners.len(), preflight.runner_len);
        save_hits.clear();
        for &runner_index in &preflight.runner_indices {
            self.runners[runner_index].next_plain_with_context(
                RunnerId(runner_index),
                xhtml_element,
                position,
                store,
                save_hits,
                structural,
            );
        }
        #[cfg(any(debug_assertions, test))]
        self.trace_preflight_rejections(xhtml_element, position, store, preflight);
        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();
    }

    #[inline(never)]
    #[allow(dead_code)]
    pub(crate) fn next_with_siblings_into(
        &mut self,
        xhtml_element: &XHtmlElement<'html>,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        save_hits: &mut Vec<SaveHit>,
        preflight: &ElementPreflight<'query>,
        sibling_callbacks: &mut Vec<SiblingCallback>,
    ) {
        self.next_with_siblings_into_with_context(
            xhtml_element,
            position,
            store,
            save_hits,
            preflight,
            sibling_callbacks,
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn next_with_siblings_into_with_context(
        &mut self,
        xhtml_element: &XHtmlElement<'html>,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        save_hits: &mut Vec<SaveHit>,
        preflight: &ElementPreflight<'query>,
        sibling_callbacks: &mut Vec<SiblingCallback>,
        structural: Option<&StructuralMatchContext<'query>>,
    ) {
        debug_assert_eq!(self.runners.len(), preflight.runner_len);
        save_hits.clear();
        sibling_callbacks.clear();
        for &runner_index in &preflight.runner_indices {
            self.runners[runner_index].next_with_siblings(
                RunnerId(runner_index),
                xhtml_element,
                position,
                store,
                save_hits,
                sibling_callbacks,
                structural,
            );
        }
        #[cfg(any(debug_assertions, test))]
        self.trace_preflight_rejections(xhtml_element, position, store, preflight);
        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();
    }

    #[cfg(any(debug_assertions, test))]
    fn trace_preflight_rejections(
        &self,
        xhtml_element: &XHtmlElement<'html>,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
        preflight: &ElementPreflight<'query>,
    ) {
        for (runner_index, runner) in self.runners.iter().enumerate() {
            if self.is_runner_active(RunnerId(runner_index))
                && !preflight.runner_indices.contains(&runner_index)
            {
                runner.trace_name_rejections(
                    runner_index,
                    xhtml_element,
                    position.element_depth,
                    store,
                );
            }
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
            let retire = significant_close && session.early_exit();
            if retire {
                session.release_cursor_storage();
            }
            !retire
        });
    }

    pub(crate) fn back<const RETIREMENT: bool>(
        &mut self,
        xhtml_element: &'html str,
        position: &DocumentPosition,
        reader: &Reader<'html>,
        store: &mut Store<'html, 'query>,
    ) -> bool {
        if !RETIREMENT {
            debug_assert!(self.active.is_none());
            for (index, session) in self.runners.iter_mut().enumerate() {
                let _ = session.back(RunnerId(index), xhtml_element, position, store);
            }
            let _ = reader;
            #[cfg(feature = "bench-internals")]
            self.track_cursor_stats();
            return false;
        }

        if let Some(active_ids) = self.active.as_mut() {
            Self::back_sparse(
                &mut self.runners,
                active_ids,
                xhtml_element,
                position,
                store,
            );
        } else {
            let runner_count = self.runners.len();
            let mut remaining = None;
            for (index, session) in self.runners.iter_mut().enumerate() {
                let significant_close =
                    session.back(RunnerId(index), xhtml_element, position, store);
                let retire = significant_close && session.early_exit();
                if retire {
                    session.release_cursor_storage();
                    if remaining.is_none() {
                        let mut ids = Vec::with_capacity(runner_count.saturating_sub(1));
                        ids.extend((0..index).map(RunnerId));
                        remaining = Some(ids);
                    }
                } else if let Some(ids) = remaining.as_mut() {
                    ids.push(RunnerId(index));
                }
            }
            if let Some(remaining) = remaining {
                self.active = Some(Box::new(remaining));
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
    pub(crate) fn runner_cursor_capacity(&self, runner: RunnerId) -> usize {
        self.runners[runner.index()].cursors.capacity()
    }

    #[cfg(test)]
    pub(crate) fn retire_runner_for_test(&mut self, runner: RunnerId) {
        self.runners[runner.index()].release_cursor_storage();
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
    use super::{ActiveRunnerSet, ElementPreflight, QueryMultiplexer, RunnerId, SiblingCallback};
    use crate::Position;
    use crate::store::{ElementId, Store};
    use crate::{Query, Reader, Save, XHtmlParser};

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
    fn multiplexer_features_aggregate_mixed_query_slice() {
        let queries = [
            Query::all("main > article", Save::none()).unwrap().build(),
            Query::all("h1 ~ p", Save::none()).unwrap().build(),
        ];
        let mux = QueryMultiplexer::new(&queries);

        assert!(mux.features().has_sibling_queries);
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
    fn multiplexer_features_specialize_structural_bookkeeping() {
        let root = [Query::all(":root", Save::none()).unwrap().build()];
        let child = [Query::all("li:first-child", Save::none()).unwrap().build()];
        let type_ordinal = [Query::all("li:nth-of-type(2)", Save::none())
            .unwrap()
            .build()];
        let filtered = [Query::all("li:nth-child(2 of .hit)", Save::none())
            .unwrap()
            .build()];

        let root_features = QueryMultiplexer::new(&root).features();
        assert!(!root_features.needs_child_ordinals);
        assert!(!root_features.needs_type_ordinals);
        assert!(!root_features.needs_filtered_ordinals);

        let child_features = QueryMultiplexer::new(&child).features();
        assert!(child_features.needs_child_ordinals);
        assert!(!child_features.needs_type_ordinals);
        assert!(!child_features.needs_filtered_ordinals);

        let type_features = QueryMultiplexer::new(&type_ordinal).features();
        assert!(!type_features.needs_child_ordinals);
        assert!(type_features.needs_type_ordinals);
        assert!(!type_features.needs_filtered_ordinals);

        let filtered_features = QueryMultiplexer::new(&filtered).features();
        assert!(!filtered_features.needs_child_ordinals);
        assert!(!filtered_features.needs_type_ordinals);
        assert!(filtered_features.needs_filtered_ordinals);
    }

    #[test]
    fn type_ordinal_interest_tracks_named_types_and_falls_back_for_universal_queries() {
        let named = [
            Query::all("li:nth-of-type(2)", Save::none())
                .unwrap()
                .build(),
            Query::all("SPAN:first-of-type", Save::none())
                .unwrap()
                .build(),
        ];
        let names = QueryMultiplexer::new(&named)
            .type_ordinal_names()
            .expect("named selectors have finite type interest");
        assert_eq!(names.as_slice(), &["li", "SPAN"]);

        let universal = [Query::all("*:nth-of-type(2)", Save::none())
            .unwrap()
            .build()];
        assert!(
            QueryMultiplexer::new(&universal)
                .type_ordinal_names()
                .is_none()
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
        assert_eq!(mux.runner_cursor_capacity(RunnerId(0)), 0);

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

        assert!(parser.selectors.active_set_is_dense());
        assert!(parser.next(&mut reader)); // <main>
        assert!(parser.next(&mut reader)); // <h1>
        assert!(parser.next(&mut reader)); // </h1>: runners 0 and 2 retire together

        assert!(!parser.selectors.active_set_is_dense());
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
        assert_eq!(parser.selectors.runner_cursor_capacity(RunnerId(0)), 0);
        assert_eq!(parser.selectors.runner_cursor_capacity(RunnerId(2)), 0);
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

    #[test]
    fn preflight_carries_only_name_viable_runners_and_their_attributes() {
        let queries = [
            Query::all("a[href]", Save::name_only()).unwrap().build(),
            Query::all("span.hero", Save::name_only()).unwrap().build(),
            Query::all("[data-x]", Save::name_only()).unwrap().build(),
        ];
        let selectors = QueryMultiplexer::new(&queries);
        let mut preflight = ElementPreflight::default();

        selectors.prepare_element::<false, false>("span", &mut preflight);

        assert_eq!(preflight.runner_indices.as_slice(), &[1, 2]);
        assert!(preflight.attribute_interest.includes_class());
        assert!(preflight.attribute_interest.includes_attribute("data-x"));
        assert!(!preflight.attribute_interest.includes_attribute("href"));
    }

    #[test]
    fn filtered_ordinal_preflight_collects_only_filter_attributes() {
        let queries = [
            Query::all("li:nth-child(2 of .hit, [data-card])", Save::none())
                .unwrap()
                .build(),
        ];
        let selectors = QueryMultiplexer::new(&queries);
        let mut preflight = ElementPreflight::default();

        let structural_interest = selectors.structural_attribute_interest().unwrap();
        selectors.prepare_element_with_structural_interest::<false, false>(
            "div",
            &mut preflight,
            &structural_interest,
        );

        assert!(preflight.runner_indices.is_empty());
        assert!(preflight.attribute_interest.includes_class());
        assert!(preflight.attribute_interest.includes_attribute("data-card"));
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
