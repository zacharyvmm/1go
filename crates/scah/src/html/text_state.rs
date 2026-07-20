use super::entities::{contains_ampersand, decode_character_references};
use crate::engine::DepthSize;
use crate::store::TextTape;
use scah_query_ir::TextRequirements;

/// Lazy structural/whitespace separator queued for normalized text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PendingSeparator {
    #[default]
    None = 0,
    Space = 1,
    Tab = 2,
    LineBreak = 3,
}

/// Whether a saved element's normalized range should trim collapsible edges.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextEdgePolicy {
    #[default]
    TrimCollapsedSeparators,
    Preserve,
}

/// Compact per-element normalized text behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextElementBehavior {
    pub suppressed: bool,
    pub preformatted: bool,
    pub opening_separator: PendingSeparator,
    pub closing_separator: PendingSeparator,
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

    #[inline]
    pub fn edge_policy(self) -> TextEdgePolicy {
        if self.preformatted && !self.suppressed {
            TextEdgePolicy::Preserve
        } else {
            TextEdgePolicy::TrimCollapsedSeparators
        }
    }
}

/// Parser-only state for streaming text capture into shared tapes.
#[derive(Debug)]
pub(crate) struct ParserTextState {
    pub requirements: TextRequirements,
    pub source_start: Option<usize>,
    pending: PendingSeparator,
    suppressed_depth: u16,
    preformatted_depth: u16,
    /// Depth at which an immediate initial newline may still be stripped.
    initial_newline_depth: Option<DepthSize>,
    decode_scratch: Vec<u8>,
}

impl ParserTextState {
    pub fn new(requirements: TextRequirements) -> Self {
        Self {
            requirements,
            source_start: None,
            pending: PendingSeparator::None,
            suppressed_depth: 0,
            preformatted_depth: 0,
            initial_newline_depth: None,
            decode_scratch: Vec::new(),
        }
    }

    #[inline]
    pub fn captures_raw(&self) -> bool {
        self.requirements.raw_text
    }

    #[inline]
    pub fn captures_text(&self) -> bool {
        self.requirements.text
    }

    #[inline]
    pub fn captures_any(&self) -> bool {
        self.requirements.any()
    }

    #[inline]
    pub fn mark_source_start(&mut self, position: usize) {
        if self.captures_any() {
            self.source_start = Some(position);
        }
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

    /// Emit a pending separator with canonical physical form.
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
    /// Suppression is decided by the caller before this runs; suppressed
    /// elements must not queue opening separators.
    pub fn before_open_element(
        &mut self,
        tape: &mut TextTape,
        behavior: TextElementBehavior,
        is_void: bool,
    ) {
        if !self.captures_text() || behavior.suppressed {
            return;
        }
        if !is_void && behavior.opening_separator != PendingSeparator::None {
            self.queue_separator(behavior.opening_separator);
            self.flush_pending(tape);
        }
    }

    pub fn after_open_element(&mut self, behavior: TextElementBehavior, depth: DepthSize) {
        if !self.captures_text() {
            return;
        }
        self.enter_element(behavior.flags(), depth);
    }

    pub fn after_close_element(&mut self, behavior_flags: TextElementFlags) {
        if !self.captures_text() {
            return;
        }
        self.exit_element(behavior_flags);
    }

    pub fn enter_element(&mut self, flags: TextElementFlags, depth: DepthSize) {
        if flags.contains(TextElementFlags::SUPPRESSED) {
            self.suppressed_depth = self.suppressed_depth.saturating_add(1);
        }
        if flags.contains(TextElementFlags::PREFORMATTED) {
            let was_pre = self.preformatted_depth > 0;
            self.preformatted_depth = self.preformatted_depth.saturating_add(1);
            if !was_pre {
                self.initial_newline_depth = Some(depth);
            }
        }
    }

    pub fn exit_element(&mut self, flags: TextElementFlags) {
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

/// Apply a structural/whitespace separator with canonical physical form.
///
/// Invariants enforced for generated separators:
/// - no leading synthetic separator on an empty tape
/// - no `" \n"`, `"\n "`, `"\n\t"`, or `"\n\n"` from adjacent boundaries
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
            let Some(last) = tape.last_byte() else {
                return;
            };
            if last == b'\n' || last == b'\t' {
                return;
            }
            if last == b' ' {
                tape.pop_byte();
            }
            tape.push_byte(b'\t');
        }
        PendingSeparator::LineBreak => {
            loop {
                match tape.last_byte() {
                    Some(b' ' | b'\t') => {
                        tape.pop_byte();
                    }
                    Some(b'\n') | None => return,
                    Some(_) => break,
                }
            }
            tape.push_byte(b'\n');
        }
    }
}

/// Instance-specific text behavior for an open element.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextElementFlags(u8);

impl TextElementFlags {
    pub const SUPPRESSED: Self = Self(1 << 0);
    pub const PREFORMATTED: Self = Self(1 << 1);

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
    fn canonical_linebreak_strips_trailing_spaces() {
        let mut tape = TextTape::new();
        tape.push_str("A  ");
        apply_separator(&mut tape, PendingSeparator::LineBreak);
        assert_eq!(tape.slice(0..tape.len()), "A\n");
    }

    #[test]
    fn canonical_tab_replaces_trailing_space() {
        let mut tape = TextTape::new();
        tape.push_str("A ");
        apply_separator(&mut tape, PendingSeparator::Tab);
        assert_eq!(tape.slice(0..tape.len()), "A\t");
    }
}
