use crate::engine::DepthSize;
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
    pub saved: Vec<SavedElement>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct OpenElementStack<'html> {
    entries: Vec<OpenElement<'html>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeKind {
    Default,
    ListItem,
    Button,
    Table,
    Select,
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
        self.entries.len().try_into().unwrap_or(DepthSize::MAX)
    }

    pub fn push(&mut self, name: &'html str) {
        self.entries.push(OpenElement {
            name,
            saved: Vec::new(),
        });
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

    pub fn prepare_for_open(&mut self, name: &str) -> Vec<OpenElement<'html>> {
        let mut popped = Vec::new();

        if closes_open_p(name) {
            popped.extend(self.pop_matching_in_scope(&["p"], ScopeKind::Default));
        }

        match name {
            "button" => popped.extend(self.pop_matching_in_scope(&["button"], ScopeKind::Button)),
            "li" => popped.extend(self.pop_matching_in_scope(&["li"], ScopeKind::ListItem)),
            "dt" | "dd" => {
                popped.extend(self.pop_matching_in_scope(&["dt", "dd"], ScopeKind::ListItem))
            }
            "option" => popped.extend(self.pop_matching_in_scope(&["option"], ScopeKind::Select)),
            "optgroup" => {
                popped.extend(self.pop_matching_in_scope(&["option"], ScopeKind::Select));
                popped.extend(self.pop_matching_in_scope(&["optgroup"], ScopeKind::Select));
            }
            "tr" => popped.extend(self.pop_matching_in_scope(&["tr"], ScopeKind::Table)),
            "td" | "th" => {
                popped.extend(self.pop_matching_in_scope(&["td", "th"], ScopeKind::Table))
            }
            _ => {}
        }

        popped
    }

    #[cfg(test)]
    pub fn close_by_end_tag(&mut self, name: &str) -> Vec<OpenElement<'html>> {
        let mut popped = Vec::new();
        self.close_by_end_tag_into(name, &mut popped);
        popped
    }

    pub fn close_by_end_tag_into(&mut self, name: &str, popped: &mut Vec<OpenElement<'html>>) {
        popped.clear();
        let scope = close_scope(name);
        if let Some(index) = self.find_matching_index(name, scope) {
            while self.entries.len() > index {
                if let Some(open) = self.entries.pop() {
                    popped.push(open);
                }
            }
        }
    }

    pub fn close_all_at_eof(&mut self) -> Vec<OpenElement<'html>> {
        self.entries.drain(..).rev().collect()
    }

    fn pop_matching_in_scope(
        &mut self,
        names: &[&str],
        scope: ScopeKind,
    ) -> Vec<OpenElement<'html>> {
        if let Some(index) = self.find_first_of(names, scope) {
            let mut popped = Vec::with_capacity(self.entries.len() - index);
            while self.entries.len() > index {
                if let Some(open) = self.entries.pop() {
                    popped.push(open);
                }
            }
            popped
        } else {
            Vec::new()
        }
    }

    fn find_first_of(&self, names: &[&str], scope: ScopeKind) -> Option<usize> {
        for (index, entry) in self.entries.iter().enumerate().rev() {
            if names
                .iter()
                .any(|name| entry.name.eq_ignore_ascii_case(name))
            {
                return Some(index);
            }
            if is_scope_barrier(entry.name, scope) {
                return None;
            }
        }
        None
    }

    fn find_matching_index(&self, name: &str, scope: ScopeKind) -> Option<usize> {
        for (index, entry) in self.entries.iter().enumerate().rev() {
            if entry.name.eq_ignore_ascii_case(name) {
                return Some(index);
            }
            if is_scope_barrier(entry.name, scope) {
                return None;
            }
        }
        None
    }
}

fn close_scope(name: &str) -> ScopeKind {
    match name {
        "li" | "dt" | "dd" => ScopeKind::ListItem,
        "button" => ScopeKind::Button,
        "tr" | "td" | "th" | "thead" | "tbody" | "tfoot" | "caption" | "colgroup" => {
            ScopeKind::Table
        }
        "option" | "optgroup" => ScopeKind::Select,
        _ => ScopeKind::Default,
    }
}

fn closes_open_p(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "div"
            | "dl"
            | "fieldset"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "ul"
    )
}

fn is_scope_barrier(name: &str, scope: ScopeKind) -> bool {
    if matches!(name, "html" | "template") {
        return true;
    }

    match scope {
        ScopeKind::Default => matches!(
            name,
            "applet" | "marquee" | "object" | "table" | "td" | "th"
        ),
        ScopeKind::ListItem => matches!(
            name,
            "applet" | "marquee" | "object" | "table" | "td" | "th" | "ol" | "ul"
        ),
        ScopeKind::Button => {
            matches!(
                name,
                "applet" | "marquee" | "object" | "table" | "td" | "th" | "button"
            )
        }
        ScopeKind::Table => matches!(name, "html" | "table" | "template"),
        ScopeKind::Select => !matches!(name, "option" | "optgroup"),
    }
}

#[cfg(test)]
mod tests {
    use super::OpenElementStack;

    #[test]
    fn test_misnested_close_bubbles_to_match() {
        let mut stack = OpenElementStack::default();
        stack.push("div");
        stack.push("span");

        let popped = stack.close_by_end_tag("div");
        assert_eq!(popped.len(), 2);
        assert_eq!(popped[0].name, "span");
        assert_eq!(popped[1].name, "div");
    }

    #[test]
    fn test_stray_close_is_ignored() {
        let mut stack = OpenElementStack::default();
        stack.push("div");

        let popped = stack.close_by_end_tag("span");
        assert!(popped.is_empty());
        assert_eq!(stack.depth(), 1);
    }

    #[test]
    fn test_opening_li_closes_previous_li() {
        let mut stack = OpenElementStack::default();
        stack.push("ul");
        stack.push("li");

        let popped = stack.prepare_for_open("li");
        assert_eq!(popped.len(), 1);
        assert_eq!(popped[0].name, "li");
    }

    #[test]
    fn test_opening_option_closes_previous_option() {
        let mut stack = OpenElementStack::default();
        stack.push("select");
        stack.push("option");

        let popped = stack.prepare_for_open("option");
        assert_eq!(popped.len(), 1);
        assert_eq!(popped[0].name, "option");
    }

    #[test]
    fn test_opening_optgroup_closes_option_then_optgroup() {
        let mut stack = OpenElementStack::default();
        stack.push("select");
        stack.push("optgroup");
        stack.push("option");

        let popped = stack.prepare_for_open("optgroup");
        assert_eq!(popped.len(), 2);
        assert_eq!(popped[0].name, "option");
        assert_eq!(popped[1].name, "optgroup");
    }

    #[test]
    fn test_opening_td_closes_previous_cell() {
        let mut stack = OpenElementStack::default();
        stack.push("table");
        stack.push("tr");
        stack.push("td");

        let popped = stack.prepare_for_open("td");
        assert_eq!(popped.len(), 1);
        assert_eq!(popped[0].name, "td");
    }

    #[test]
    fn test_opening_button_closes_previous_button() {
        let mut stack = OpenElementStack::default();
        stack.push("div");
        stack.push("button");

        let popped = stack.prepare_for_open("button");
        assert_eq!(popped.len(), 1);
        assert_eq!(popped[0].name, "button");
        assert_eq!(stack.depth(), 1);
    }

    #[test]
    fn test_select_scope_ignores_non_select_end_tags() {
        let mut stack = OpenElementStack::default();
        stack.push("select");
        stack.push("option");

        let popped = stack.close_by_end_tag("div");
        assert!(popped.is_empty());
        assert_eq!(stack.depth(), 2);
    }
}
