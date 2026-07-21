use crate::ParseError;
use crate::engine::{DepthSize, MAX_ELEMENT_DEPTH};
use crate::html::tag::{ScopeKind, TagFlags};
use crate::store::ElementId;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SavedElement {
    pub element_id: ElementId,
    pub inner_html_start: Option<usize>,
    pub text_content_start: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OpenElement<'html> {
    pub name: &'html str,
    tag: TagFlags,
    pub saved: Vec<SavedElement>,
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

    pub fn push_classified(&mut self, name: &'html str, tag: TagFlags) -> Result<(), ParseError> {
        if Self::would_exceed_max_depth(self.entries.len()) {
            return Err(ParseError::MaximumDepthExceeded);
        }
        self.entries.push(OpenElement {
            name,
            tag,
            saved: Vec::new(),
        });
        Ok(())
    }

    #[cfg(test)]
    pub fn push(&mut self, name: &'html str) -> Result<(), ParseError> {
        self.push_classified(name, TagFlags::classify(name))
    }

    pub fn attach_saved(
        &mut self,
        element_id: ElementId,
        inner_html_start: Option<usize>,
        text_content_start: Option<usize>,
    ) {
        if let Some(open_element) = self.entries.last_mut() {
            open_element.saved.push(SavedElement {
                element_id,
                inner_html_start,
                text_content_start,
            });
        }
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
    use super::{OpenElement, OpenElementStack};
    use crate::engine::MAX_ELEMENT_DEPTH;

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn open_element_size_is_compact() {
        assert_eq!(std::mem::size_of::<OpenElement<'_>>(), 48);
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
