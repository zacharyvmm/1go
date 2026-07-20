use crate::{ElementId, QuerySectionId, TransitionId};
use std::fmt::Write as _;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct TraceStore<'html, 'query> {
    events: Vec<TraceEvent<'html, 'query>>,
}

impl<'html, 'query> TraceStore<'html, 'query> {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            events: Vec::with_capacity(capacity),
        }
    }

    pub fn push(&mut self, event: TraceEvent<'html, 'query>) {
        self.events.push(event);
    }

    pub fn events(&self) -> &[TraceEvent<'html, 'query>] {
        &self.events
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn print_to_console(&self) {
        print!("{}", self.to_jsonl());
    }

    pub fn to_jsonl(&self) -> String {
        let mut output = String::new();
        for (index, event) in self.events.iter().enumerate() {
            event.write_json_line(index, &mut output);
            output.push('\n');
        }
        output
    }

    pub fn write_jsonl<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        std::fs::write(path, self.to_jsonl())
    }
}

impl<'html, 'query> Default for TraceStore<'html, 'query> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TraceEvent<'html, 'query> {
    ParseStarted {
        html_len: usize,
        query_count: usize,
    },
    ParseFinished {
        element_count: usize,
        query_node_count: usize,
        attribute_count: usize,
        raw_text_len: usize,
        text_len: usize,
    },
    OpenTag {
        tag: &'html str,
        depth: u16,
        reader_position: usize,
        self_closing: bool,
    },
    CloseTag {
        tag: &'html str,
        depth: u16,
        reader_position: usize,
    },
    ImpliedClose {
        tag: &'html str,
        depth: u16,
        reason: ImpliedCloseReason,
    },
    TransitionMatched {
        runner_index: usize,
        cursor: CursorTraceKind,
        selector: &'query str,
        element: &'html str,
        depth: u16,
        selection: QuerySectionId,
        state: TransitionId,
    },
    TransitionRejected {
        runner_index: usize,
        cursor: CursorTraceKind,
        selector: &'query str,
        element: &'html str,
        depth: u16,
        selection: QuerySectionId,
        state: TransitionId,
        reason: TransitionRejectReason,
    },
    ScopedCursorCreated {
        runner_index: usize,
        depth: u16,
        scope_depth: u16,
        parent: ElementId,
        selection: QuerySectionId,
        state: TransitionId,
        reason: ScopedCursorReason,
    },
    ScopedCursorPruned {
        runner_index: usize,
        cursor_index: usize,
        scope_depth: u16,
        close_depth: u16,
        selection: QuerySectionId,
        state: TransitionId,
    },
    ElementSaved {
        runner_index: usize,
        selector: &'query str,
        element: &'html str,
        element_id: ElementId,
        parent_id: ElementId,
        save_inner_html: bool,
        save_raw_text: bool,
        save_text: bool,
    },
    ContentFinalized {
        element_id: ElementId,
        tag: &'html str,
        has_inner_html: bool,
        has_raw_text: bool,
        has_text: bool,
    },
    EarlyExit {
        runner_index: usize,
        selector: &'query str,
        section: QuerySectionId,
    },
    CursorSuppressed {
        runner_index: usize,
        parent: ElementId,
        selection: QuerySectionId,
        state: TransitionId,
        candidate_base_depth: u16,
        dominating_base_depth: u16,
        reason: CursorSuppressionReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorTraceKind {
    Root,
    Scoped { index: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopedCursorReason {
    DescendantFork,
    BranchSibling,
    ChildSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImpliedCloseReason {
    OpenTagRule,
    MismatchedEndTag,
    EofDrain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionRejectReason {
    DepthGuardFailed,
    PredicateFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorSuppressionReason {
    DescendantDominated,
    ExactDuplicate,
    /// A completed First winner already owns this `(section, output parent)`.
    FirstScopeClaimed,
}

impl<'html, 'query> TraceEvent<'html, 'query> {
    fn write_json_line(&self, index: usize, output: &mut String) {
        write!(output, "{{\"index\":{index},").unwrap();
        match self {
            Self::ParseStarted {
                html_len,
                query_count,
            } => {
                write!(
                    output,
                    "\"event\":\"ParseStarted\",\"html_len\":{html_len},\"query_count\":{query_count}"
                )
                .unwrap();
            }
            Self::ParseFinished {
                element_count,
                query_node_count,
                attribute_count,
                raw_text_len,
                text_len,
            } => {
                write!(
                    output,
                    "\"event\":\"ParseFinished\",\"element_count\":{element_count},\"query_node_count\":{query_node_count},\"attribute_count\":{attribute_count},\"raw_text_len\":{raw_text_len},\"text_len\":{text_len}"
                )
                .unwrap();
            }
            Self::OpenTag {
                tag,
                depth,
                reader_position,
                self_closing,
            } => {
                write!(
                    output,
                    "\"event\":\"OpenTag\",\"tag\":{},\"depth\":{depth},\"reader_position\":{reader_position},\"self_closing\":{self_closing}",
                    JsonString(tag)
                )
                .unwrap();
            }
            Self::CloseTag {
                tag,
                depth,
                reader_position,
            } => {
                write!(
                    output,
                    "\"event\":\"CloseTag\",\"tag\":{},\"depth\":{depth},\"reader_position\":{reader_position}",
                    JsonString(tag)
                )
                .unwrap();
            }
            Self::ImpliedClose { tag, depth, reason } => {
                write!(
                    output,
                    "\"event\":\"ImpliedClose\",\"tag\":{},\"depth\":{depth},\"reason\":\"{reason:?}\"",
                    JsonString(tag)
                )
                .unwrap();
            }
            Self::TransitionMatched {
                runner_index,
                cursor,
                selector,
                element,
                depth,
                selection,
                state,
            } => {
                write!(
                    output,
                    "\"event\":\"TransitionMatched\",\"runner_index\":{runner_index},\"cursor\":{},\"selector\":{},\"element\":{},\"depth\":{depth},\"selection\":{},\"state\":{}",
                    JsonString(&format!("{cursor:?}")),
                    JsonString(selector),
                    JsonString(element),
                    selection.index(),
                    state.index()
                )
                .unwrap();
            }
            Self::TransitionRejected {
                runner_index,
                cursor,
                selector,
                element,
                depth,
                selection,
                state,
                reason,
            } => {
                write!(
                    output,
                    "\"event\":\"TransitionRejected\",\"runner_index\":{runner_index},\"cursor\":{},\"selector\":{},\"element\":{},\"depth\":{depth},\"selection\":{},\"state\":{},\"reason\":\"{reason:?}\"",
                    JsonString(&format!("{cursor:?}")),
                    JsonString(selector),
                    JsonString(element),
                    selection.index(),
                    state.index()
                )
                .unwrap();
            }
            Self::ScopedCursorCreated {
                runner_index,
                depth,
                scope_depth,
                parent,
                selection,
                state,
                reason,
            } => {
                write!(
                    output,
                    "\"event\":\"ScopedCursorCreated\",\"runner_index\":{runner_index},\"depth\":{depth},\"scope_depth\":{scope_depth},\"parent\":{},\"selection\":{},\"state\":{},\"reason\":\"{reason:?}\"",
                    parent.index(),
                    selection.index(),
                    state.index()
                )
                .unwrap();
            }
            Self::ScopedCursorPruned {
                runner_index,
                cursor_index,
                scope_depth,
                close_depth,
                selection,
                state,
            } => {
                write!(
                    output,
                    "\"event\":\"ScopedCursorPruned\",\"runner_index\":{runner_index},\"cursor_index\":{cursor_index},\"scope_depth\":{scope_depth},\"close_depth\":{close_depth},\"selection\":{},\"state\":{}",
                    selection.index(),
                    state.index()
                )
                .unwrap();
            }
            Self::ElementSaved {
                runner_index,
                selector,
                element,
                element_id,
                parent_id,
                save_inner_html,
                save_raw_text,
                save_text,
            } => {
                write!(
                    output,
                    "\"event\":\"ElementSaved\",\"runner_index\":{runner_index},\"selector\":{},\"element\":{},\"element_id\":{},\"parent_id\":{},\"save_inner_html\":{save_inner_html},\"save_raw_text\":{save_raw_text},\"save_text\":{save_text}",
                    JsonString(selector),
                    JsonString(element),
                    element_id.index(),
                    parent_id.index()
                )
                .unwrap();
            }
            Self::ContentFinalized {
                element_id,
                tag,
                has_inner_html,
                has_raw_text,
                has_text,
            } => {
                write!(
                    output,
                    "\"event\":\"ContentFinalized\",\"element_id\":{},\"tag\":{},\"has_inner_html\":{has_inner_html},\"has_raw_text\":{has_raw_text},\"has_text\":{has_text}",
                    element_id.index(),
                    JsonString(tag)
                )
                .unwrap();
            }
            Self::EarlyExit {
                runner_index,
                selector,
                section,
            } => {
                write!(
                    output,
                    "\"event\":\"EarlyExit\",\"runner_index\":{runner_index},\"selector\":{},\"section\":{}",
                    JsonString(selector),
                    section.index()
                )
                .unwrap();
            }
            Self::CursorSuppressed {
                runner_index,
                parent,
                selection,
                state,
                candidate_base_depth,
                dominating_base_depth,
                reason,
            } => {
                write!(
                    output,
                    "\"event\":\"CursorSuppressed\",\"runner_index\":{runner_index},\"parent\":{},\"selection\":{},\"state\":{},\"candidate_base_depth\":{candidate_base_depth},\"dominating_base_depth\":{dominating_base_depth},\"reason\":\"{reason:?}\"",
                    parent.index(),
                    selection.index(),
                    state.index()
                )
                .unwrap();
            }
        }
        output.push('}');
    }
}

struct JsonString<'a>(&'a str);

impl std::fmt::Display for JsonString<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_char('"')?;
        for character in self.0.chars() {
            match character {
                '"' => formatter.write_str("\\\"")?,
                '\\' => formatter.write_str("\\\\")?,
                '\n' => formatter.write_str("\\n")?,
                '\r' => formatter.write_str("\\r")?,
                '\t' => formatter.write_str("\\t")?,
                '\u{08}' => formatter.write_str("\\b")?,
                '\u{0c}' => formatter.write_str("\\f")?,
                character if character.is_control() => {
                    write!(formatter, "\\u{:04x}", character as u32)?
                }
                character => formatter.write_char(character)?,
            }
        }
        formatter.write_char('"')
    }
}

#[cfg(test)]
mod tests {
    use crate::debug::{TraceEvent, TraceStore};
    use crate::{ElementId, QuerySectionId, TransitionId};

    #[test]
    fn trace_store_formats_jsonl() {
        let mut trace = TraceStore::new();
        trace.push(TraceEvent::OpenTag {
            tag: "main",
            depth: 1,
            reader_position: 0,
            self_closing: false,
        });
        trace.push(TraceEvent::ElementSaved {
            runner_index: 0,
            selector: "main > a",
            element: "a",
            element_id: ElementId::from(1),
            parent_id: ElementId::from(0),
            save_inner_html: true,
            save_raw_text: true,
            save_text: true,
        });

        let jsonl = trace.to_jsonl();

        assert!(jsonl.contains(r#""index":0"#));
        assert!(jsonl.contains(r#""event":"OpenTag""#));
        assert!(jsonl.contains(r#""tag":"main""#));
        assert!(jsonl.contains(r#""event":"ElementSaved""#));
        assert!(jsonl.contains(r#""selector":"main > a""#));
        assert_eq!(jsonl.lines().count(), 2);
    }

    #[test]
    fn trace_store_writes_jsonl_file() {
        let mut trace = TraceStore::new();
        trace.push(TraceEvent::EarlyExit {
            runner_index: 3,
            selector: "a[href]",
            section: QuerySectionId::from(2),
        });
        trace.push(TraceEvent::TransitionMatched {
            runner_index: 0,
            cursor: super::CursorTraceKind::Root,
            selector: "a[href]",
            element: "a",
            depth: 1,
            selection: QuerySectionId::from(0),
            state: TransitionId::from(1),
        });

        let path = std::env::temp_dir().join(format!("scah-trace-{}.jsonl", std::process::id()));
        trace.write_jsonl(&path).unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains(r#""event":"EarlyExit""#));
        assert!(written.contains(r#""section":2"#));
        assert!(written.contains(r#""event":"TransitionMatched""#));

        std::fs::remove_file(path).unwrap();
    }
}
