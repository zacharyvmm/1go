use super::executor::QueryExecutor;
use crate::XHtmlElement;
use crate::store::ElementId;
use crate::store::Store;
use crate::{QuerySpec, Reader};

pub(crate) struct DocumentPosition {
    pub reader_position: usize,
    pub text_content_position: usize,
    pub element_depth: crate::engine::DepthSize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SaveHit {
    pub element_id: ElementId,
    pub save_inner_html: bool,
    pub save_text_content: bool,
}

//type Runner<'query, Q> = SmallVec<[QueryExecutor<'query, Q>; 1]>;
type Runner<'query, Q> = Vec<QueryExecutor<'query, Q>>;

pub struct QueryMultiplexer<'query, Q> {
    runners: Runner<'query, Q>,
}

impl<'html, 'query: 'html, Q> QueryMultiplexer<'query, Q>
where
    Q: QuerySpec<'query>,
{
    pub fn new(queries: &'query [Q]) -> Self {
        Self {
            #[allow(clippy::redundant_closure)]
            runners: queries
                .iter()
                .map(|query| QueryExecutor::new(query))
                .collect::<Runner<'query, Q>>(),
        }
    }

    pub(crate) fn next(
        &mut self,
        xhtml_element: &XHtmlElement<'html>,
        position: &DocumentPosition,
        store: &mut Store<'html, 'query>,
    ) -> Vec<SaveHit> {
        let len = store.elements.len();
        let mut save_hits = Vec::new();
        for (runner_index, session) in self.runners.iter_mut().enumerate() {
            session.next(runner_index, xhtml_element, position, store, &mut save_hits);
        }
        if len == store.elements.len() {
            // Element was not saved
            // Thus delete from the tape
            xhtml_element.remove_attributes(&mut store.attributes);
        }
        save_hits
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
            let early_exit_previous = session.early_exit();
            let back = session.back(index, xhtml_element, position, store);

            // Only trigger early_exit if the cursor was already at the
            // exit section *before* back(). This prevents premature runner
            // removal when step_backward moves the cursor into the exit
            // section from a child section (the parent's content hasn't
            // been captured yet).
            if back && early_exit_previous {
                remove_indices.push(index);
            }
        }
        let _ = reader;
        for idx in remove_indices.into_iter().rev() {
            self.runners.remove(idx);
        }

        self.runners.is_empty()
    }
}
