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

/// Parser-only state for streaming text capture into shared tapes.
#[derive(Debug)]
pub(crate) struct ParserTextState {
    pub requirements: TextRequirements,
    pub source_start: Option<usize>,
    pub pending: PendingSeparator,
    pub suppressed_depth: u16,
    pub preformatted_depth: u16,
    pub strip_initial_preformatted_newline: bool,
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
            strip_initial_preformatted_newline: false,
            decode_scratch: Vec::new(),
        }
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

    #[inline]
    pub fn queue_separator(&mut self, separator: PendingSeparator) {
        if self.suppressed_depth > 0 {
            return;
        }
        if separator > self.pending {
            self.pending = separator;
        }
    }

    /// Emit a pending separator into the normalized tape if one is queued.
    pub fn flush_pending(&mut self, tape: &mut TextTape) {
        let separator = self.pending;
        self.pending = PendingSeparator::None;
        match separator {
            PendingSeparator::None => {}
            PendingSeparator::Space => tape.push_byte(b' '),
            PendingSeparator::Tab => tape.push_byte(b'\t'),
            PendingSeparator::LineBreak => tape.push_byte(b'\n'),
        }
    }

    pub fn write_normalized_fragment(&mut self, tape: &mut TextTape, source: &str) {
        if self.suppressed_depth > 0 || source.is_empty() {
            return;
        }

        self.decode_scratch.clear();
        html_escape::decode_html_entities_to_vec(source, &mut self.decode_scratch);

        if self.preformatted_depth > 0 {
            // Take ownership of the scratch buffer to avoid borrow conflicts.
            let decoded = std::mem::take(&mut self.decode_scratch);
            self.write_preformatted(tape, &decoded);
            self.decode_scratch = decoded;
        } else {
            let decoded = std::mem::take(&mut self.decode_scratch);
            self.write_collapsed(tape, &decoded);
            self.decode_scratch = decoded;
        }
    }

    fn write_collapsed(&mut self, tape: &mut TextTape, decoded: &[u8]) {
        let mut i = 0;
        while i < decoded.len() {
            let byte = decoded[i];
            if is_html_whitespace(byte) || is_nbsp_at(decoded, i) {
                self.queue_separator(PendingSeparator::Space);
                i += if is_nbsp_at(decoded, i) { 2 } else { 1 };
                continue;
            }

            let char_len = utf8_char_len(byte);
            self.flush_pending(tape);
            let end = (i + char_len).min(decoded.len());
            tape.push_bytes(&decoded[i..end]);
            i = end;
        }
    }

    fn write_preformatted(&mut self, tape: &mut TextTape, decoded: &[u8]) {
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
                if self.strip_initial_preformatted_newline {
                    self.strip_initial_preformatted_newline = false;
                } else {
                    self.flush_pending(tape);
                    tape.push_byte(b'\n');
                }
                continue;
            }

            if byte == b'\n' {
                i += 1;
                if self.strip_initial_preformatted_newline {
                    self.strip_initial_preformatted_newline = false;
                } else {
                    self.flush_pending(tape);
                    tape.push_byte(b'\n');
                }
                continue;
            }

            self.strip_initial_preformatted_newline = false;

            if is_nbsp_at(decoded, i) {
                self.flush_pending(tape);
                // Normalized text converts NBSP to an ordinary space even in pre.
                tape.push_byte(b' ');
                i += 2;
                continue;
            }

            let char_len = utf8_char_len(byte);
            self.flush_pending(tape);
            let end = (i + char_len).min(decoded.len());
            tape.push_bytes(&decoded[i..end]);
            i = end;
        }
    }

    pub fn enter_element(&mut self, flags: TextElementFlags) {
        if flags.contains(TextElementFlags::SUPPRESSED) {
            self.suppressed_depth = self.suppressed_depth.saturating_add(1);
        }
        if flags.contains(TextElementFlags::PREFORMATTED) {
            let was_pre = self.preformatted_depth > 0;
            self.preformatted_depth = self.preformatted_depth.saturating_add(1);
            if !was_pre {
                self.strip_initial_preformatted_newline = true;
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
                self.strip_initial_preformatted_newline = false;
            }
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
