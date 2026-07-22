use super::entities::{contains_ampersand, decode_character_references};
use super::text_edge::TextEdgePolicy;
use crate::engine::DepthSize;
use crate::store::TextTape;
use scah_query_ir::TextRequirements;

/// Which text representations the current parse is capturing.
///
/// Constructed once from [`TextRequirements`] so hot parser paths can branch
/// on a compact mode instead of repeatedly inspecting requirement bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextCaptureMode {
    None,
    RawOnly,
    TextOnly,
    Both,
}

impl TextCaptureMode {
    #[inline]
    pub const fn from_requirements(requirements: TextRequirements) -> Self {
        match (requirements.raw_text, requirements.text) {
            (false, false) => Self::None,
            (true, false) => Self::RawOnly,
            (false, true) => Self::TextOnly,
            (true, true) => Self::Both,
        }
    }

    #[inline]
    pub const fn captures_any(self) -> bool {
        !matches!(self, Self::None)
    }

    #[inline]
    #[allow(dead_code)] // part of the capture-mode API surface
    pub const fn captures_raw(self) -> bool {
        matches!(self, Self::RawOnly | Self::Both)
    }

    #[inline]
    pub const fn captures_text(self) -> bool {
        matches!(self, Self::TextOnly | Self::Both)
    }
}

/// Lazy structural/whitespace separator queued for normalized text.
///
/// Separators stay pending until the next visible text (or an opening boundary
/// that must emit before a child range starts). Canonicalization happens only
/// in this pending state — never by mutating bytes already on the tape.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PendingSeparator {
    #[default]
    None = 0,
    Space = 1,
    Tab = 2,
    LineBreak = 3,
}

/// Compact per-element normalized text behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextElementBehavior {
    pub suppressed: bool,
    pub preformatted: bool,
    pub opening_separator: PendingSeparator,
}

impl TextElementBehavior {
    #[inline]
    pub fn flags(self) -> TextElementFlags {
        let mut flags = TextElementFlags::empty();
        if self.suppressed {
            flags.insert(TextElementFlags::SUPPRESSED);
        }
        if self.preformatted {
            flags.insert(TextElementFlags::PREFORMATTED);
        }
        flags
    }

    /// Merge runtime behavior with tag classification bits for the open stack.
    #[inline]
    pub fn stack_flags(self, text_tag: super::tag::TextTagFlags) -> TextElementFlags {
        let mut flags = TextElementFlags::from_text_tag(text_tag);
        // Attribute-driven hidden suppression overrides / extends tag flags.
        if self.suppressed {
            flags.insert(TextElementFlags::SUPPRESSED);
        }
        if self.preformatted {
            flags.insert(TextElementFlags::PREFORMATTED);
        }
        flags
    }
}

/// Parser-only state for streaming text capture into shared tapes.
#[derive(Debug)]
pub(crate) struct ParserTextState {
    pub mode: TextCaptureMode,
    pub source_start: Option<usize>,
    pending: PendingSeparator,
    suppressed_depth: u16,
    preformatted_depth: u16,
    /// Depth at which an immediate initial newline may still be stripped.
    initial_newline_depth: Option<DepthSize>,
    decode_scratch: Vec<u8>,
    #[cfg(feature = "bench-internals")]
    pub path_stats: TextPathStats,
}

/// Optional counters for verifying text-path isolation under `bench-internals`.
#[cfg(feature = "bench-internals")]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TextPathStats {
    pub flush_calls: usize,
    pub mark_start_calls: usize,
    pub normalized_behavior_computations: usize,
    pub hidden_attribute_scans: usize,
    pub decoded_fragments: usize,
    pub tag_classifications: usize,
    pub text_tag_classifications: usize,
    pub text_flag_writes: usize,
}

impl ParserTextState {
    pub fn new(requirements: TextRequirements) -> Self {
        Self {
            mode: TextCaptureMode::from_requirements(requirements),
            source_start: None,
            pending: PendingSeparator::None,
            suppressed_depth: 0,
            preformatted_depth: 0,
            initial_newline_depth: None,
            decode_scratch: Vec::new(),
            #[cfg(feature = "bench-internals")]
            path_stats: TextPathStats::default(),
        }
    }

    #[inline]
    #[allow(dead_code)] // used by raw-only specialization paths / future call sites
    pub fn captures_raw(&self) -> bool {
        self.mode.captures_raw()
    }

    #[inline]
    pub fn captures_text(&self) -> bool {
        self.mode.captures_text()
    }

    #[inline]
    #[allow(dead_code)] // mirrored by parser.capture_mode for hot-path checks
    pub fn captures_any(&self) -> bool {
        self.mode.captures_any()
    }

    #[inline]
    pub fn is_preformatted(&self) -> bool {
        self.preformatted_depth > 0
    }

    /// Edge policy for a newly opened child, using inherited preformatted
    /// context plus the child's own behavior.
    ///
    /// Must be evaluated before [`Self::enter_element`] so inheritance sees the
    /// parent depth. Suppressed elements always trim (their text is empty).
    ///
    /// Visible table cells use [`TextEdgePolicy::Preserve`] so literal trailing
    /// whitespace from preformatted descendants (e.g. `textarea`) remains in
    /// the cell's own range. Ordinary collapsed cell content still lands
    /// correctly: leading pending separators are flushed before `text_start`,
    /// and trailing collapsed whitespace stays pending until after finalize.
    #[inline]
    pub fn edge_policy_for_child(
        &self,
        behavior: TextElementBehavior,
        is_table_cell: bool,
    ) -> TextEdgePolicy {
        if behavior.suppressed {
            TextEdgePolicy::TrimCollapsedSeparators
        } else if self.is_preformatted() || behavior.preformatted || is_table_cell {
            TextEdgePolicy::Preserve
        } else {
            TextEdgePolicy::TrimCollapsedSeparators
        }
    }

    #[inline]
    pub fn mark_source_start(&mut self, position: usize) {
        #[cfg(feature = "bench-internals")]
        {
            self.path_stats.mark_start_calls += 1;
        }
        self.source_start = Some(position);
    }

    /// Cancel pending initial-newline stripping (comment/declaration/child tag).
    #[inline]
    pub fn cancel_initial_newline(&mut self) {
        self.initial_newline_depth = None;
    }

    #[inline]
    pub fn queue_separator(&mut self, separator: PendingSeparator) {
        if !self.captures_text() || self.suppressed_depth > 0 {
            return;
        }
        if separator > self.pending {
            self.pending = separator;
        }
    }

    /// Queue a table-cell column boundary, replacing any pending separator.
    ///
    /// Unlike [`Self::queue_separator`], this does not use ranking. A pending
    /// end-of-cell block newline (from a closed `div`, `p`, etc.) must become
    /// the cell's structural tab so the next cell remains distinguishable.
    /// Row closure still upgrades that tab to a line break via ordinary
    /// ranking (`Tab + LineBreak => LineBreak`).
    #[inline]
    pub fn queue_cell_boundary(&mut self) {
        if !self.captures_text() || self.suppressed_depth > 0 {
            return;
        }
        self.pending = PendingSeparator::Tab;
    }

    /// Emit a pending separator with canonical physical form.
    ///
    /// Append-only: never removes bytes already written to the tape. Generated
    /// separators are resolved in pending state (`Space + LineBreak => LineBreak`,
    /// etc.) before this runs. Literal preformatted whitespace already on the
    /// tape is left intact.
    pub fn flush_pending(&mut self, tape: &mut TextTape) {
        let pending = std::mem::take(&mut self.pending);
        apply_separator(tape, pending);
    }

    pub fn write_normalized_fragment(
        &mut self,
        tape: &mut TextTape,
        source: &str,
        depth: DepthSize,
    ) {
        if !self.captures_text() || self.suppressed_depth > 0 || source.is_empty() {
            return;
        }

        if self.preformatted_depth > 0 {
            self.write_preformatted_source(tape, source, depth);
        } else {
            self.write_collapsed_source(tape, source);
        }
    }

    fn write_collapsed_source(&mut self, tape: &mut TextTape, source: &str) {
        if !contains_ampersand(source) {
            self.write_collapsed_decoded(tape, source.as_bytes());
            return;
        }

        #[cfg(feature = "bench-internals")]
        {
            self.path_stats.decoded_fragments += 1;
        }
        self.decode_scratch.clear();
        decode_character_references(source, &mut self.decode_scratch);
        // Split decode_scratch away so write helpers can borrow `&mut self`.
        let decoded = std::mem::take(&mut self.decode_scratch);
        self.write_collapsed_decoded(tape, &decoded);
        self.decode_scratch = decoded;
    }

    fn write_collapsed_decoded(&mut self, tape: &mut TextTape, decoded: &[u8]) {
        let mut i = 0;
        while i < decoded.len() {
            let byte = decoded[i];
            if is_html_whitespace(byte) || is_nbsp_at(decoded, i) {
                self.queue_separator(PendingSeparator::Space);
                i += if is_nbsp_at(decoded, i) { 2 } else { 1 };
                continue;
            }

            // Copy maximal non-special UTF-8 run in one extend.
            let run_start = i;
            while i < decoded.len() {
                let b = decoded[i];
                if is_html_whitespace(b) || is_nbsp_at(decoded, i) {
                    break;
                }
                i += utf8_char_len(b);
                if i > decoded.len() {
                    i = decoded.len();
                    break;
                }
            }

            self.flush_pending(tape);
            tape.push_bytes(&decoded[run_start..i]);
        }
    }

    fn write_preformatted_source(&mut self, tape: &mut TextTape, source: &str, depth: DepthSize) {
        if !contains_ampersand(source) {
            self.write_preformatted_decoded(tape, source.as_bytes(), depth);
            return;
        }

        #[cfg(feature = "bench-internals")]
        {
            self.path_stats.decoded_fragments += 1;
        }
        self.decode_scratch.clear();
        decode_character_references(source, &mut self.decode_scratch);
        let decoded = std::mem::take(&mut self.decode_scratch);
        self.write_preformatted_decoded(tape, &decoded, depth);
        self.decode_scratch = decoded;
    }

    fn write_preformatted_decoded(
        &mut self,
        tape: &mut TextTape,
        decoded: &[u8],
        depth: DepthSize,
    ) {
        let mut i = 0;
        while i < decoded.len() {
            let byte = decoded[i];

            if byte == b'\r' {
                let next = decoded.get(i + 1).copied();
                if next == Some(b'\n') {
                    i += 2;
                } else {
                    i += 1;
                }
                if self.take_initial_newline(depth) {
                    continue;
                }
                self.flush_pending(tape);
                tape.push_byte(b'\n');
                continue;
            }

            if byte == b'\n' {
                i += 1;
                if self.take_initial_newline(depth) {
                    continue;
                }
                self.flush_pending(tape);
                tape.push_byte(b'\n');
                continue;
            }

            // Any non-newline cancels initial-newline eligibility.
            self.initial_newline_depth = None;

            if is_nbsp_at(decoded, i) {
                self.flush_pending(tape);
                // Normalized text converts NBSP to an ordinary space even in pre.
                tape.push_byte(b' ');
                i += 2;
                continue;
            }

            let run_start = i;
            while i < decoded.len() {
                let b = decoded[i];
                if b == b'\r' || b == b'\n' || is_nbsp_at(decoded, i) {
                    break;
                }
                i += utf8_char_len(b);
                if i > decoded.len() {
                    i = decoded.len();
                    break;
                }
            }

            self.flush_pending(tape);
            tape.push_bytes(&decoded[run_start..i]);
        }
    }

    #[inline]
    fn take_initial_newline(&mut self, depth: DepthSize) -> bool {
        if self.initial_newline_depth == Some(depth) {
            self.initial_newline_depth = None;
            true
        } else {
            false
        }
    }

    /// Apply opening-boundary behavior for a visible (or suppressed) element.
    ///
    /// Elements that begin suppression (`behavior.suppressed`) and descendants
    /// already inside a suppressed subtree (`suppressed_depth > 0`) must not
    /// queue or flush opening separators. Direct suppression is checked here
    /// because depth is incremented only in [`Self::after_open_element`].
    ///
    /// Opening separators are flushed before the child's text range starts so
    /// parent-owned structural newlines sit outside the child range. Once
    /// flushed, those bytes are immutable.
    ///
    /// Callers that capture a normalized range start must also call
    /// [`Self::before_text_range_start`] afterward so any remaining
    /// parent/sibling-owned pending separator is emitted outside that range.
    pub fn before_open_element(
        &mut self,
        tape: &mut TextTape,
        behavior: TextElementBehavior,
        is_void: bool,
    ) {
        if !self.captures_text() || behavior.suppressed || self.suppressed_depth > 0 {
            return;
        }
        if !is_void && behavior.opening_separator != PendingSeparator::None {
            self.queue_separator(behavior.opening_separator);
            self.flush_pending(tape);
        }
    }

    /// Flush pending separators that must not belong to the newly opened
    /// element's normalized range.
    ///
    /// Pending bytes caused by preceding siblings or parent context must be
    /// emitted before `text_start` is recorded when:
    /// - the element uses [`TextEdgePolicy::Preserve`] (preformatted / inherited
    ///   preformatted), because edge trimming will not remove a leaked separator
    /// - the element is a visible table cell, so consecutive empty cells each
    ///   contribute their own tab boundary on the shared tape
    ///
    /// Suppressed elements and descendants already inside a suppressed subtree
    /// must not flush: opening a hidden/`script`/`style`/`template` subtree
    /// (or nesting inside one) must not force a parent separator onto the tape.
    pub fn before_text_range_start(
        &mut self,
        tape: &mut TextTape,
        behavior: TextElementBehavior,
        edge_policy: TextEdgePolicy,
        is_table_cell: bool,
    ) {
        if !self.captures_text() || behavior.suppressed || self.suppressed_depth > 0 {
            return;
        }

        if edge_policy == TextEdgePolicy::Preserve || is_table_cell {
            self.flush_pending(tape);
        }
    }

    pub fn after_open_element(&mut self, behavior: TextElementBehavior, depth: DepthSize) {
        if !self.captures_text() {
            return;
        }
        self.enter_element(behavior.flags(), depth);
    }

    pub fn after_close_element(&mut self, behavior_flags: TextElementFlags, depth: DepthSize) {
        if !self.captures_text() {
            return;
        }
        self.exit_element(behavior_flags, depth);
    }

    pub fn enter_element(&mut self, flags: TextElementFlags, depth: DepthSize) {
        if flags.contains(TextElementFlags::SUPPRESSED) {
            self.suppressed_depth = self.suppressed_depth.saturating_add(1);
        }
        if flags.contains(TextElementFlags::PREFORMATTED) {
            // Each pre/textarea owns its own initial-newline eligibility,
            // including when nested inside another preformatted context.
            self.preformatted_depth = self.preformatted_depth.saturating_add(1);
            self.initial_newline_depth = Some(depth);
        }
    }

    pub fn exit_element(&mut self, flags: TextElementFlags, depth: DepthSize) {
        if self.initial_newline_depth == Some(depth) {
            self.initial_newline_depth = None;
        }
        if flags.contains(TextElementFlags::SUPPRESSED) {
            self.suppressed_depth = self.suppressed_depth.saturating_sub(1);
        }
        if flags.contains(TextElementFlags::PREFORMATTED) {
            self.preformatted_depth = self.preformatted_depth.saturating_sub(1);
            if self.preformatted_depth == 0 {
                self.initial_newline_depth = None;
            }
        }
    }
}

/// Apply a structural/whitespace separator with append-only physical form.
///
/// Invariants for *generated* separators (collapsed whitespace / structural
/// boundaries), resolved primarily via [`PendingSeparator`] ranking before
/// flush:
/// - no leading synthetic separator on an empty tape
/// - no `" \n"`, `"\n "`, or `"\n\n"` from adjacent *generated* boundaries in
///   ordinary collapsed mode (`Space`/`Tab` vs `LineBreak` ranking collapses
///   mixed pending separators before either is written)
///
/// Table-cell column boundaries are an exception: closing a cell uses
/// [`ParserTextState::queue_cell_boundary`], which assigns `Tab` and replaces
/// any pending end-of-cell separator (including a generated block `LineBreak`).
/// Row closure still upgrades that tab via ordinary ranking
/// (`Tab + LineBreak => LineBreak`).
///
/// Literal preformatted whitespace already on the tape is never removed or
/// rewritten. Trailing literal spaces/newlines followed by a structural tab
/// (table cell boundary) or newline are valid, e.g. `"A \tB"` / `"A\n\tB"`.
fn apply_separator(tape: &mut TextTape, separator: PendingSeparator) {
    match separator {
        PendingSeparator::None => {}
        PendingSeparator::Space => {
            let Some(last) = tape.last_byte() else {
                return;
            };
            if matches!(last, b' ' | b'\t' | b'\n') {
                return;
            }
            tape.push_byte(b' ');
        }
        PendingSeparator::Tab => {
            // Suppress only on an empty tape (no leading synthetic separator).
            // Do not treat a trailing physical space/newline as proof the tab
            // is redundant: that byte may be literal preformatted content, and
            // the cell boundary must still be represented. Collapsed Space+Tab
            // is resolved in pending state before flush. Consecutive tabs are
            // allowed so empty columns remain `\t\t`.
            if tape.last_byte().is_none() {
                return;
            }
            tape.push_byte(b'\t');
        }
        PendingSeparator::LineBreak => {
            match tape.last_byte() {
                Some(b'\n') | None => {}
                // Do not strip trailing spaces/tabs: they may be literal
                // preformatted content already covered by a finalized range.
                Some(_) => tape.push_byte(b'\n'),
            }
        }
    }
}

/// Compact per-element normalized text behavior recorded on the open stack.
///
/// Includes both runtime behavior (suppressed / preformatted from attributes
/// and tag class) and the classification bits needed at close time for
/// structural separators. No-text parses push [`Self::empty`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextElementFlags(u8);

impl TextElementFlags {
    pub const SUPPRESSED: Self = Self(1 << 0);
    pub const PREFORMATTED: Self = Self(1 << 1);
    pub const BREAK: Self = Self(1 << 2);
    pub const LINE_SEP: Self = Self(1 << 3); // block or row → line break after close
    pub const CELL: Self = Self(1 << 4);

    #[inline]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[inline]
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    #[inline]
    pub(crate) fn from_text_tag(text_tag: super::tag::TextTagFlags) -> Self {
        let mut flags = Self::empty();
        if text_tag.is_break() {
            flags.insert(Self::BREAK);
        }
        if text_tag.is_block() || text_tag.is_row() {
            flags.insert(Self::LINE_SEP);
        }
        if text_tag.is_cell() {
            flags.insert(Self::CELL);
        }
        if text_tag.is_suppressed() {
            flags.insert(Self::SUPPRESSED);
        }
        if text_tag.is_preformatted() {
            flags.insert(Self::PREFORMATTED);
        }
        flags
    }

    #[inline]
    pub(crate) fn post_text_separator(self) -> Option<PendingSeparator> {
        if self.contains(Self::BREAK) || self.contains(Self::LINE_SEP) {
            Some(PendingSeparator::LineBreak)
        } else if self.contains(Self::CELL) {
            Some(PendingSeparator::Tab)
        } else {
            None
        }
    }
}

#[inline]
fn is_html_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)
}

#[inline]
fn is_nbsp_at(bytes: &[u8], index: usize) -> bool {
    bytes.get(index..index + 2) == Some(&[0xC2, 0xA0])
}

#[inline]
fn utf8_char_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_space_skips_after_newline() {
        let mut tape = TextTape::new();
        tape.push_str("A\n");
        apply_separator(&mut tape, PendingSeparator::Space);
        assert_eq!(tape.slice(0..tape.len()), "A\n");
    }

    #[test]
    fn canonical_linebreak_collapses_duplicate() {
        let mut tape = TextTape::new();
        tape.push_str("A\n");
        apply_separator(&mut tape, PendingSeparator::LineBreak);
        assert_eq!(tape.slice(0..tape.len()), "A\n");
    }

    #[test]
    fn linebreak_preserves_literal_trailing_spaces() {
        let mut tape = TextTape::new();
        tape.push_str("A  ");
        apply_separator(&mut tape, PendingSeparator::LineBreak);
        assert_eq!(tape.slice(0..tape.len()), "A  \n");
    }

    #[test]
    fn tab_preserves_trailing_literal_space_and_emits_boundary() {
        let mut tape = TextTape::new();
        tape.push_str("A ");
        apply_separator(&mut tape, PendingSeparator::Tab);
        assert_eq!(tape.slice(0..tape.len()), "A \t");
    }

    #[test]
    fn tab_preserves_trailing_literal_newline_and_emits_boundary() {
        let mut tape = TextTape::new();
        tape.push_str("A\n");
        apply_separator(&mut tape, PendingSeparator::Tab);
        assert_eq!(tape.slice(0..tape.len()), "A\n\t");
    }

    #[test]
    fn tab_suppressed_on_empty_tape() {
        let mut tape = TextTape::new();
        apply_separator(&mut tape, PendingSeparator::Tab);
        assert_eq!(tape.len(), 0);
    }

    #[test]
    fn consecutive_tabs_are_emitted_for_empty_columns() {
        let mut tape = TextTape::new();
        tape.push_str("A");
        apply_separator(&mut tape, PendingSeparator::Tab);
        apply_separator(&mut tape, PendingSeparator::Tab);
        assert_eq!(tape.slice(0..tape.len()), "A\t\t");
    }

    #[test]
    fn pending_space_upgrades_to_linebreak_without_tape_mutation() {
        let mut state = ParserTextState::new(TextRequirements {
            raw_text: false,
            text: true,
        });
        let mut tape = TextTape::new();
        tape.push_str("A");
        state.queue_separator(PendingSeparator::Space);
        state.queue_separator(PendingSeparator::LineBreak);
        state.flush_pending(&mut tape);
        assert_eq!(tape.slice(0..tape.len()), "A\n");
    }

    #[test]
    fn cell_boundary_replaces_pending_linebreak() {
        let mut state = ParserTextState::new(TextRequirements {
            raw_text: false,
            text: true,
        });
        let mut tape = TextTape::new();

        tape.push_str("A");
        state.queue_separator(PendingSeparator::LineBreak);
        state.queue_cell_boundary();
        state.flush_pending(&mut tape);

        assert_eq!(tape.slice(0..tape.len()), "A\t");
    }

    #[test]
    fn row_boundary_upgrades_pending_cell_tab_to_linebreak() {
        let mut state = ParserTextState::new(TextRequirements {
            raw_text: false,
            text: true,
        });
        let mut tape = TextTape::new();

        tape.push_str("A");
        state.queue_cell_boundary();
        state.queue_separator(PendingSeparator::LineBreak);
        state.flush_pending(&mut tape);

        assert_eq!(tape.slice(0..tape.len()), "A\n");
    }

    #[test]
    fn edge_policy_inherits_preformatted_context() {
        let mut state = ParserTextState::new(TextRequirements {
            raw_text: false,
            text: true,
        });
        state.enter_element(TextElementFlags::PREFORMATTED, 1);
        let child = TextElementBehavior {
            suppressed: false,
            preformatted: false,
            opening_separator: PendingSeparator::None,
        };
        assert_eq!(
            state.edge_policy_for_child(child, false),
            TextEdgePolicy::Preserve
        );
    }

    #[test]
    fn edge_policy_table_cell_preserves_literal_edges() {
        let state = ParserTextState::new(TextRequirements {
            raw_text: false,
            text: true,
        });
        let cell = TextElementBehavior {
            suppressed: false,
            preformatted: false,
            opening_separator: PendingSeparator::None,
        };
        assert_eq!(
            state.edge_policy_for_child(cell, true),
            TextEdgePolicy::Preserve
        );
    }

    #[test]
    fn edge_policy_suppressed_inside_pre_still_trims() {
        let mut state = ParserTextState::new(TextRequirements {
            raw_text: false,
            text: true,
        });
        state.enter_element(TextElementFlags::PREFORMATTED, 1);
        let child = TextElementBehavior {
            suppressed: true,
            preformatted: false,
            opening_separator: PendingSeparator::None,
        };
        assert_eq!(
            state.edge_policy_for_child(child, false),
            TextEdgePolicy::TrimCollapsedSeparators
        );
    }

    #[test]
    fn inherited_suppression_blocks_pre_range_flush() {
        let mut state = ParserTextState::new(TextRequirements {
            raw_text: false,
            text: true,
        });
        let mut tape = TextTape::new();

        tape.push_str("A");
        state.queue_separator(PendingSeparator::LineBreak);

        state.enter_element(TextElementFlags::SUPPRESSED, 1);

        let visible_descendant = TextElementBehavior {
            suppressed: false,
            preformatted: false,
            opening_separator: PendingSeparator::None,
        };

        state.before_text_range_start(
            &mut tape,
            visible_descendant,
            TextEdgePolicy::Preserve,
            false,
        );

        assert_eq!(tape.slice(0..tape.len()), "A");

        state.exit_element(TextElementFlags::SUPPRESSED, 1);
        state.flush_pending(&mut tape);

        assert_eq!(tape.slice(0..tape.len()), "A\n");
    }

    #[test]
    fn inherited_suppression_blocks_opening_boundary_flush() {
        let mut state = ParserTextState::new(TextRequirements {
            raw_text: false,
            text: true,
        });
        let mut tape = TextTape::new();

        tape.push_str("A");
        state.queue_separator(PendingSeparator::LineBreak);
        state.enter_element(TextElementFlags::SUPPRESSED, 1);

        let block_descendant = TextElementBehavior {
            suppressed: false,
            preformatted: false,
            opening_separator: PendingSeparator::LineBreak,
        };

        state.before_open_element(&mut tape, block_descendant, false);

        assert_eq!(tape.slice(0..tape.len()), "A");
    }
}
