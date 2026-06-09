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
    ) {
        let len = store.elements.len();
        save_hits.clear();
        for (runner_index, session) in self.runners.iter_mut().enumerate() {
            session.next(runner_index, xhtml_element, position, store, save_hits);
        }
        if len == store.elements.len() {
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
            let early_exit_previous = session.early_exit();
            let back = session.back(index, xhtml_element, position, store);

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
