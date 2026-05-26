//! SIMD-accelerated structural character indexing (Stage 1)
//!
//! This module implements the first stage of the two-stage pipeline:
//! scanning the entire HTML input with SIMD to find all structural characters
//! and build a flat index of their positions.
//!
//! ## Performance Characteristics
//! - Processes 64 bytes per iteration (2 × 32-byte AVX2 chunks)
//! - Falls back to scalar processing for tail bytes
//! - Produces a dense `Vec<u32>` of positions for cache-friendly Stage 2

use scah_reader::simd::{SimdInput, bitmask_to_indexes};
use super::tape_entry::{TapeEntry, TapeEntryKind, CompactAttrEntry};

/// Structural characters for HTML tokenization
///
/// These characters delimit the structure of HTML documents:
/// - `<` and `>`: Tag boundaries
/// - `/`: Close tags and self-closing markers
/// - `"` and `'`: Attribute value delimiters
/// - `=`: Attribute key-value separator
/// - `!`: Comments and doctype
#[allow(dead_code)]
pub const HTML_STRUCTURAL_CHARS: [u8; 4] = [b'<', b'>', b'"', b'\''];

/// Additional structural characters for more detailed indexing
#[allow(dead_code)]
pub const HTML_EXTENDED_STRUCTURAL: [u8; 7] = [b'<', b'>', b'/', b'"', b'\'', b'=', b'!'];

/// Result of structural indexing
///
/// Contains the positions of all structural characters found in the input,
/// along with metadata about the scan.
#[derive(Debug, Clone)]
pub struct StructuralIndex {
    /// Positions of structural characters (sorted ascending)
    pub positions: Vec<u32>,
    /// Total bytes scanned
    pub input_length: u32,
}

impl StructuralIndex {
    /// Create a new empty structural index
    pub fn new() -> Self {
        Self {
            positions: Vec::new(),
            input_length: 0,
        }
    }

    /// Create a structural index with pre-allocated capacity
    pub fn with_capacity(estimated_entries: usize) -> Self {
        Self {
            positions: Vec::with_capacity(estimated_entries),
            input_length: 0,
        }
    }

    /// Build a structural index from HTML input using SIMD acceleration
    ///
    /// This is the Stage 1 of the two-stage pipeline. It scans the entire
    /// input looking for structural characters and records their positions.
    ///
    /// # Arguments
    /// * `input` - The HTML source bytes
    ///
    /// # Returns
    /// A `StructuralIndex` containing all structural character positions
    pub fn build(input: &[u8]) -> Self {
        let estimated_capacity = input.len() / 16; // Rough estimate: 1 structural char per 16 bytes
        let mut index = Self::with_capacity(estimated_capacity);
        index.scan(input);
        index
    }

    /// Scan the input and populate the index
    fn scan(&mut self, input: &[u8]) {
        self.input_length = input.len() as u32;
        self.positions.clear();

        let len = input.len();
        let mut pos = 0;

        // SIMD path: process 64 bytes at a time (2 × 32-byte AVX2 chunks)
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                // Process 64 bytes at a time
                while pos + 64 <= len {
                    unsafe {
                        let in0 = SimdInput::load(input[pos..].as_ptr());
                        let in1 = SimdInput::load(input[pos + 32..].as_ptr());

                        // Find all structural characters
                        let mask0 =
                            in0.eq(b'<') | in0.eq(b'>') | in0.eq(b'"') | in0.eq(b'\'');
                        let mask1 =
                            in1.eq(b'<') | in1.eq(b'>') | in1.eq(b'"') | in1.eq(b'\'');

                        bitmask_to_indexes(mask0, pos as u32, &mut self.positions);
                        bitmask_to_indexes(mask1, (pos + 32) as u32, &mut self.positions);
                    }
                    pos += 64;
                }

                // Process remaining 32-byte chunk if available
                if pos + 32 <= len {
                    unsafe {
                        let in0 = SimdInput::load(input[pos..].as_ptr());
                        let mask0 =
                            in0.eq(b'<') | in0.eq(b'>') | in0.eq(b'"') | in0.eq(b'\'');
                        bitmask_to_indexes(mask0, pos as u32, &mut self.positions);
                    }
                    pos += 32;
                }
            }
        }

        // Scalar tail: handle remaining bytes
        while pos < len {
            if matches!(input[pos], b'<' | b'>' | b'"' | b'\'') {
                self.positions.push(pos as u32);
            }
            pos += 1;
        }
    }

    /// Get the number of structural characters found
    #[inline]
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// Check if the index is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Get a position by index
    #[inline]
    pub fn get(&self, index: usize) -> Option<u32> {
        self.positions.get(index).copied()
    }

    /// Get the position at a given index (unchecked)
    ///
    /// # Safety
    /// The caller must ensure `index < self.len()`
    #[inline]
    pub unsafe fn get_unchecked(&self, index: usize) -> u32 {
        unsafe { *self.positions.get_unchecked(index) }
    }

    /// Iterate over positions
    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.positions.iter().copied()
    }

    /// Find the next structural position at or after `from`
    pub fn next_position_after(&self, from: u32) -> Option<u32> {
        // Binary search for efficiency
        match self.positions.binary_search(&from) {
            Ok(idx) => self.positions.get(idx).copied(),
            Err(idx) => self.positions.get(idx).copied(),
        }
    }

    /// Find the structural character at a specific position
    pub fn char_at_position(&self, input: &[u8], position: u32) -> Option<u8> {
        if (position as usize) < input.len() {
            Some(input[position as usize])
        } else {
            None
        }
    }
}

impl Default for StructuralIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// Fast check if a byte is a structural character
#[inline]
#[allow(dead_code)]
pub fn is_structural(byte: u8) -> bool {
    matches!(byte, b'<' | b'>' | b'"' | b'\'')
}

/// Fast check if a byte is an extended structural character
#[inline]
#[allow(dead_code)]
pub fn is_extended_structural(byte: u8) -> bool {
    matches!(byte, b'<' | b'>' | b'/' | b'"' | b'\'' | b'=' | b'!')
}

/// State machine states for the fused tape builder
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[allow(dead_code)]
enum FusedState {
    /// Outside any tag (text content)
    Text,
    /// Inside a tag, parsing the tag name
    TagName,
    /// Inside a tag, parsing an attribute name
    AttrName,
    /// Inside a tag, parsing an attribute value
    AttrValue,
    /// Inside a comment
    Comment,
    /// Inside a doctype
    Doctype,
}

/// A fused single-pass tape builder that combines SIMD structural scanning
/// with attribute tokenization.
///
/// Instead of the current 3-stage pipeline:
/// 1. Structural scanning (SIMD)
/// 2. Tape construction (byte-at-a-time)
/// 3. Attribute re-parsing via Reader+tokenizer
///
/// This builder does it all in one pass using bitmask-driven state machine.
pub struct FusedTapeBuilder {
    /// The flat tape of parsed entries
    tape: Vec<TapeEntry>,
    /// Compact attribute entries (pre-tokenized)
    attributes: Vec<CompactAttrEntry>,
    /// Maps tag tape index to attribute range (start, count)
    tag_attr_map: Vec<(usize, usize)>,
    /// Starting attribute index for the current tag being parsed
    current_tag_attr_start: usize,
}

impl FusedTapeBuilder {
    /// Create a new fused tape builder
    pub fn new() -> Self {
        Self {
            tape: Vec::new(),
            attributes: Vec::new(),
            tag_attr_map: Vec::new(),
            current_tag_attr_start: 0,
        }
    }

    /// Create a new fused tape builder with capacity hints
    pub fn with_capacity(tape_capacity: usize, attr_capacity: usize) -> Self {
        Self {
            tape: Vec::with_capacity(tape_capacity),
            attributes: Vec::with_capacity(attr_capacity),
            tag_attr_map: Vec::with_capacity(tape_capacity / 4),
            current_tag_attr_start: 0,
        }
    }

    /// Build a fused tape from HTML input using SIMD acceleration.
    ///
    /// This performs a single pass through the input, using SIMD to find
    /// structural characters and drive a bitmask-based state machine
    /// that builds the tape with pre-tokenized attributes.
    ///
    /// # Arguments
    /// * `input` - The HTML source bytes
    ///
    /// # Returns
    /// A tuple of (tape entries, compact attribute entries, tag-to-attr mapping)
    pub fn build(input: &[u8]) -> (Vec<TapeEntry>, Vec<CompactAttrEntry>, Vec<(usize, usize)>) {
        let estimated_capacity = input.len() / 16;
        let mut builder = Self::with_capacity(estimated_capacity, estimated_capacity / 4);
        builder.fused_scan(input);
        (builder.tape, builder.attributes, builder.tag_attr_map)
    }

    /// Get a reference to the tape entries
    pub fn tape(&self) -> &[TapeEntry] {
        &self.tape
    }

    /// Get a reference to the compact attribute entries
    pub fn attributes(&self) -> &[CompactAttrEntry] {
        &self.attributes
    }

    /// Perform the fused SIMD scan with bitmask-driven state machine
    fn fused_scan(&mut self, input: &[u8]) {
        let len = input.len();
        let mut pos: usize = 0;
        let mut state = FusedState::Text;

        // Current tag tracking
        let mut tag_start: usize = 0;
        let mut tag_name_start: usize = 0;
        let mut tag_name_end: usize = 0;
        let mut in_self_closing = false;

        // Current attribute tracking
        let mut attr_key_start: usize = 0;
        let mut attr_key_end: usize = 0;
        let mut attr_value_start: usize = 0;
        let mut attr_in_quotes: bool = false;
        let mut attr_quote_char: u8 = 0;

        // Comment/doctype tracking
        let mut _comment_start: usize = 0;

        // Text content tracking
        let mut text_start: usize = 0;

        // SIMD path: process 32 bytes at a time
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                while pos + 32 <= len {
                    unsafe {
                        let chunk = SimdInput::load(input[pos..].as_ptr());
                        let lt_mask = chunk.eq(b'<');
                        let gt_mask = chunk.eq(b'>');
                        let quote_mask = chunk.eq(b'"') | chunk.eq(b'\'');
                        let eq_mask = chunk.eq(b'=');
                        let ws_mask = self.classify_whitespace(&chunk);
                        let slash_mask = chunk.eq(b'/');
                        let excl_mask = chunk.eq(b'!');
                        let _dash_mask = chunk.eq(b'-');

                        // Process each set bit in the combined mask
                        let combined = lt_mask | gt_mask | quote_mask | eq_mask | ws_mask | slash_mask | excl_mask;
                        let mut mask = combined;

                        while mask != 0 {
                            let tz = mask.trailing_zeros() as usize;
                            let abs_pos = pos + tz;
                            let ch = input[abs_pos];
                            let bit = 1u32 << tz;

                            match state {
                                FusedState::Text => {
                                    if lt_mask & bit != 0 {
                                        // Flush text before the tag
                                        if abs_pos > text_start {
                                            self.tape.push(TapeEntry::new(
                                                TapeEntryKind::Text,
                                                text_start as u32,
                                                (abs_pos - text_start) as u32,
                                            ));
                                        }
                                        tag_start = abs_pos;
                                        state = FusedState::TagName;
                                        tag_name_start = 0;
                                        tag_name_end = 0;
                                        in_self_closing = false;
                                    }
                                }
                                FusedState::TagName => {
                                    if excl_mask & bit != 0 && abs_pos == tag_start + 1 {
                                        // Comment or doctype: <!...>
                                        state = FusedState::Comment;
                                        _comment_start = tag_start;
                                    } else if slash_mask & bit != 0 && abs_pos == tag_start + 1 {
                                        // Closing tag: </...>
                                        tag_name_start = abs_pos + 1;
                                    } else if ws_mask & bit != 0 || gt_mask & bit != 0 || slash_mask & bit != 0 {
                                        // End of tag name
                                        if tag_name_start == 0 {
                                            tag_name_start = tag_start + 1;
                                        }
                                        if tag_name_end == 0 {
                                            tag_name_end = abs_pos;
                                        }
                                        state = FusedState::AttrName;
                                        attr_key_start = abs_pos + 1;

                                        if gt_mask & bit != 0 {
                                            // Tag ends immediately after name
                                            self.finish_tag(
                                                input,
                                                tag_start,
                                                tag_name_start,
                                                tag_name_end,
                                                false,
                                                abs_pos,
                                            );
                                            text_start = abs_pos + 1;
                                            state = FusedState::Text;
                                        } else if slash_mask & bit != 0 {
                                            in_self_closing = true;
                                        }
                                    }
                                }
                                FusedState::AttrName => {
                                    if gt_mask & bit != 0 {
                                        // End of tag
                                        let _is_close = input.get(tag_start + 1).copied() == Some(b'/');
                                        self.finish_tag(
                                            input,
                                            tag_start,
                                            tag_name_start,
                                            tag_name_end,
                                            in_self_closing,
                                            abs_pos,
                                        );
                                        text_start = abs_pos + 1;
                                        state = FusedState::Text;
                                    } else if eq_mask & bit != 0 {
                                        // Attribute has a value
                                        attr_key_end = abs_pos;
                                        state = FusedState::AttrValue;
                                        attr_value_start = 0;
                                    } else if ws_mask & bit != 0 {
                                        // Boolean attribute or end of attribute name
                                        if attr_key_start < abs_pos {
                                            // Boolean attribute
                                            self.attributes.push(CompactAttrEntry::new_bool(
                                                attr_key_start as u32,
                                                (abs_pos - attr_key_start) as u16,
                                            ));
                                        }
                                        attr_key_start = abs_pos + 1;
                                    } else if slash_mask & bit != 0 {
                                        in_self_closing = true;
                                        // Boolean attribute before slash
                                        if attr_key_start < abs_pos {
                                            self.attributes.push(CompactAttrEntry::new_bool(
                                                attr_key_start as u32,
                                                (abs_pos - attr_key_start) as u16,
                                            ));
                                        }
                                    } else if quote_mask & bit != 0 {
                                        // Quote in attribute name - unusual but handle
                                        attr_key_end = abs_pos;
                                    }
                                }
                                FusedState::AttrValue => {
                                    if attr_value_start == 0 {
                                        // Looking for value start
                                        if quote_mask & bit != 0 {
                                            attr_value_start = abs_pos + 1;
                                            attr_quote_char = ch;
                                            attr_in_quotes = true;
                                        } else if !ws_mask & bit == 0 {
                                            // Skip whitespace before value
                                        } else {
                                            // Unquoted value
                                            attr_value_start = abs_pos;
                                            attr_in_quotes = false;
                                        }
                                    } else if attr_in_quotes {
                                        // Inside quoted value - look for matching quote
                                        if quote_mask & bit != 0 && ch == attr_quote_char {
                                            // End of quoted value
                                            let attr = if attr_quote_char == b'"' {
                                                CompactAttrEntry::new_double_quoted(
                                                    attr_key_start as u32,
                                                    (attr_key_end - attr_key_start) as u16,
                                                    attr_value_start as u32,
                                                    (abs_pos - attr_value_start) as u16,
                                                )
                                            } else {
                                                CompactAttrEntry::new_single_quoted(
                                                    attr_key_start as u32,
                                                    (attr_key_end - attr_key_start) as u16,
                                                    attr_value_start as u32,
                                                    (abs_pos - attr_value_start) as u16,
                                                )
                                            };
                                            self.attributes.push(attr);
                                            state = FusedState::AttrName;
                                            attr_key_start = abs_pos + 1;
                                            attr_in_quotes = false;
                                        }
                                    } else {
                                        // Unquoted value - end at whitespace or '>'
                                        if ws_mask & bit != 0 || gt_mask & bit != 0 {
                                            let attr = CompactAttrEntry::new_unquoted(
                                                attr_key_start as u32,
                                                (attr_key_end - attr_key_start) as u16,
                                                attr_value_start as u32,
                                                (abs_pos - attr_value_start) as u16,
                                            );
                                            self.attributes.push(attr);
                                            state = FusedState::AttrName;
                                            attr_key_start = abs_pos + 1;

                                            if gt_mask & bit != 0 {
                                                // End of tag
                                                self.finish_tag(
                                                    input,
                                                    tag_start,
                                                    tag_name_start,
                                                    tag_name_end,
                                                    in_self_closing,
                                                    abs_pos,
                                                );
                                                text_start = abs_pos + 1;
                                                state = FusedState::Text;
                                            }
                                        }
                                    }
                                }
                                FusedState::Comment => {
                                    // Look for --> to end comment
                                    if gt_mask & bit != 0 {
                                        // Check if this is the end of comment -->
                                        if abs_pos >= 2 && input[abs_pos - 1] == b'-' && input[abs_pos - 2] == b'-' {
                                            self.tape.push(TapeEntry::new(
                                                TapeEntryKind::Comment,
                                                tag_start as u32,
                                                (abs_pos - tag_start + 1) as u32,
                                            ));
                                            text_start = abs_pos + 1;
                                            state = FusedState::Text;
                                        } else if tag_start + 2 < abs_pos {
                                            // Doctype or other <!...>
                                            self.tape.push(TapeEntry::new(
                                                TapeEntryKind::Doctype,
                                                tag_start as u32,
                                                (abs_pos - tag_start + 1) as u32,
                                            ));
                                            text_start = abs_pos + 1;
                                            state = FusedState::Text;
                                        }
                                    }
                                }
                                FusedState::Doctype => {
                                    // Doctype is handled in Comment state
                                }
                            }

                            // Clear the processed bit
                            mask &= mask.wrapping_sub(1);
                        }
                    }
                    pos += 32;
                }
            }
        }

        // Scalar tail: handle remaining bytes
        while pos < len {
            let ch = input[pos];
            match state {
                FusedState::Text => {
                    if ch == b'<' {
                        // Flush text
                        if pos > text_start {
                            self.tape.push(TapeEntry::new(
                                TapeEntryKind::Text,
                                text_start as u32,
                                (pos - text_start) as u32,
                            ));
                        }
                        tag_start = pos;
                        state = FusedState::TagName;
                        tag_name_start = 0;
                        tag_name_end = 0;
                        in_self_closing = false;
                    }
                }
                FusedState::TagName => {
                    if ch == b'!' && pos == tag_start + 1 {
                        state = FusedState::Comment;
                        _comment_start = tag_start;
                    } else if ch == b'/' && pos == tag_start + 1 {
                        tag_name_start = pos + 1;
                    } else if ch == b' ' || ch == b'\t' || ch == b'\n' || ch == b'\r' || ch == b'>' || ch == b'/' {
                        if tag_name_start == 0 {
                            tag_name_start = tag_start + 1;
                        }
                        if tag_name_end == 0 {
                            tag_name_end = pos;
                        }
                        state = FusedState::AttrName;
                        attr_key_start = pos + 1;

                        if ch == b'>' {
                            self.finish_tag(
                                input,
                                tag_start,
                                tag_name_start,
                                tag_name_end,
                                false,
                                pos,
                            );
                            text_start = pos + 1;
                            state = FusedState::Text;
                        } else if ch == b'/' {
                            in_self_closing = true;
                        }
                    }
                }
                FusedState::AttrName => {
                    if ch == b'>' {
                        self.finish_tag(
                            input,
                            tag_start,
                            tag_name_start,
                            tag_name_end,
                            in_self_closing,
                            pos,
                        );
                        text_start = pos + 1;
                        state = FusedState::Text;
                    } else if ch == b'=' {
                        attr_key_end = pos;
                        state = FusedState::AttrValue;
                        attr_value_start = 0;
                    } else if ch == b' ' || ch == b'\t' || ch == b'\n' || ch == b'\r' {
                        if attr_key_start < pos {
                            self.attributes.push(CompactAttrEntry::new_bool(
                                attr_key_start as u32,
                                (pos - attr_key_start) as u16,
                            ));
                        }
                        attr_key_start = pos + 1;
                    } else if ch == b'/' {
                        in_self_closing = true;
                        if attr_key_start < pos {
                            self.attributes.push(CompactAttrEntry::new_bool(
                                attr_key_start as u32,
                                (pos - attr_key_start) as u16,
                            ));
                        }
                    }
                }
                FusedState::AttrValue => {
                    if attr_value_start == 0 {
                        if ch == b'"' || ch == b'\'' {
                            attr_value_start = pos + 1;
                            attr_quote_char = ch;
                            attr_in_quotes = true;
                        } else if ch != b' ' && ch != b'\t' && ch != b'\n' && ch != b'\r' {
                            attr_value_start = pos;
                            attr_in_quotes = false;
                        }
                    } else if attr_in_quotes {
                        if ch == attr_quote_char {
                            let attr = if attr_quote_char == b'"' {
                                CompactAttrEntry::new_double_quoted(
                                    attr_key_start as u32,
                                    (attr_key_end - attr_key_start) as u16,
                                    attr_value_start as u32,
                                    (pos - attr_value_start) as u16,
                                )
                            } else {
                                CompactAttrEntry::new_single_quoted(
                                    attr_key_start as u32,
                                    (attr_key_end - attr_key_start) as u16,
                                    attr_value_start as u32,
                                    (pos - attr_value_start) as u16,
                                )
                            };
                            self.attributes.push(attr);
                            state = FusedState::AttrName;
                            attr_key_start = pos + 1;
                            attr_in_quotes = false;
                        }
                    } else {
                        if ch == b' ' || ch == b'\t' || ch == b'\n' || ch == b'\r' || ch == b'>' || ch == b'/' {
                            let attr = CompactAttrEntry::new_unquoted(
                                attr_key_start as u32,
                                (attr_key_end - attr_key_start) as u16,
                                attr_value_start as u32,
                                (pos - attr_value_start) as u16,
                            );
                            self.attributes.push(attr);
                            state = FusedState::AttrName;
                            attr_key_start = pos + 1;

                            if ch == b'>' {
                                self.finish_tag(
                                    input,
                                    tag_start,
                                    tag_name_start,
                                    tag_name_end,
                                    in_self_closing,
                                    pos,
                                );
                                text_start = pos + 1;
                                state = FusedState::Text;
                            } else if ch == b'/' {
                                in_self_closing = true;
                            }
                        }
                    }
                }
                FusedState::Comment | FusedState::Doctype => {
                    if ch == b'>' {
                        if pos >= 2 && input[pos - 1] == b'-' && input[pos - 2] == b'-' {
                            self.tape.push(TapeEntry::new(
                                TapeEntryKind::Comment,
                                tag_start as u32,
                                (pos - tag_start + 1) as u32,
                            ));
                        } else if tag_start + 2 < pos {
                            self.tape.push(TapeEntry::new(
                                TapeEntryKind::Doctype,
                                tag_start as u32,
                                (pos - tag_start + 1) as u32,
                            ));
                        }
                        text_start = pos + 1;
                        state = FusedState::Text;
                    }
                }
            }
            pos += 1;
        }

        // Flush remaining text
        if state == FusedState::Text && text_start < len {
            self.tape.push(TapeEntry::new(
                TapeEntryKind::Text,
                text_start as u32,
                (len - text_start) as u32,
            ));
        }
    }

    /// Finish a tag and add it to the tape with its attributes
    fn finish_tag(
        &mut self,
        input: &[u8],
        tag_start: usize,
        _tag_name_start: usize,
        _tag_name_end: usize,
        is_self_closing: bool,
        gt_pos: usize,
    ) {
        // Check for implied self-closing by looking at the last attribute
        let implied_self_close = if !is_self_closing {
            if let Some(last_attr) = self.attributes.last() {
                // Copy fields to avoid issues with packed struct references
                let key_offset = last_attr.key_offset as usize;
                let key_length = last_attr.key_length as usize;
                let key = unsafe {
                    std::str::from_utf8_unchecked(&input[key_offset..key_offset + key_length])
                };
                key == "/" || key.ends_with('/')
            } else {
                false
            }
        } else {
            false
        };

        let kind = if is_self_closing || implied_self_close {
            TapeEntryKind::SelfClosingTag
        } else if input.get(tag_start + 1).copied() == Some(b'/') {
            TapeEntryKind::CloseTag
        } else {
            TapeEntryKind::OpenTag
        };

        let _tape_idx = self.tape.len();
        self.tape.push(TapeEntry::new(
            kind,
            tag_start as u32,
            (gt_pos - tag_start + 1) as u32,
        ));

        // Record attribute mapping for this tag
        let attr_count = self.attributes.len() - self.current_tag_attr_start;
        self.tag_attr_map.push((self.current_tag_attr_start, attr_count));
        self.current_tag_attr_start = self.attributes.len();
    }

    /// Classify whitespace using SIMD (AVX2)
    ///
    /// Uses the `eq` method from SimdInput to avoid accessing private fields.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn classify_whitespace(&self, input: &SimdInput) -> u32 {
        unsafe {
            // Use the public eq() method to check for whitespace characters
            // Space (0x20), Tab (0x09), LF (0x0A), CR (0x0D)
            input.eq(b' ') | input.eq(b'\t') | input.eq(b'\n') | input.eq(b'\r')
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_input() {
        let index = StructuralIndex::build(b"");
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
        assert_eq!(index.input_length, 0);
    }

    #[test]
    fn test_no_structural_chars() {
        let input = b"hello world this is plain text";
        let index = StructuralIndex::build(input);
        assert!(index.is_empty());
    }

    #[test]
    fn test_simple_tag() {
        let input = b"<div>content</div>";
        let index = StructuralIndex::build(input);

        // <div>content</div>
        // 0123456789012345678
        // < at 0, > at 4, < at 12, > at 17
        assert_eq!(index.len(), 4);
        assert_eq!(index.get(0), Some(0));  // <
        assert_eq!(index.get(1), Some(4));  // >
        assert_eq!(index.get(2), Some(12)); // <
        assert_eq!(index.get(3), Some(17)); // >
    }

    #[test]
    fn test_attributes_with_quotes() {
        let input = r#"<div class="test" id='main'>content</div>"#;
        let index = StructuralIndex::build(input.as_bytes());

        // Should find: <, ", ", ', ', >, <, >
        assert!(index.len() >= 7);
        assert_eq!(index.get(0), Some(0)); // <
        assert_eq!(index.get(1), Some(11)); // "
        assert_eq!(index.get(2), Some(16)); // "
        assert_eq!(index.get(3), Some(21)); // '
        assert_eq!(index.get(4), Some(26)); // '
        assert_eq!(index.get(5), Some(27)); // >
    }

    #[test]
    fn test_self_closing_tag() {
        let input = b"<br/><img src='test'/>";
        let index = StructuralIndex::build(input);

        // Should find: <, >, <, ', ', >
        assert!(index.len() >= 6);
    }

    #[test]
    fn test_nested_tags() {
        let input = b"<div><span><a href='link'>text</a></span></div>";
        let index = StructuralIndex::build(input);

        // Multiple <, >, ', ' characters
        assert!(index.len() >= 10);
    }

    #[test]
    fn test_next_position_after() {
        let input = b"<div>content</div>";
        let index = StructuralIndex::build(input);

        // <div>content</div>
        // 0123456789012345678
        assert_eq!(index.next_position_after(0), Some(0));
        assert_eq!(index.next_position_after(1), Some(4));
        assert_eq!(index.next_position_after(5), Some(12));
        assert_eq!(index.next_position_after(13), Some(17));
        assert_eq!(index.next_position_after(18), None);
    }

    #[test]
    fn test_char_at_position() {
        let input = b"<div>content</div>";
        let index = StructuralIndex::build(input);

        assert_eq!(index.char_at_position(input, 0), Some(b'<'));
        assert_eq!(index.char_at_position(input, 4), Some(b'>'));
        assert_eq!(index.char_at_position(input, 5), Some(b'c')); // Not structural
        assert_eq!(index.char_at_position(input, 100), None);
    }

    #[test]
    fn test_positions_are_sorted() {
        let input = b"<div class='test' id=\"main\">content</div>";
        let index = StructuralIndex::build(input);

        for i in 1..index.len() {
            assert!(
                index.get(i - 1).unwrap() <= index.get(i).unwrap(),
                "Positions should be sorted: {:?}",
                &index.positions[..=i]
            );
        }
    }

    #[test]
    fn test_is_structural() {
        assert!(is_structural(b'<'));
        assert!(is_structural(b'>'));
        assert!(is_structural(b'"'));
        assert!(is_structural(b'\''));
        assert!(!is_structural(b'a'));
        assert!(!is_structural(b' '));
        assert!(!is_structural(b'/'));
        assert!(!is_structural(b'='));
    }

    #[test]
    fn test_is_extended_structural() {
        assert!(is_extended_structural(b'<'));
        assert!(is_extended_structural(b'>'));
        assert!(is_extended_structural(b'/'));
        assert!(is_extended_structural(b'='));
        assert!(is_extended_structural(b'!'));
        assert!(!is_extended_structural(b'a'));
        assert!(!is_extended_structural(b' '));
    }

    #[test]
    fn test_large_input() {
        // Test with a larger input to ensure SIMD paths are exercised
        let mut input = Vec::new();
        for i in 0..1000 {
            input.extend_from_slice(format!("<div class='item{}'>content{}</div>", i, i).as_bytes());
        }

        let index = StructuralIndex::build(&input);
        assert!(!index.is_empty());

        // Verify all positions are valid
        for &pos in &index.positions {
            assert!((pos as usize) < input.len());
            assert!(is_structural(input[pos as usize]));
        }
    }

    #[test]
    fn test_simd_scalar_equivalence() {
        // Test that SIMD and scalar paths produce the same results
        let input = b"<div class='test' id=\"main\">Hello <b>World</b> &amp; <i>More</i></div>";

        let index = StructuralIndex::build(input);

        // Manually count structural characters
        let expected_count = input
            .iter()
            .filter(|&&b| matches!(b, b'<' | b'>' | b'"' | b'\''))
            .count();

        assert_eq!(index.len(), expected_count);
    }
}
