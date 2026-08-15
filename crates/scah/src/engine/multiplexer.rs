use super::attribute_interest::AttributeInterest;
use super::executor::{QueryExecutor, ascii_case_insensitive_hash};
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
    runner_epoch: usize,
}

type Runner<'query, Q> = Vec<QueryExecutor<'query, 'query, Q>>;

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
    runner_epoch: usize,
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
            runner_epoch: 0,
            #[cfg(feature = "bench-internals")]
            cursor_stats: None,
        }
    }

    #[cfg(feature = "bench-internals")]
    pub(crate) fn new_with_cursor_stats(queries: &'query [Q]) -> Self {
        Self {
            runners: Self::build_runners(queries),
            runner_epoch: 0,
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

    /// Whether the active query frontier may inspect or save attributes for
    /// this element name.
    #[inline]
    pub(crate) fn prepare_element(&self, name: &str, preflight: &mut ElementPreflight<'query>) {
        preflight.attribute_interest.clear();
        preflight.runner_indices.clear();
        preflight.runner_epoch = self.runner_epoch;
        let name_hash = ascii_case_insensitive_hash(name);
        for (runner_index, runner) in self.runners.iter().enumerate() {
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
    ) {
        save_hits.clear();
        if preflight.runner_epoch == self.runner_epoch {
            for &runner_index in &preflight.runner_indices {
                self.runners[runner_index].next(
                    runner_index,
                    xhtml_element,
                    position,
                    store,
                    save_hits,
                );
            }
            #[cfg(any(debug_assertions, test))]
            for (runner_index, session) in self.runners.iter_mut().enumerate() {
                if !preflight.runner_indices.contains(&runner_index) {
                    // Debug traces intentionally retain predicate-rejection
                    // events for name-incompatible runners. This loop is
                    // compiled out of optimized release builds.
                    session.next(runner_index, xhtml_element, position, store, save_hits);
                }
            }
        } else {
            // An implied close can complete and remove a `First` runner after
            // tag-name preflight but before this open tag is executed. That is
            // uncommon; use the current runner set rather than stale indices.
            for (runner_index, session) in self.runners.iter_mut().enumerate() {
                session.next(runner_index, xhtml_element, position, store, save_hits);
            }
        }
        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();
        let retains_parsed_attributes = save_hits.iter().any(|hit| {
            store.elements[hit.element_id]
                .attributes
                .as_ref()
                .is_some_and(|range| !range.is_empty())
        });
        if !retains_parsed_attributes {
            xhtml_element.remove_attributes(&mut store.attributes);
        }
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
            self.runner_epoch = self.runner_epoch.wrapping_add(1);
        }

        #[cfg(feature = "bench-internals")]
        self.track_cursor_stats();

        self.runners.is_empty()
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
}
