use crate::store::ElementId;
use crate::{
    Combinator, QuerySection, QuerySpec, Reader, Save, SelectionKind, Store, TransitionId,
    XHtmlElement,
};
use super::tag_utils::{
    find_next_tag, find_tag_end, is_raw_text_tag, tag_info, tag_name_eq,
    tag_name_eq_ignore_ascii_case, tag_name_str,
};
use std::ops::Range;

#[derive(Clone, Copy)]
struct SimpleTagQuery<'query> {
    tag: &'query str,
    source: &'query str,
    save: Save,
}

struct OpenCapture {
    element_id: ElementId,
    inner_html_start: usize,
    text_content_start: usize,
}

pub(crate) fn parse_if_simple_tag<'html: 'query, 'query: 'html, Q>(
    html: &'html str,
    queries: &[Q],
) -> Option<Store<'html, 'query>>
where
    Q: QuerySpec<'query>,
{
    let simple = simple_tag_query(queries)?;
    Some(parse_simple_tag(html, simple))
}

fn simple_tag_query<'query, Q>(queries: &[Q]) -> Option<SimpleTagQuery<'query>>
where
    Q: QuerySpec<'query>,
{
    if queries.len() != 1 {
        return None;
    }

    let query = &queries[0];
    let states = query.states();
    let sections = query.queries();
    if states.len() != 1 || sections.len() != 1 {
        return None;
    }

    let transition = &states[0];
    if transition.guard != Combinator::Descendant {
        return None;
    }

    let predicate = &transition.predicate;
    if predicate.id.is_some()
        || !predicate.classes.as_slice().is_empty()
        || !predicate.attributes.as_slice().is_empty()
    {
        return None;
    }

    let section = &sections[0];
    if section.kind != SelectionKind::All
        || section.parent.is_some()
        || section.range.start.index() != 0
        || section.range.end.index() != 1
    {
        return None;
    }

    Some(SimpleTagQuery {
        tag: predicate.name?,
        source: section.source,
        save: section.save,
    })
}

fn parse_simple_tag<'html: 'query, 'query: 'html>(
    html: &'html str,
    simple: SimpleTagQuery<'query>,
) -> Store<'html, 'query> {
    let mut store = if simple.save.text_content {
        Store::with_capacity(html.len())
    } else {
        Store::default()
    };
    let section = QuerySection::new(
        simple.source,
        simple.save,
        SelectionKind::All,
        TransitionId(0)..TransitionId(1),
        None,
    );
    let input = html.as_bytes();
    let mut reader = Reader::new(html);
    let mut captures: Vec<OpenCapture> = Vec::new();
    let mut text_start: Option<usize> = None;
    let mut search_pos = 0;

    while let Some(tag_start) = find_next_tag(input, search_pos) {
        let Some(tag_end) = find_tag_end(input, tag_start + 1) else {
            break;
        };

        if simple.save.text_content && !captures.is_empty() {
            push_text_segment(
                &mut store,
                &reader,
                text_start.unwrap_or(tag_start),
                tag_start,
            );
            text_start = Some(tag_end + 1);
        }

        let Some(info) = tag_info(input, tag_start, tag_end) else {
            search_pos = tag_end + 1;
            continue;
        };

        if !info.is_close
            && !tag_name_eq(input, &info.name, simple.tag)
            && is_raw_text_tag(input, &info.name)
            && let Some((raw_close_start, raw_close_end)) =
                find_raw_text_close(input, tag_name_str(input, &info.name), tag_end + 1)
        {
            if simple.save.text_content && !captures.is_empty() {
                push_text_segment(
                    &mut store,
                    &reader,
                    text_start.unwrap_or(tag_end + 1),
                    raw_close_start,
                );
                text_start = Some(raw_close_end + 1);
            }
            search_pos = raw_close_end + 1;
            continue;
        }

        if info.is_close {
            if tag_name_eq(input, &info.name, simple.tag)
                && let Some(capture) = captures.pop()
            {
                let inner_html = simple
                    .save
                    .inner_html
                    .then(|| reader.slice(capture.inner_html_start..tag_start));
                let text_content = simple
                    .save
                    .text_content
                    .then(|| text_content_range(&store, capture.text_content_start))
                    .flatten();
                store.set_content(capture.element_id, inner_html, text_content);

                if simple.save.text_content {
                    text_start = (!captures.is_empty()).then_some(tag_end + 1);
                }
            }

            search_pos = tag_end + 1;
            continue;
        }

        if tag_name_eq(input, &info.name, simple.tag) {
            reader.set_position(tag_start + 1);
            let mut element = XHtmlElement::default();
            element.from(&mut reader, &mut store.attributes);
            let is_self_closing = info.is_self_closing || element.is_self_closing();
            let element_id = store.push(ElementId::default(), &section, element);

            if !is_self_closing && (simple.save.inner_html || simple.save.text_content) {
                let text_content_start = if simple.save.text_content {
                    current_text_position(&store)
                } else {
                    usize::MAX
                };
                captures.push(OpenCapture {
                    element_id,
                    inner_html_start: tag_end + 1,
                    text_content_start,
                });
                if simple.save.text_content && text_start.is_none() {
                    text_start = Some(tag_end + 1);
                }
            }
        }

        search_pos = tag_end + 1;
    }

    if simple.save.text_content
        && !captures.is_empty()
        && let Some(start) = text_start
    {
        push_text_segment(&mut store, &reader, start, input.len());
    }

    while let Some(capture) = captures.pop() {
        let inner_html = simple
            .save
            .inner_html
            .then(|| reader.slice(capture.inner_html_start..input.len()));
        let text_content = simple
            .save
            .text_content
            .then(|| text_content_range(&store, capture.text_content_start))
            .flatten();
        store.set_content(capture.element_id, inner_html, text_content);
    }

    store
}

#[inline]
fn current_text_position(store: &Store<'_, '_>) -> usize {
    if store.text_content.is_empty() {
        usize::MAX
    } else {
        store.text_content.get_position()
    }
}

#[inline]
fn text_content_range(store: &Store<'_, '_>, text_content_start: usize) -> Option<Range<usize>> {
    if store.text_content.is_empty() {
        return None;
    }

    let end = store.text_content.get_position();
    if text_content_start == usize::MAX {
        Some(0..end)
    } else if text_content_start == end {
        None
    } else {
        Some((text_content_start + 1)..end)
    }
}

#[inline]
fn push_text_segment<'html>(
    store: &mut Store<'html, '_>,
    reader: &Reader<'html>,
    start: usize,
    end: usize,
) {
    if start <= end {
        store.text_content.set_start(start);
        let _ = store.text_content.push(reader, end);
    }
}

fn find_raw_text_close(input: &[u8], tag: &str, mut search_pos: usize) -> Option<(usize, usize)> {
    while let Some(tag_start) = find_next_tag(input, search_pos) {
        let tag_end = find_tag_end(input, tag_start + 1)?;
        if let Some(info) = tag_info(input, tag_start, tag_end)
            && info.is_close
            && tag_name_eq_ignore_ascii_case(input, &info.name, tag)
        {
            return Some((tag_start, tag_end));
        }
        search_pos = tag_end + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Query, Save, parse};

    #[test]
    fn simple_anchor_fast_path_matches_streaming_parser() {
        let html = r#"<main><a href="/one"><b>One</b> &lt;1&gt;</a><span>x</span><a class="next" href="/two">Two</a></main>"#;
        let queries = &[Query::all("a", Save::all()).unwrap().build()];

        let fast = parse_if_simple_tag(html, queries).unwrap();
        let streaming = parse(html, queries);

        let fast_links: Vec<_> = fast.get("a").unwrap().collect();
        let streaming_links: Vec<_> = streaming.get("a").unwrap().collect();
        assert_eq!(fast_links.len(), streaming_links.len());

        for (fast_link, streaming_link) in fast_links.iter().zip(streaming_links) {
            assert_eq!(fast_link.name, streaming_link.name);
            assert_eq!(fast_link.inner_html, streaming_link.inner_html);
            assert_eq!(
                fast_link.text_content(&fast),
                streaming_link.text_content(&streaming)
            );
            assert_eq!(
                fast_link.attribute(&fast, "href"),
                streaming_link.attribute(&streaming, "href")
            );
        }
    }

    #[test]
    fn attribute_selector_uses_general_parser() {
        let queries = &[Query::all("a[href]", Save::all()).unwrap().build()];
        assert!(simple_tag_query(queries).is_none());
    }
}
