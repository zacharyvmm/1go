use crate::ParseError;
use crate::engine::{DepthSize, MAX_ELEMENT_DEPTH};
use crate::html::tag::{ScopeKind, TagFlags};
use crate::html::text_state::{TextEdgePolicy, TextElementFlags};
use crate::store::ElementId;

/// Sentinel for an inactive deferred content-start offset.
///
/// A start of `usize::MAX` cannot be a valid tape or source index: Rust
/// allocations cannot have length `usize::MAX`, and reader/tape lengths must
/// remain representable and indexable. Any input near this limit would fail
/// allocation long before a range could be created.
const NO_START: usize = usize::MAX;

/// Deferred close-finalization record for a matched open element.
///
/// Optional start offsets are packed as plain `usize` values with [`NO_START`]
/// for `None`, so inactive modes (e.g. inner-HTML-only) do not pay for three
/// `Option<usize>` niches (typically 16 bytes each on 64-bit).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SavedElement {
    pub element_id: ElementId,
    inner_html_start: usize,
    raw_text_start: usize,
    text_start: usize,
    text_edge_policy: TextEdgePolicy,
}

impl SavedElement {
    #[inline]
    pub(crate) fn new(
        element_id: ElementId,
        inner_html_start: Option<usize>,
        raw_text_start: Option<usize>,
        text_start: Option<usize>,
        text_edge_policy: TextEdgePolicy,
    ) -> Self {
        Self {
            element_id,
            inner_html_start: inner_html_start.unwrap_or(NO_START),
            raw_text_start: raw_text_start.unwrap_or(NO_START),
            text_start: text_start.unwrap_or(NO_START),
            text_edge_policy,
        }
    }

    #[inline]
    pub(crate) fn inner_html_start(&self) -> Option<usize> {
        (self.inner_html_start != NO_START).then_some(self.inner_html_start)
    }

    #[inline]
    pub(crate) fn raw_text_start(&self) -> Option<usize> {
        (self.raw_text_start != NO_START).then_some(self.raw_text_start)
    }

    #[inline]
    pub(crate) fn text_start(&self) -> Option<usize> {
        (self.text_start != NO_START).then_some(self.text_start)
    }

    #[inline]
    pub(crate) fn text_edge_policy(&self) -> TextEdgePolicy {
        self.text_edge_policy
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OpenElement<'html> {
    pub name: &'html str,
    tag: TagFlags,
    pub text_flags: TextElementFlags,
    pub saved: Vec<SavedElement>,
}

impl<'html> OpenElement<'html> {
    #[inline]
    pub fn tag(&self) -> TagFlags {
        self.tag
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct OpenElementStack<'html> {
    entries: Vec<OpenElement<'html>>,
}

impl<'html> Default for OpenElementStack<'html> {
    fn default() -> Self {
        const ASSUMED_MAX_DEPTH: usize = 16;
        Self {
            entries: Vec::with_capacity(ASSUMED_MAX_DEPTH),
        }
    }
}

impl<'html> OpenElementStack<'html> {
    pub fn depth(&self) -> DepthSize {
        self.entries.len().try_into().unwrap_or(MAX_ELEMENT_DEPTH)
    }

    #[inline]
    pub(crate) const fn would_exceed_max_depth(len: usize) -> bool {
        len >= MAX_ELEMENT_DEPTH as usize
    }

    pub fn push_classified(
        &mut self,
        name: &'html str,
        tag: TagFlags,
        text_flags: TextElementFlags,
    ) -> Result<(), ParseError> {
        if Self::would_exceed_max_depth(self.entries.len()) {
            return Err(ParseError::MaximumDepthExceeded);
        }
        self.entries.push(OpenElement {
            name,
            tag,
            text_flags,
            saved: Vec::new(),
        });
        Ok(())
    }

    #[cfg(test)]
    pub fn push(&mut self, name: &'html str) -> Result<(), ParseError> {
        self.push_classified(name, TagFlags::classify(name), TextElementFlags::empty())
    }

    pub fn attach_saved(
        &mut self,
        element_id: ElementId,
        inner_html_start: Option<usize>,
        raw_text_start: Option<usize>,
        text_start: Option<usize>,
        text_edge_policy: TextEdgePolicy,
    ) {
        if let Some(open_element) = self.entries.last_mut() {
            open_element.saved.push(SavedElement::new(
                element_id,
                inner_html_start,
                raw_text_start,
                text_start,
                text_edge_policy,
            ));
        }
    }

    #[cfg(test)]
    pub(crate) fn last(&self) -> Option<&OpenElement<'html>> {
        self.entries.last()
    }

    #[cfg(test)]
    pub fn prepare_for_open(&mut self, name: &str) -> Vec<OpenElement<'html>> {
        let mut popped = Vec::new();
        self.prepare_for_open_into(TagFlags::classify(name), &mut popped);
        popped
    }

    pub fn prepare_for_open_into(&mut self, tag: TagFlags, popped: &mut Vec<OpenElement<'html>>) {
        popped.clear();

        if tag.closes_open_p() {
            self.pop_matching_in_scope_into(TagFlags::P_MASK, ScopeKind::Default, popped);
        }

        if tag.intersects(TagFlags::BUTTON_MASK) {
            self.pop_matching_in_scope_into(TagFlags::BUTTON_MASK, ScopeKind::Button, popped);
        } else if tag.intersects(TagFlags::LI_MASK) {
            self.pop_matching_in_scope_into(TagFlags::LI_MASK, ScopeKind::ListItem, popped);
        } else if tag.intersects(TagFlags::DT_DD_MASK) {
            self.pop_matching_in_scope_into(TagFlags::DT_DD_MASK, ScopeKind::ListItem, popped);
        } else if tag.intersects(TagFlags::OPTION_MASK) {
            self.pop_matching_in_scope_into(TagFlags::OPTION_MASK, ScopeKind::Select, popped);
        } else if tag.intersects(TagFlags::OPTGROUP_MASK) {
            self.pop_matching_in_scope_into(TagFlags::OPTION_MASK, ScopeKind::Select, popped);
            self.pop_matching_in_scope_into(TagFlags::OPTGROUP_MASK, ScopeKind::Select, popped);
        } else if tag.intersects(TagFlags::TR_MASK) {
            self.pop_matching_in_scope_into(TagFlags::TR_MASK, ScopeKind::Table, popped);
        } else if tag.intersects(TagFlags::CELL_MASK) {
            self.pop_matching_in_scope_into(TagFlags::CELL_MASK, ScopeKind::Table, popped);
        }
    }

    #[cfg(test)]
    pub fn close_by_end_tag(&mut self, name: &str) -> Vec<OpenElement<'html>> {
        let mut popped = Vec::new();
        self.close_by_end_tag_into(name, &mut popped);
        popped
    }

    pub fn close_by_end_tag_into(&mut self, name: &str, popped: &mut Vec<OpenElement<'html>>) {
        popped.clear();
        let tag = TagFlags::classify(name);
        if let Some(index) = self.find_matching_index(name, tag.close_scope()) {
            while self.entries.len() > index {
                if let Some(open) = self.entries.pop() {
                    popped.push(open);
                }
            }
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub fn close_all_at_eof(&mut self) -> Vec<OpenElement<'html>> {
        let mut popped = Vec::new();
        self.close_all_at_eof_into(&mut popped);
        popped
    }

    pub fn close_all_at_eof_into(&mut self, popped: &mut Vec<OpenElement<'html>>) {
        popped.clear();
        popped.extend(self.entries.drain(..).rev());
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn pop_matching_in_scope(
        &mut self,
        tags: TagFlags,
        scope: ScopeKind,
    ) -> Vec<OpenElement<'html>> {
        let mut popped = Vec::new();
        self.pop_matching_in_scope_into(tags, scope, &mut popped);
        popped
    }

    fn pop_matching_in_scope_into(
        &mut self,
        tags: TagFlags,
        scope: ScopeKind,
        popped: &mut Vec<OpenElement<'html>>,
    ) {
        if let Some(index) = self.find_first_of(tags, scope) {
            while self.entries.len() > index {
                if let Some(open) = self.entries.pop() {
                    popped.push(open);
                }
            }
        }
    }

    fn find_first_of(&self, tags: TagFlags, scope: ScopeKind) -> Option<usize> {
        for (index, entry) in self.entries.iter().enumerate().rev() {
            if entry.tag.intersects(tags) {
                return Some(index);
            }
            if entry.tag.is_scope_barrier(scope) {
                return None;
            }
        }
        None
    }

    fn find_matching_index(&self, name: &str, scope: ScopeKind) -> Option<usize> {
        for (index, entry) in self.entries.iter().enumerate().rev() {
            if entry.name == name || entry.name.eq_ignore_ascii_case(name) {
                return Some(index);
            }
            if entry.tag.is_scope_barrier(scope) {
                return None;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenElementStack, SavedElement};
    use crate::engine::MAX_ELEMENT_DEPTH;
    use crate::html::text_state::TextEdgePolicy;
    use crate::store::ElementId;

    #[test]
    fn saved_element_none_starts_round_trip() {
        let saved = SavedElement::new(
            ElementId::from(0usize),
            None,
            None,
            None,
            TextEdgePolicy::TrimCollapsedSeparators,
        );

        assert_eq!(saved.inner_html_start(), None);
        assert_eq!(saved.raw_text_start(), None);
        assert_eq!(saved.text_start(), None);
        assert_eq!(
            saved.text_edge_policy(),
            TextEdgePolicy::TrimCollapsedSeparators
        );
    }

    #[test]
    fn saved_element_present_starts_round_trip() {
        let saved = SavedElement::new(
            ElementId::from(1usize),
            Some(10),
            Some(20),
            Some(30),
            TextEdgePolicy::Preserve,
        );

        assert_eq!(saved.inner_html_start(), Some(10));
        assert_eq!(saved.raw_text_start(), Some(20));
        assert_eq!(saved.text_start(), Some(30));
        assert_eq!(saved.text_edge_policy(), TextEdgePolicy::Preserve);
    }

    #[test]
    fn saved_element_mixed_optional_starts_round_trip() {
        let cases = [
            (Some(1), None, None),
            (None, Some(2), None),
            (None, None, Some(3)),
            (Some(4), Some(5), None),
            (Some(6), None, Some(7)),
            (None, Some(8), Some(9)),
            (Some(10), Some(11), Some(12)),
            (None, None, None),
        ];
        for (inner, raw, text) in cases {
            let saved = SavedElement::new(
                ElementId::from(0usize),
                inner,
                raw,
                text,
                TextEdgePolicy::Preserve,
            );
            assert_eq!(saved.inner_html_start(), inner);
            assert_eq!(saved.raw_text_start(), raw);
            assert_eq!(saved.text_start(), text);
        }
    }

    #[test]
    fn saved_element_layout_is_compact_on_64bit() {
        use std::mem::size_of;
        // On 64-bit, sentinel-packed starts should restore the historical ~40-byte
        // deferred record (or at most 48 with alignment padding).
        if size_of::<usize>() == 8 {
            let size = size_of::<SavedElement>();
            assert!(
                size <= 48,
                "SavedElement grew unexpectedly: {size} bytes (expected <= 48)"
            );
        }
    }

    #[test]
    fn would_exceed_max_depth_at_boundary() {
        assert!(!OpenElementStack::would_exceed_max_depth(
            MAX_ELEMENT_DEPTH as usize - 1
        ));
        assert!(OpenElementStack::would_exceed_max_depth(
            MAX_ELEMENT_DEPTH as usize
        ));
    }

    #[test]
    fn test_misnested_close_bubbles_to_match() {
        let mut stack = OpenElementStack::default();
        stack.push("div").unwrap();
        stack.push("span").unwrap();

        let popped = stack.close_by_end_tag("div");
        assert_eq!(popped.len(), 2);
        assert_eq!(popped[0].name, "span");
        assert_eq!(popped[1].name, "div");
    }

    #[test]
    fn test_stray_close_is_ignored() {
        let mut stack = OpenElementStack::default();
        stack.push("div").unwrap();

        let popped = stack.close_by_end_tag("span");
        assert!(popped.is_empty());
        assert_eq!(stack.depth(), 1);
    }

    #[test]
    fn test_opening_li_closes_previous_li() {
        let mut stack = OpenElementStack::default();
        stack.push("ul").unwrap();
        stack.push("li").unwrap();

        let popped = stack.prepare_for_open("li");
        assert_eq!(popped.len(), 1);
        assert_eq!(popped[0].name, "li");
    }

    #[test]
    fn test_opening_option_closes_previous_option() {
        let mut stack = OpenElementStack::default();
        stack.push("select").unwrap();
        stack.push("option").unwrap();

        let popped = stack.prepare_for_open("option");
        assert_eq!(popped.len(), 1);
        assert_eq!(popped[0].name, "option");
    }

    #[test]
    fn test_opening_optgroup_closes_option_then_optgroup() {
        let mut stack = OpenElementStack::default();
        stack.push("select").unwrap();
        stack.push("optgroup").unwrap();
        stack.push("option").unwrap();

        let popped = stack.prepare_for_open("optgroup");
        assert_eq!(popped.len(), 2);
        assert_eq!(popped[0].name, "option");
        assert_eq!(popped[1].name, "optgroup");
    }

    #[test]
    fn test_opening_td_closes_previous_cell() {
        let mut stack = OpenElementStack::default();
        stack.push("table").unwrap();
        stack.push("tr").unwrap();
        stack.push("td").unwrap();

        let popped = stack.prepare_for_open("td");
        assert_eq!(popped.len(), 1);
        assert_eq!(popped[0].name, "td");
    }

    #[test]
    fn test_opening_button_closes_previous_button() {
        let mut stack = OpenElementStack::default();
        stack.push("div").unwrap();
        stack.push("button").unwrap();

        let popped = stack.prepare_for_open("button");
        assert_eq!(popped.len(), 1);
        assert_eq!(popped[0].name, "button");
        assert_eq!(stack.depth(), 1);
    }

    #[test]
    fn test_select_scope_ignores_non_select_end_tags() {
        let mut stack = OpenElementStack::default();
        stack.push("select").unwrap();
        stack.push("option").unwrap();

        let popped = stack.close_by_end_tag("div");
        assert!(popped.is_empty());
        assert_eq!(stack.depth(), 2);
    }
}
