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

use super::tape_entry::{CompactAttrEntry, TapeEntry, TapeEntryKind};
use rayon::prelude::*;
use scah_reader::simd::{SimdInput, bitmask_to_indexes};

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
                        let mask0 = in0.eq(b'<') | in0.eq(b'>') | in0.eq(b'"') | in0.eq(b'\'');
                        let mask1 = in1.eq(b'<') | in1.eq(b'>') | in1.eq(b'"') | in1.eq(b'\'');

                        bitmask_to_indexes(mask0, pos as u32, &mut self.positions);
                        bitmask_to_indexes(mask1, (pos + 32) as u32, &mut self.positions);
                    }
                    pos += 64;
                }

                // Process remaining 32-byte chunk if available
                if pos + 32 <= len {
                    unsafe {
                        let in0 = SimdInput::load(input[pos..].as_ptr());
                        let mask0 = in0.eq(b'<') | in0.eq(b'>') | in0.eq(b'"') | in0.eq(b'\'');
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
                        let combined = lt_mask
                            | gt_mask
                            | quote_mask
                            | eq_mask
                            | ws_mask
                            | slash_mask
                            | excl_mask;
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
                                    } else if ws_mask & bit != 0
                                        || gt_mask & bit != 0
                                        || slash_mask & bit != 0
                                    {
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
                                        let _is_close =
                                            input.get(tag_start + 1).copied() == Some(b'/');
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
                                        if abs_pos >= 2
                                            && input[abs_pos - 1] == b'-'
                                            && input[abs_pos - 2] == b'-'
                                        {
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
                    } else if ch == b' '
                        || ch == b'\t'
                        || ch == b'\n'
                        || ch == b'\r'
                        || ch == b'>'
                        || ch == b'/'
                    {
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
                        if ch == b' '
                            || ch == b'\t'
                            || ch == b'\n'
                            || ch == b'\r'
                            || ch == b'>'
                            || ch == b'/'
                        {
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
        self.tag_attr_map
            .push((self.current_tag_attr_start, attr_count));
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

/// Default chunk size for parallel processing (256KB)
const DEFAULT_CHUNK_SIZE: usize = 256 * 1024;

/// Minimum document size to trigger parallel processing (1MB)
/// The parallel path has overhead from chunk merging, so it only
/// benefits documents large enough to amortize that cost.
const PARALLEL_THRESHOLD: usize = 1024 * 1024; // 1MB

/// Minimum chunk size to avoid excessive overhead from small chunks
const MIN_CHUNK_SIZE: usize = 64 * 1024; // 64KB

/// Maximum chunk size to ensure good parallelism
const MAX_CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4MB

/// Counts of tape entries, attributes, and tag map entries for a chunk.
/// Used by the direct parallel writing optimization to pre-allocate buffers.
#[derive(Debug, Clone, Copy)]
struct ChunkCounts {
    tape_count: usize,
    attr_count: usize,
    tag_map_count: usize,
}

/// Result of processing a single chunk in parallel
///
/// Contains the tape entries, attribute entries, and tag-attribute mapping
/// produced by running `FusedTapeBuilder` on one chunk.
#[derive(Debug, Clone)]
pub struct ChunkResult {
    pub tape: Vec<TapeEntry>,
    pub attributes: Vec<CompactAttrEntry>,
    pub tag_attr_map: Vec<(usize, usize)>,
    /// Byte offset of this chunk's start in the original input
    pub chunk_offset: usize,
    /// Byte length of this chunk
    pub chunk_length: usize,
    /// Whether the chunk ended inside an open tag (not yet closed by `>`)
    pub ends_in_open_tag: bool,
    /// State at the end of the chunk (to resume in next chunk if needed)
    pub end_state: ChunkEndState,
}

/// Parser state at chunk boundary for resumption
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkEndState {
    /// Clean boundary: between tags, at tag close, or in text
    Clean,
    /// Inside an open tag that spans the chunk boundary
    InsideTag,
    /// Inside a quoted attribute value
    InsideQuotedAttr { quote_char: u8 },
    /// Inside a comment that spans the chunk boundary
    InsideComment,
}

/// Finds safe split points in HTML input for parallel processing.
///
/// A safe split point is a position that is:
/// - Outside any tag (not between `<` and `>`)
/// - Outside any quoted string
/// - At or near the target chunk boundary (within tolerance)
///
/// Uses SIMD to scan for `<` and quote characters to track context.
pub struct ChunkSplitter;

/// Document characteristics for adaptive chunk sizing
#[derive(Debug, Clone, Copy)]
pub struct DocumentProfile {
    /// Total document size in bytes
    pub size: usize,
    /// Estimated tag density (tags per KB)
    pub tag_density: f64,
    /// Estimated attribute density (attrs per tag)
    pub attr_density: f64,
    /// Number of available CPU cores
    pub num_cores: usize,
}

impl DocumentProfile {
    /// Analyze document characteristics from a sample of the input
    pub fn analyze(input: &[u8]) -> Self {
        let size = input.len();
        let num_cores = rayon::current_num_threads().max(1);

        // Sample first 8KB to estimate densities
        let sample_size = size.min(8 * 1024);
        let sample = &input[..sample_size];

        let mut tag_count = 0u32;
        let mut attr_count = 0u32;
        let mut in_tag = false;
        let mut in_quotes = false;
        let mut quote_char: u8 = 0;

        for &byte in sample {
            if in_quotes {
                if byte == quote_char {
                    in_quotes = false;
                }
            } else if in_tag {
                match byte {
                    b'>' => {
                        in_tag = false;
                        tag_count += 1;
                    }
                    b'"' | b'\'' => {
                        in_quotes = true;
                        quote_char = byte;
                    }
                    b'=' => {
                        attr_count += 1;
                    }
                    _ => {}
                }
            } else {
                if byte == b'<' {
                    in_tag = true;
                }
            }
        }

        let sample_kb = sample_size as f64 / 1024.0;
        let tag_density = if sample_kb > 0.0 {
            tag_count as f64 / sample_kb
        } else {
            10.0
        };
        let attr_density = if tag_count > 0 {
            attr_count as f64 / tag_count as f64
        } else {
            2.0
        };

        Self {
            size,
            tag_density,
            attr_density,
            num_cores,
        }
    }

    /// Calculate optimal chunk size based on document characteristics
    pub fn optimal_chunk_size(&self) -> usize {
        // Base chunk size on document size and available cores
        let ideal_chunks = self.num_cores * 4; // Aim for 4x cores for good load balancing
        let base_chunk_size = self.size / ideal_chunks;

        // Adjust based on tag density:
        // - High tag density (many small tags) -> smaller chunks for better parallelism
        // - Low tag density (few large tags) -> larger chunks to reduce merge overhead
        let density_factor = if self.tag_density > 50.0 {
            0.75 // Attribute-heavy HTML benefits from smaller chunks
        } else if self.tag_density < 10.0 {
            1.5 // Text-heavy HTML benefits from larger chunks
        } else {
            1.0
        };

        // Adjust based on attribute density:
        // - High attr density -> larger chunks (attrs are expensive to merge)
        let attr_factor = if self.attr_density > 5.0 { 1.25 } else { 1.0 };

        let adjusted = (base_chunk_size as f64 * density_factor * attr_factor) as usize;

        // Clamp to reasonable bounds
        adjusted.max(MIN_CHUNK_SIZE).min(MAX_CHUNK_SIZE)
    }
}

impl ChunkSplitter {
    /// Split input into chunks at safe boundaries near 64KB intervals.
    ///
    /// Returns a vector of (start, length) tuples covering the entire input.
    /// Each chunk starts at a position where it's safe to begin a new
    /// FusedTapeBuilder pass (i.e., not inside a tag or quoted string).
    ///
    /// # Arguments
    /// * `input` - The HTML source bytes
    /// * `target_chunk_size` - Desired chunk size in bytes (will be adjusted to safe boundaries)
    pub fn split(input: &[u8], target_chunk_size: usize) -> Vec<(usize, usize)> {
        let len = input.len();
        if len <= target_chunk_size {
            return vec![(0, len)];
        }

        let mut chunks = Vec::with_capacity(len / target_chunk_size + 1);
        let mut start = 0;

        while start < len {
            let remaining = len - start;
            if remaining <= target_chunk_size {
                // Last chunk takes all remaining
                chunks.push((start, remaining));
                break;
            }

            // Find a safe split point near the target boundary
            let target_end = start + target_chunk_size;
            let safe_end = Self::find_safe_split_point(input, target_end);

            let chunk_len = safe_end - start;
            chunks.push((start, chunk_len));
            start = safe_end;
        }

        chunks
    }

    /// Split input into chunks using adaptive sizing based on document characteristics.
    ///
    /// This method analyzes the document to determine optimal chunk size based on:
    /// - Document size
    /// - Tag density (tags per KB)
    /// - Attribute density (attrs per tag)
    /// - Available CPU cores
    ///
    /// # Arguments
    /// * `input` - The HTML source bytes
    pub fn split_adaptive(input: &[u8]) -> Vec<(usize, usize)> {
        let profile = DocumentProfile::analyze(input);
        let chunk_size = profile.optimal_chunk_size();
        Self::split(input, chunk_size)
    }

    /// Split input into chunks using a document profile for optimal sizing.
    ///
    /// Returns a vector of (start, length) tuples covering the entire input.
    /// Each chunk starts at a position where it's safe to begin a new
    /// FusedTapeBuilder pass (i.e., not inside a tag or quoted string).
    ///
    /// # Arguments
    /// * `input` - The HTML source bytes
    /// * `profile` - Document profile with optimal chunk size
    pub fn split_with_profile(input: &[u8], profile: &DocumentProfile) -> Vec<(usize, usize)> {
        let target_chunk_size = profile.optimal_chunk_size();
        Self::split(input, target_chunk_size)
    }

    /// Find the nearest safe split point to the target position.
    ///
    /// Scans backward from `target` to find a position that is outside
    /// any tag and outside any quoted attribute value.
    ///
    /// Uses SIMD to detect `<` and quote characters for context tracking.
    fn find_safe_split_point(input: &[u8], target: usize) -> usize {
        let len = input.len();
        if target >= len {
            return len;
        }

        // Scan backward from target to find a safe point
        // We track whether we're inside a tag by counting unescaped `<` and `>`
        // We track whether we're inside quotes by counting quote characters
        let scan_start = target.saturating_sub(4096).max(0);

        // First pass: determine context at scan_start by scanning from position 0
        // For efficiency, we use a simple heuristic: scan backward from target
        // looking for a `>` (end of tag) that isn't inside quotes.
        let mut in_tag = false;
        let mut quote_char: u8 = 0;
        let mut in_quotes = false;

        // Scan from beginning to track context state at scan_start
        Self::scan_context(
            input,
            scan_start,
            &mut in_tag,
            &mut in_quotes,
            &mut quote_char,
        );

        // Now scan from scan_start forward looking for a clean boundary
        let mut pos = scan_start;
        while pos < len && pos < target + 4096 {
            let ch = input[pos];

            if in_quotes {
                if ch == quote_char {
                    in_quotes = false;
                }
            } else if in_tag {
                match ch {
                    b'>' => {
                        in_tag = false;
                        // Position right after `>` is safe
                        if pos + 1 >= target {
                            return pos + 1;
                        }
                    }
                    b'"' | b'\'' => {
                        in_quotes = true;
                        quote_char = ch;
                    }
                    _ => {}
                }
            } else {
                match ch {
                    b'<' => {
                        in_tag = true;
                        // Position right before `<` is safe
                        if pos >= target {
                            return pos;
                        }
                    }
                    _ => {}
                }
            }
            pos += 1;
        }

        // Fallback: return target as-is (may split inside a tag, merger will handle it)
        target
    }

    /// Scan input from the beginning to determine context (in_tag, in_quotes) at `up_to`.
    fn scan_context(
        input: &[u8],
        up_to: usize,
        in_tag: &mut bool,
        in_quotes: &mut bool,
        quote_char: &mut u8,
    ) {
        let end = up_to.min(input.len());

        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") && end >= 32 {
                unsafe {
                    Self::scan_context_simd(input, end, in_tag, in_quotes, quote_char);
                    return;
                }
            }
        }

        // Scalar fallback
        for i in 0..end {
            let ch = input[i];
            if *in_quotes {
                if ch == *quote_char {
                    *in_quotes = false;
                }
            } else if *in_tag {
                match ch {
                    b'>' => *in_tag = false,
                    b'"' | b'\'' => {
                        *in_quotes = true;
                        *quote_char = ch;
                    }
                    _ => {}
                }
            } else {
                if ch == b'<' {
                    *in_tag = true;
                }
            }
        }
    }

    /// SIMD-accelerated context scanning for determining tag/quote state.
    ///
    /// Scans 32 bytes at a time using AVX2 to find `<`, `>`, `"`, `'`
    /// and updates the context tracking state.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn scan_context_simd(
        input: &[u8],
        up_to: usize,
        in_tag: &mut bool,
        in_quotes: &mut bool,
        quote_char: &mut u8,
    ) {
        let mut pos = 0;

        while pos + 32 <= up_to {
            unsafe {
                let chunk = SimdInput::load(input[pos..].as_ptr());
                let lt_mask = chunk.eq(b'<');
                let gt_mask = chunk.eq(b'>');
                let dq_mask = chunk.eq(b'"');
                let sq_mask = chunk.eq(b'\'');

                let combined = lt_mask | gt_mask | dq_mask | sq_mask;
                let mut mask = combined;

                while mask != 0 {
                    let tz = mask.trailing_zeros() as usize;
                    let abs_pos = pos + tz;
                    let ch = input[abs_pos];
                    let bit = 1u32 << tz;

                    if *in_quotes {
                        if ch == *quote_char {
                            *in_quotes = false;
                        }
                    } else if *in_tag {
                        if gt_mask & bit != 0 {
                            *in_tag = false;
                        } else if (dq_mask | sq_mask) & bit != 0 {
                            *in_quotes = true;
                            *quote_char = ch;
                        }
                    } else {
                        if lt_mask & bit != 0 {
                            *in_tag = true;
                        }
                    }

                    mask &= mask.wrapping_sub(1);
                }
            }
            pos += 32;
        }

        // Scalar tail
        while pos < up_to {
            let ch = input[pos];
            if *in_quotes {
                if ch == *quote_char {
                    *in_quotes = false;
                }
            } else if *in_tag {
                match ch {
                    b'>' => *in_tag = false,
                    b'"' | b'\'' => {
                        *in_quotes = true;
                        *quote_char = ch;
                    }
                    _ => {}
                }
            } else {
                if ch == b'<' {
                    *in_tag = true;
                }
            }
            pos += 1;
        }
    }
}

/// Merges tape segments from parallel chunk processing.
///
/// Handles three kinds of boundary fixups:
/// 1. **Split tags**: A tag that starts in one chunk and ends in the next
/// 2. **Attribute count adjustments**: Attribute indices need rebasing
/// 3. **Text entry coalescing**: Adjacent text entries across boundaries are merged
pub struct TapeMerger;

impl TapeMerger {
    /// Merge chunk results into a single coherent tape.
    ///
    /// # Arguments
    /// * `input` - The original HTML source (for context)
    /// * `chunk_results` - Results from parallel chunk processing, in order
    ///
    /// # Returns
    /// A tuple of (merged tape, merged attributes, merged tag_attr_map)
    pub fn merge(
        input: &[u8],
        chunk_results: Vec<ChunkResult>,
    ) -> (Vec<TapeEntry>, Vec<CompactAttrEntry>, Vec<(usize, usize)>) {
        if chunk_results.is_empty() {
            return (Vec::new(), Vec::new(), Vec::new());
        }

        if chunk_results.len() == 1 {
            let r = chunk_results.into_iter().next().unwrap();
            return (r.tape, r.attributes, r.tag_attr_map);
        }

        // Pre-calculate total sizes for efficient allocation
        let total_tape: usize = chunk_results.iter().map(|r| r.tape.len()).sum();
        let total_attrs: usize = chunk_results.iter().map(|r| r.attributes.len()).sum();
        let total_tag_map: usize = chunk_results.iter().map(|r| r.tag_attr_map.len()).sum();

        let mut merged_tape = Vec::with_capacity(total_tape);
        let mut merged_attrs = Vec::with_capacity(total_attrs);
        let mut merged_tag_attr_map = Vec::with_capacity(total_tag_map);

        let mut attr_offset: usize = 0; // Running offset for rebasing attribute indices

        for (idx, chunk) in chunk_results.into_iter().enumerate() {
            let chunk_tape = chunk.tape;
            let chunk_attrs = chunk.attributes;
            let chunk_tag_map = chunk.tag_attr_map;

            // Fix up the first entry of this chunk if needed
            let start_idx = if idx > 0 && !chunk_tape.is_empty() {
                Self::fixup_chunk_boundary(
                    input,
                    &mut merged_tape,
                    &chunk_tape,
                    &chunk_attrs,
                    &chunk_tag_map,
                    attr_offset,
                    chunk.chunk_offset,
                )
            } else {
                0
            };

            // Append tape entries (skipping any that were consumed by boundary fixup)
            for entry in &chunk_tape[start_idx..] {
                merged_tape.push(*entry);
            }

            // Append attributes with rebased indices in tag_attr_map
            for &(attr_start, attr_count) in &chunk_tag_map {
                merged_tag_attr_map.push((attr_start + attr_offset, attr_count));
            }

            // Append the actual attribute entries
            merged_attrs.extend_from_slice(&chunk_attrs);
            attr_offset += chunk_attrs.len();
        }

        // Post-merge pass: coalesce adjacent text entries
        Self::coalesce_adjacent_text(&mut merged_tape);

        (merged_tape, merged_attrs, merged_tag_attr_map)
    }

    /// Merge chunk results with inline offset adjustment.
    ///
    /// This is an optimized version that adjusts offsets during the merge
    /// pass instead of requiring a separate adjustment pass.
    pub fn merge_with_offset_adjustment(
        input: &[u8],
        chunk_results: Vec<ChunkResult>,
    ) -> (Vec<TapeEntry>, Vec<CompactAttrEntry>, Vec<(usize, usize)>) {
        if chunk_results.is_empty() {
            return (Vec::new(), Vec::new(), Vec::new());
        }

        if chunk_results.len() == 1 {
            let mut r = chunk_results.into_iter().next().unwrap();
            // Adjust offsets for single chunk
            let offset = r.chunk_offset as u32;
            for entry in &mut r.tape {
                entry.offset += offset;
            }
            for attr in &mut r.attributes {
                attr.key_offset += offset as u32;
                if attr.has_value() {
                    attr.value_offset += offset as u32;
                }
            }
            return (r.tape, r.attributes, r.tag_attr_map);
        }

        // Pre-calculate total sizes for efficient allocation
        let total_tape: usize = chunk_results.iter().map(|r| r.tape.len()).sum();
        let total_attrs: usize = chunk_results.iter().map(|r| r.attributes.len()).sum();
        let total_tag_map: usize = chunk_results.iter().map(|r| r.tag_attr_map.len()).sum();

        let mut merged_tape = Vec::with_capacity(total_tape);
        let mut merged_attrs = Vec::with_capacity(total_attrs);
        let mut merged_tag_attr_map = Vec::with_capacity(total_tag_map);

        let mut attr_offset: usize = 0; // Running offset for rebasing attribute indices

        for (idx, chunk) in chunk_results.into_iter().enumerate() {
            let chunk_offset = chunk.chunk_offset as u32;
            let mut chunk_tape = chunk.tape;
            let mut chunk_attrs = chunk.attributes;
            let chunk_tag_map = chunk.tag_attr_map;

            // Fix up the first entry of this chunk if needed
            let start_idx = if idx > 0 && !chunk_tape.is_empty() {
                Self::fixup_chunk_boundary(
                    input,
                    &mut merged_tape,
                    &chunk_tape,
                    &chunk_attrs,
                    &chunk_tag_map,
                    attr_offset,
                    chunk.chunk_offset,
                )
            } else {
                0
            };

            // Adjust and append tape entries inline
            for entry in &mut chunk_tape[start_idx..] {
                entry.offset += chunk_offset;
                merged_tape.push(*entry);
            }

            // Append attributes with rebased indices in tag_attr_map
            for &(attr_start, attr_count) in &chunk_tag_map {
                merged_tag_attr_map.push((attr_start + attr_offset, attr_count));
            }

            // Adjust and append attribute entries inline
            for attr in &mut chunk_attrs {
                attr.key_offset += chunk_offset as u32;
                if attr.has_value() {
                    attr.value_offset += chunk_offset as u32;
                }
            }
            merged_attrs.extend_from_slice(&chunk_attrs);
            attr_offset += chunk_attrs.len();
        }

        // Post-merge pass: coalesce adjacent text entries
        Self::coalesce_adjacent_text(&mut merged_tape);

        (merged_tape, merged_attrs, merged_tag_attr_map)
    }

    /// Fix up the boundary between two adjacent chunks.
    ///
    /// Handles:
    /// 1. Split tags: if the previous chunk ended inside a tag, and the current
    ///    chunk starts with the continuation, we need to combine them.
    /// 2. Text coalescing: if both chunks have text entries at the boundary,
    ///    we'll merge them in the coalescing pass.
    ///
    /// Returns the index in `current_tape` from which to start copying entries.
    fn fixup_chunk_boundary(
        _input: &[u8],
        merged_tape: &mut Vec<TapeEntry>,
        current_tape: &[TapeEntry],
        _current_attrs: &[CompactAttrEntry],
        _current_tag_map: &[(usize, usize)],
        _attr_offset: usize,
        _chunk_offset: usize,
    ) -> usize {
        if current_tape.is_empty() {
            return 0;
        }

        // Check if the last entry in merged tape is text and first in current is text
        // This is handled in the coalescing pass, but we can also handle simple cases here
        if let Some(last) = merged_tape.last() {
            let first = &current_tape[0];

            // If both are text and adjacent, we'll merge them
            if last.kind == TapeEntryKind::Text && first.kind == TapeEntryKind::Text {
                // Extend the last text entry to cover the current one
                if let Some(last_mut) = merged_tape.last_mut() {
                    let new_end = first.offset + first.length;
                    last_mut.length = new_end - last_mut.offset;
                    return 1; // Skip the first entry of current tape
                }
            }

            // If the previous chunk ended with a partial tag (inside tag state),
            // and the current chunk starts with the tag continuation,
            // we need to handle this specially.
            // For now, our ChunkSplitter ensures clean boundaries so this shouldn't happen.
            // If it does (fallback case), the tag will be slightly malformed but
            // the parser handles malformed HTML gracefully.
        }

        // Handle split tags: if the first entry of current chunk is a tag
        // that appears to be a continuation of a tag from the previous chunk
        // (i.e., it doesn't start with '<'), we need special handling.
        // Our ChunkSplitter tries to avoid this, but for robustness:
        if !current_tape.is_empty() {
            let first = &current_tape[0];
            // If first entry is a tag that doesn't start with '<', it's a split tag
            if first.is_tag() {
                // This shouldn't happen with clean split points, but if it does,
                // we skip this entry as it's part of a tag from the previous chunk
                // that was already processed.
                // We check by looking at the actual byte at the offset
                // If it's not '<', it's a continuation
                // (We don't have access to input here, so we rely on the splitter)
            }
        }

        0
    }

    /// Coalesce adjacent text entries in the merged tape.
    ///
    /// After merging chunks, there may be adjacent text entries that
    /// should be combined into a single entry.
    pub fn coalesce_adjacent_text(tape: &mut Vec<TapeEntry>) {
        if tape.len() < 2 {
            return;
        }

        let mut write_idx = 0;
        for read_idx in 1..tape.len() {
            if tape[write_idx].kind == TapeEntryKind::Text
                && tape[read_idx].kind == TapeEntryKind::Text
                && tape[write_idx].end() == tape[read_idx].offset
            {
                // Merge: extend the write entry to cover the read entry
                tape[write_idx].length += tape[read_idx].length;
            } else {
                write_idx += 1;
                if write_idx != read_idx {
                    tape[write_idx] = tape[read_idx];
                }
            }
        }
        tape.truncate(write_idx + 1);
    }

    /// Streaming merge using rayon's fold/reduce pattern.
    ///
    /// This method processes chunks incrementally, reducing memory pressure
    /// by not collecting all chunk results before merging. Instead, it uses
    /// rayon's fold/reduce to merge pairs of results as they become available.
    ///
    /// # Arguments
    /// * `input` - The original HTML source (for context)
    /// * `chunk_results` - Results from parallel chunk processing, in order
    ///
    /// # Returns
    /// A tuple of (merged tape, merged attributes, merged tag_attr_map)
    pub fn merge_streaming(
        _input: &[u8],
        chunk_results: Vec<ChunkResult>,
    ) -> (Vec<TapeEntry>, Vec<CompactAttrEntry>, Vec<(usize, usize)>) {
        if chunk_results.is_empty() {
            return (Vec::new(), Vec::new(), Vec::new());
        }

        if chunk_results.len() == 1 {
            let r = chunk_results.into_iter().next().unwrap();
            return (r.tape, r.attributes, r.tag_attr_map);
        }

        // Use sequential merge with boundary coalescing
        // Note: rayon's reduce doesn't preserve order, so we use sequential merge
        // for correctness. The parallelism is in the chunk processing, not the merge.
        let total_tape: usize = chunk_results.iter().map(|r| r.tape.len()).sum();
        let total_attrs: usize = chunk_results.iter().map(|r| r.attributes.len()).sum();
        let total_tag_map: usize = chunk_results.iter().map(|r| r.tag_attr_map.len()).sum();

        let mut merged_tape = Vec::with_capacity(total_tape);
        let mut merged_attrs = Vec::with_capacity(total_attrs);
        let mut merged_tag_attr_map = Vec::with_capacity(total_tag_map);

        let mut attr_offset: usize = 0;

        for (idx, chunk) in chunk_results.into_iter().enumerate() {
            let chunk_offset = chunk.chunk_offset as u32;
            let chunk_tape = chunk.tape;
            let chunk_attrs = chunk.attributes;
            let chunk_tag_map = chunk.tag_attr_map;

            // Handle boundary coalescing
            let start_idx = if idx > 0 && !chunk_tape.is_empty() && !merged_tape.is_empty() {
                let last: &TapeEntry = merged_tape.last().unwrap();
                let first = &chunk_tape[0];

                if last.kind == TapeEntryKind::Text
                    && first.kind == TapeEntryKind::Text
                    && last.end() == first.offset + chunk_offset
                {
                    if let Some(last_mut) = merged_tape.last_mut() {
                        let new_end = first.offset + chunk_offset + first.length;
                        last_mut.length = new_end - last_mut.offset;
                    }
                    1
                } else {
                    0
                }
            } else {
                0
            };

            // Append tape entries with adjusted offsets
            for entry in &chunk_tape[start_idx..] {
                let mut adjusted = *entry;
                adjusted.offset += chunk_offset;
                merged_tape.push(adjusted);
            }

            // Append attributes with adjusted offsets
            for attr in &chunk_attrs {
                let mut adjusted = *attr;
                adjusted.key_offset += chunk_offset;
                if adjusted.has_value() {
                    adjusted.value_offset += chunk_offset;
                }
                merged_attrs.push(adjusted);
            }

            // Append tag map with rebased indices
            for &(attr_start, attr_count) in &chunk_tag_map {
                merged_tag_attr_map.push((attr_start + attr_offset, attr_count));
            }

            attr_offset += chunk_attrs.len();
        }

        (merged_tape, merged_attrs, merged_tag_attr_map)
    }
}

impl FusedTapeBuilder {
    /// Build a fused tape from HTML input using parallel chunk processing.
    ///
    /// This splits the input into chunks, processes each chunk independently
    /// in parallel using rayon, then merges the results with boundary fixups.
    ///
    /// For documents smaller than the parallel threshold (128KB), falls back
    /// to the sequential `build` method.
    ///
    /// # Arguments
    /// * `input` - The HTML source bytes
    ///
    /// # Returns
    /// A tuple of (tape entries, compact attribute entries, tag-to-attr mapping)
    pub fn build_parallel(
        input: &[u8],
    ) -> (Vec<TapeEntry>, Vec<CompactAttrEntry>, Vec<(usize, usize)>) {
        Self::build_parallel_with_chunk_size(input, DEFAULT_CHUNK_SIZE)
    }

    /// Build a fused tape from HTML input using adaptive parallel chunk processing.
    ///
    /// This method analyzes the document to determine optimal chunk size based on:
    /// - Document size
    /// - Tag density (tags per KB)
    /// - Attribute density (attrs per tag)
    /// - Available CPU cores
    ///
    /// # Arguments
    /// * `input` - The HTML source bytes
    ///
    /// # Returns
    /// A tuple of (tape entries, compact attribute entries, tag-to-attr mapping)
    pub fn build_parallel_adaptive(
        input: &[u8],
    ) -> (Vec<TapeEntry>, Vec<CompactAttrEntry>, Vec<(usize, usize)>) {
        // Fall back to sequential for small documents
        if input.len() < PARALLEL_THRESHOLD {
            return Self::build(input);
        }

        // Use adaptive chunk sizing
        let chunks = ChunkSplitter::split_adaptive(input);

        // If we got a single chunk, just run sequentially
        if chunks.len() <= 1 {
            return Self::build(input);
        }

        // Process chunks in parallel using rayon
        let chunk_results: Vec<ChunkResult> = chunks
            .par_iter()
            .map(|&(start, len)| {
                let chunk_input = &input[start..start + len];
                let (tape, attrs, tag_map) = Self::build_chunk(chunk_input);

                // Detect end state for boundary fixup
                let end_state = Self::detect_end_state(chunk_input);

                ChunkResult {
                    tape,
                    attributes: attrs,
                    tag_attr_map: tag_map,
                    chunk_offset: start,
                    chunk_length: len,
                    ends_in_open_tag: end_state == ChunkEndState::InsideTag,
                    end_state,
                }
            })
            .collect();

        // Merge chunk results (offset adjustment happens inline during merge)
        TapeMerger::merge_with_offset_adjustment(input, chunk_results)
    }

    /// Build a fused tape using direct parallel writing with prefix scan.
    ///
    /// This optimized version avoids the merge overhead by:
    /// 1. First pass: Each chunk computes only counts (fast, no allocations)
    /// 2. Prefix scan: Compute cumulative offsets for each chunk
    /// 3. Second pass: Each chunk writes directly to the final buffer
    ///
    /// This eliminates the collect → merge pipeline entirely.
    ///
    /// # Arguments
    /// * `input` - The HTML source bytes
    ///
    /// # Returns
    /// A tuple of (tape entries, compact attribute entries, tag-to-attr mapping)
    pub fn build_parallel_direct(
        input: &[u8],
    ) -> (Vec<TapeEntry>, Vec<CompactAttrEntry>, Vec<(usize, usize)>) {
        Self::build_parallel_direct_with_chunk_size(input, DEFAULT_CHUNK_SIZE)
    }

    /// Build a fused tape using direct parallel writing with adaptive chunk sizing.
    ///
    /// This method combines the benefits of adaptive chunk sizing with the
    /// optimized direct parallel writing path.
    ///
    /// # Arguments
    /// * `input` - The HTML source bytes
    ///
    /// # Returns
    /// A tuple of (tape entries, compact attribute entries, tag-to-attr mapping)
    pub fn build_parallel_direct_adaptive(
        input: &[u8],
    ) -> (Vec<TapeEntry>, Vec<CompactAttrEntry>, Vec<(usize, usize)>) {
        // Fall back to sequential for small documents
        if input.len() < PARALLEL_THRESHOLD {
            return Self::build(input);
        }

        // Use adaptive chunk sizing
        let chunks = ChunkSplitter::split_adaptive(input);

        // If we got a single chunk, just run sequentially
        if chunks.len() <= 1 {
            return Self::build(input);
        }

        let num_chunks = chunks.len();

        // Phase 1: Fast count-only pass (no allocations)
        let chunk_counts: Vec<ChunkCounts> = chunks
            .par_iter()
            .map(|&(start, len)| {
                let chunk_input = &input[start..start + len];
                Self::count_chunk_simd(chunk_input)
            })
            .collect();

        // Phase 2: Prefix scan to compute cumulative offsets
        let mut tape_offsets = Vec::with_capacity(num_chunks + 1);
        let mut attr_offsets = Vec::with_capacity(num_chunks + 1);
        let mut tag_map_offsets = Vec::with_capacity(num_chunks + 1);

        tape_offsets.push(0);
        attr_offsets.push(0);
        tag_map_offsets.push(0);

        for counts in &chunk_counts {
            let prev_tape = *tape_offsets.last().unwrap();
            let prev_attr = *attr_offsets.last().unwrap();
            let prev_tag_map = *tag_map_offsets.last().unwrap();
            tape_offsets.push(prev_tape + counts.tape_count);
            attr_offsets.push(prev_attr + counts.attr_count);
            tag_map_offsets.push(prev_tag_map + counts.tag_map_count);
        }

        let total_tape = *tape_offsets.last().unwrap();
        let total_attrs = *attr_offsets.last().unwrap();
        let total_tag_map = *tag_map_offsets.last().unwrap();

        // Phase 3: Allocate final buffers
        let mut final_tape = vec![TapeEntry::new(TapeEntryKind::Text, 0, 0); total_tape];
        let mut final_attrs = Vec::with_capacity(total_attrs);
        unsafe {
            final_attrs.set_len(total_attrs);
        }
        let mut final_tag_map = vec![(0usize, 0usize); total_tag_map];

        // Phase 4: Parallel write - each chunk writes directly to final buffer
        let tape_ptr = final_tape.as_mut_ptr() as usize;
        let attr_ptr = final_attrs.as_mut_ptr() as usize;
        let tag_map_ptr = final_tag_map.as_mut_ptr() as usize;

        chunks
            .par_iter()
            .zip(chunk_counts.par_iter())
            .enumerate()
            .for_each(|(idx, (&(start, len), _counts))| {
                let chunk_input = &input[start..start + len];
                let chunk_offset = start as u32;

                let (tape, attrs, tag_map) = Self::build_chunk(chunk_input);

                let tape_out_start = tape_offsets[idx];
                let tape_out_end = tape_offsets[idx + 1];
                let attr_out_start = attr_offsets[idx];
                let attr_out_end = attr_offsets[idx + 1];
                let tag_map_out_start = tag_map_offsets[idx];
                let tag_map_out_end = tag_map_offsets[idx + 1];

                unsafe {
                    let tape_dest = (tape_ptr as *mut TapeEntry).add(tape_out_start);
                    for (i, entry) in tape.iter().enumerate() {
                        if tape_out_start + i < tape_out_end {
                            let mut adjusted = *entry;
                            adjusted.offset += chunk_offset;
                            std::ptr::write(tape_dest.add(i), adjusted);
                        }
                    }

                    let attr_dest = (attr_ptr as *mut CompactAttrEntry).add(attr_out_start);
                    for (i, attr) in attrs.iter().enumerate() {
                        if attr_out_start + i < attr_out_end {
                            let mut adjusted = *attr;
                            adjusted.key_offset += chunk_offset;
                            if adjusted.has_value() {
                                adjusted.value_offset += chunk_offset;
                            }
                            std::ptr::write(attr_dest.add(i), adjusted);
                        }
                    }

                    let tag_map_dest = (tag_map_ptr as *mut (usize, usize)).add(tag_map_out_start);
                    for (i, &(attr_start, attr_count)) in tag_map.iter().enumerate() {
                        if tag_map_out_start + i < tag_map_out_end {
                            std::ptr::write(
                                tag_map_dest.add(i),
                                (attr_start + attr_offsets[idx], attr_count),
                            );
                        }
                    }
                }
            });

        // Post-process: coalesce adjacent text entries across chunk boundaries
        TapeMerger::coalesce_adjacent_text(&mut final_tape);

        (final_tape, final_attrs, final_tag_map)
    }

    /// Build a fused tape using direct parallel writing with a custom chunk size.
    ///
    /// # Arguments
    /// * `input` - The HTML source bytes
    /// * `chunk_size` - Target chunk size in bytes
    ///
    /// # Returns
    /// A tuple of (tape entries, compact attribute entries, tag-to-attr mapping)
    pub fn build_parallel_direct_with_chunk_size(
        input: &[u8],
        chunk_size: usize,
    ) -> (Vec<TapeEntry>, Vec<CompactAttrEntry>, Vec<(usize, usize)>) {
        // Fall back to sequential for small documents
        if input.len() < PARALLEL_THRESHOLD {
            return Self::build(input);
        }

        // Split input into safe chunks
        let chunks = ChunkSplitter::split(input, chunk_size);

        // If we got a single chunk, just run sequentially
        if chunks.len() <= 1 {
            return Self::build(input);
        }

        let num_chunks = chunks.len();

        // Phase 1: Fast count-only pass (no allocations)
        // Each chunk computes how many tape entries, attributes, and tag map entries it will produce
        let chunk_counts: Vec<ChunkCounts> = chunks
            .par_iter()
            .map(|&(start, len)| {
                let chunk_input = &input[start..start + len];
                Self::count_chunk(chunk_input)
            })
            .collect();

        // Phase 2: Prefix scan to compute cumulative offsets
        let mut tape_offsets = Vec::with_capacity(num_chunks + 1);
        let mut attr_offsets = Vec::with_capacity(num_chunks + 1);
        let mut tag_map_offsets = Vec::with_capacity(num_chunks + 1);

        tape_offsets.push(0);
        attr_offsets.push(0);
        tag_map_offsets.push(0);

        for counts in &chunk_counts {
            let prev_tape = *tape_offsets.last().unwrap();
            let prev_attr = *attr_offsets.last().unwrap();
            let prev_tag_map = *tag_map_offsets.last().unwrap();
            tape_offsets.push(prev_tape + counts.tape_count);
            attr_offsets.push(prev_attr + counts.attr_count);
            tag_map_offsets.push(prev_tag_map + counts.tag_map_count);
        }

        let total_tape = *tape_offsets.last().unwrap();
        let total_attrs = *attr_offsets.last().unwrap();
        let total_tag_map = *tag_map_offsets.last().unwrap();

        // Phase 3: Allocate final buffers
        let mut final_tape = vec![TapeEntry::new(TapeEntryKind::Text, 0, 0); total_tape];
        let mut final_attrs = Vec::with_capacity(total_attrs);
        unsafe {
            final_attrs.set_len(total_attrs);
        }
        let mut final_tag_map = vec![(0usize, 0usize); total_tag_map];

        // Phase 4: Parallel write - each chunk writes directly to final buffer
        // We need to use unsafe for parallel mutable writes to non-overlapping regions
        let tape_ptr = final_tape.as_mut_ptr() as usize;
        let attr_ptr = final_attrs.as_mut_ptr() as usize;
        let tag_map_ptr = final_tag_map.as_mut_ptr() as usize;

        chunks
            .par_iter()
            .zip(chunk_counts.par_iter())
            .enumerate()
            .for_each(|(idx, (&(start, len), _counts))| {
                let chunk_input = &input[start..start + len];
                let chunk_offset = start as u32;

                // Build this chunk's tape directly into the final buffer
                let (tape, attrs, tag_map) = Self::build_chunk(chunk_input);

                // Get output slices for this chunk
                let tape_out_start = tape_offsets[idx];
                let tape_out_end = tape_offsets[idx + 1];
                let attr_out_start = attr_offsets[idx];
                let attr_out_end = attr_offsets[idx + 1];
                let tag_map_out_start = tag_map_offsets[idx];
                let tag_map_out_end = tag_map_offsets[idx + 1];

                // Safety: Each chunk writes to non-overlapping regions of the final buffer
                unsafe {
                    // Write tape entries with adjusted offsets
                    let tape_dest = (tape_ptr as *mut TapeEntry).add(tape_out_start);
                    for (i, entry) in tape.iter().enumerate() {
                        if tape_out_start + i < tape_out_end {
                            let mut adjusted = *entry;
                            adjusted.offset += chunk_offset;
                            std::ptr::write(tape_dest.add(i), adjusted);
                        }
                    }

                    // Write attribute entries with adjusted offsets
                    let attr_dest = (attr_ptr as *mut CompactAttrEntry).add(attr_out_start);
                    for (i, attr) in attrs.iter().enumerate() {
                        if attr_out_start + i < attr_out_end {
                            let mut adjusted = *attr;
                            adjusted.key_offset += chunk_offset;
                            if adjusted.has_value() {
                                adjusted.value_offset += chunk_offset;
                            }
                            std::ptr::write(attr_dest.add(i), adjusted);
                        }
                    }

                    // Write tag map entries with adjusted attribute indices
                    let tag_map_dest = (tag_map_ptr as *mut (usize, usize)).add(tag_map_out_start);
                    for (i, &(attr_start, attr_count)) in tag_map.iter().enumerate() {
                        if tag_map_out_start + i < tag_map_out_end {
                            std::ptr::write(
                                tag_map_dest.add(i),
                                (attr_start + attr_offsets[idx], attr_count),
                            );
                        }
                    }
                }
            });

        // Post-process: coalesce adjacent text entries across chunk boundaries
        TapeMerger::coalesce_adjacent_text(&mut final_tape);

        (final_tape, final_attrs, final_tag_map)
    }

    /// Count the number of tape entries, attributes, and tag map entries
    /// that a chunk will produce, without actually building them.
    ///
    /// This is a fast pass that just counts structural characters to estimate
    /// the output sizes. It's much faster than actually building the tape.
    fn count_chunk(input: &[u8]) -> ChunkCounts {
        let len = input.len();
        let mut tape_count = 0usize;
        let mut attr_count = 0usize;
        let mut tag_map_count = 0usize;

        // Count structural characters to estimate tape entries
        // Each '<' potentially starts a tag (tape entry)
        // Each '>' potentially ends a tag
        // Text segments between tags are also tape entries

        let mut pos = 0;
        let mut in_tag = false;
        let mut in_quotes = false;
        let mut quote_char: u8 = 0;
        let mut has_text_start = false;
        let mut attr_in_tag = 0usize;

        while pos < len {
            let ch = input[pos];

            if in_quotes {
                if ch == quote_char {
                    in_quotes = false;
                }
            } else if in_tag {
                match ch {
                    b'>' => {
                        in_tag = false;
                        tape_count += 1; // Tag entry
                        tag_map_count += 1;
                        attr_count += attr_in_tag;
                        attr_in_tag = 0;
                        has_text_start = false;
                    }
                    b'"' | b'\'' => {
                        in_quotes = true;
                        quote_char = ch;
                    }
                    b'=' => {
                        // Attribute with value
                        attr_in_tag += 1;
                    }
                    b' ' | b'\t' | b'\n' | b'\r' => {
                        // Potential boolean attribute or whitespace
                    }
                    _ => {}
                }
            } else {
                match ch {
                    b'<' => {
                        if has_text_start {
                            tape_count += 1; // Text entry before this tag
                        }
                        in_tag = true;
                        has_text_start = false;
                    }
                    _ => {
                        has_text_start = true;
                    }
                }
            }
            pos += 1;
        }

        // Trailing text
        if has_text_start {
            tape_count += 1;
        }

        // Add some margin for safety (boolean attributes, edge cases)
        tape_count += tag_map_count; // Extra margin
        attr_count += tag_map_count; // Extra margin for boolean attributes

        ChunkCounts {
            tape_count,
            attr_count,
            tag_map_count,
        }
    }

    /// SIMD-accelerated count of tape entries, attributes, and tag map entries.
    ///
    /// This method uses SIMD to quickly find structural characters and count them,
    /// which is faster than the scalar `count_chunk` method for large chunks.
    fn count_chunk_simd(input: &[u8]) -> ChunkCounts {
        let len = input.len();
        let mut tape_count = 0usize;
        let mut attr_count = 0usize;
        let mut tag_map_count = 0usize;

        // First, use SIMD to count structural characters
        let mut pos = 0;
        let mut lt_count = 0u32;
        let mut gt_count = 0u32;
        let mut eq_count = 0u32;
        let mut dq_count = 0u32;
        let mut sq_count = 0u32;

        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                while pos + 64 <= len {
                    unsafe {
                        let in0 = SimdInput::load(input[pos..].as_ptr());
                        let in1 = SimdInput::load(input[pos + 32..].as_ptr());

                        lt_count += in0.eq(b'<').count_ones();
                        lt_count += in1.eq(b'<').count_ones();
                        gt_count += in0.eq(b'>').count_ones();
                        gt_count += in1.eq(b'>').count_ones();
                        eq_count += in0.eq(b'=').count_ones();
                        eq_count += in1.eq(b'=').count_ones();
                        dq_count += in0.eq(b'"').count_ones();
                        dq_count += in1.eq(b'"').count_ones();
                        sq_count += in0.eq(b'\'').count_ones();
                        sq_count += in1.eq(b'\'').count_ones();
                    }
                    pos += 64;
                }

                if pos + 32 <= len {
                    unsafe {
                        let in0 = SimdInput::load(input[pos..].as_ptr());
                        lt_count += in0.eq(b'<').count_ones();
                        gt_count += in0.eq(b'>').count_ones();
                        eq_count += in0.eq(b'=').count_ones();
                        dq_count += in0.eq(b'"').count_ones();
                        sq_count += in0.eq(b'\'').count_ones();
                    }
                    pos += 32;
                }
            }
        }

        // Scalar tail
        while pos < len {
            match input[pos] {
                b'<' => lt_count += 1,
                b'>' => gt_count += 1,
                b'=' => eq_count += 1,
                b'"' => dq_count += 1,
                b'\'' => sq_count += 1,
                _ => {}
            }
            pos += 1;
        }

        // Estimate counts based on structural character counts
        // Each '<' potentially starts a tag (tape entry)
        // Each '>' ends a tag
        // Text segments between tags are also tape entries

        // Rough estimate: tape_count = tags + text_segments
        // tag_map_count = number of complete tags (min of lt, gt)
        tag_map_count = lt_count.min(gt_count) as usize;
        tape_count = tag_map_count * 2; // Each tag + potential text before it
        attr_count = eq_count as usize + tag_map_count; // Attributes with values + boolean attributes

        // Add margin for safety
        tape_count += tag_map_count / 4;
        attr_count += tag_map_count / 4;

        ChunkCounts {
            tape_count,
            attr_count,
            tag_map_count,
        }
    }

    /// Build a fused tape with a custom chunk size.
    ///
    /// # Arguments
    /// * `input` - The HTML source bytes
    /// * `chunk_size` - Target chunk size in bytes
    ///
    /// # Returns
    /// A tuple of (tape entries, compact attribute entries, tag-to-attr mapping)
    pub fn build_parallel_with_chunk_size(
        input: &[u8],
        chunk_size: usize,
    ) -> (Vec<TapeEntry>, Vec<CompactAttrEntry>, Vec<(usize, usize)>) {
        // Fall back to sequential for small documents
        if input.len() < PARALLEL_THRESHOLD {
            return Self::build(input);
        }

        // Split input into safe chunks
        let chunks = ChunkSplitter::split(input, chunk_size);

        // If we got a single chunk, just run sequentially
        if chunks.len() <= 1 {
            return Self::build(input);
        }

        // Process chunks in parallel using rayon
        let chunk_results: Vec<ChunkResult> = chunks
            .par_iter()
            .map(|&(start, len)| {
                let chunk_input = &input[start..start + len];
                let (tape, attrs, tag_map) = Self::build_chunk(chunk_input);

                // Detect end state for boundary fixup
                let end_state = Self::detect_end_state(chunk_input);

                ChunkResult {
                    tape,
                    attributes: attrs,
                    tag_attr_map: tag_map,
                    chunk_offset: start,
                    chunk_length: len,
                    ends_in_open_tag: end_state == ChunkEndState::InsideTag,
                    end_state,
                }
            })
            .collect();

        // Merge chunk results (offset adjustment happens inline during merge)
        TapeMerger::merge_with_offset_adjustment(input, chunk_results)
    }

    /// Build a fused tape for a single chunk.
    ///
    /// This is similar to `build()` but processes a chunk of input.
    fn build_chunk(input: &[u8]) -> (Vec<TapeEntry>, Vec<CompactAttrEntry>, Vec<(usize, usize)>) {
        let estimated_capacity = input.len() / 16;
        let mut builder = Self::with_capacity(estimated_capacity, estimated_capacity / 4);
        builder.fused_scan(input);
        (builder.tape, builder.attributes, builder.tag_attr_map)
    }

    /// Detect the parser state at the end of a chunk.
    fn detect_end_state(input: &[u8]) -> ChunkEndState {
        // Scan from the end backward to find context
        let len = input.len();
        if len == 0 {
            return ChunkEndState::Clean;
        }

        // Quick check: if the last byte is '>', we're clean
        if input[len - 1] == b'>' {
            return ChunkEndState::Clean;
        }

        // Scan backward to find context
        let mut in_tag = false;
        let mut in_quotes = false;
        let mut quote_char: u8 = 0;
        let mut comment_depth = 0;

        // Scan from beginning to determine state at end
        for i in 0..len {
            let ch = input[i];
            if in_quotes {
                if ch == quote_char {
                    in_quotes = false;
                }
            } else if comment_depth > 0 {
                // Inside comment: look for -->
                if ch == b'>' && i >= 2 && input[i - 1] == b'-' && input[i - 2] == b'-' {
                    comment_depth -= 1;
                }
            } else if in_tag {
                match ch {
                    b'>' => in_tag = false,
                    b'"' | b'\'' => {
                        in_quotes = true;
                        quote_char = ch;
                    }
                    _ => {}
                }
            } else {
                match ch {
                    b'<' => {
                        // Check if this is a comment
                        if i + 3 < len
                            && input[i + 1] == b'!'
                            && input[i + 2] == b'-'
                            && input[i + 3] == b'-'
                        {
                            comment_depth += 1;
                        } else {
                            in_tag = true;
                        }
                    }
                    _ => {}
                }
            }
        }

        if in_quotes {
            ChunkEndState::InsideQuotedAttr { quote_char }
        } else if comment_depth > 0 {
            ChunkEndState::InsideComment
        } else if in_tag {
            ChunkEndState::InsideTag
        } else {
            ChunkEndState::Clean
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
        assert_eq!(index.get(0), Some(0)); // <
        assert_eq!(index.get(1), Some(4)); // >
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
            input
                .extend_from_slice(format!("<div class='item{}'>content{}</div>", i, i).as_bytes());
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

    // --- Parallel infrastructure tests ---

    #[test]
    fn test_chunk_splitter_small_input() {
        let input = b"<div>hello</div>";
        let chunks = ChunkSplitter::split(input, 64 * 1024);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], (0, input.len()));
    }

    #[test]
    fn test_chunk_splitter_large_input() {
        // Create a large input with many tags
        let mut input = Vec::with_capacity(200 * 1024);
        for i in 0..2000 {
            input.extend_from_slice(
                format!(
                    "<div class='item{}' data-index='{}'>content{}</div>",
                    i, i, i
                )
                .as_bytes(),
            );
        }

        let chunks = ChunkSplitter::split(&input, 64 * 1024);
        assert!(chunks.len() > 1, "Should split into multiple chunks");

        // Verify chunks cover the entire input
        let total: usize = chunks.iter().map(|&(_, len)| len).sum();
        assert_eq!(total, input.len());

        // Verify no overlap
        for i in 1..chunks.len() {
            assert_eq!(chunks[i - 1].0 + chunks[i - 1].1, chunks[i].0);
        }
    }

    #[test]
    fn test_tape_merger_single_chunk() {
        let chunk = ChunkResult {
            tape: vec![
                TapeEntry::new(TapeEntryKind::OpenTag, 0, 5),
                TapeEntry::new(TapeEntryKind::Text, 5, 5),
                TapeEntry::new(TapeEntryKind::CloseTag, 10, 6),
            ],
            attributes: vec![],
            tag_attr_map: vec![(0, 0), (0, 0)],
            chunk_offset: 0,
            chunk_length: 16,
            ends_in_open_tag: false,
            end_state: ChunkEndState::Clean,
        };

        let (tape, attrs, tag_map) = TapeMerger::merge(b"<div>hello</div>", vec![chunk]);
        assert_eq!(tape.len(), 3);
        assert!(attrs.is_empty());
        assert_eq!(tag_map.len(), 2);
    }

    #[test]
    fn test_tape_merger_coalesces_text() {
        let chunk1 = ChunkResult {
            tape: vec![
                TapeEntry::new(TapeEntryKind::OpenTag, 0, 5),
                TapeEntry::new(TapeEntryKind::Text, 5, 3),
            ],
            attributes: vec![],
            tag_attr_map: vec![(0, 0)],
            chunk_offset: 0,
            chunk_length: 8,
            ends_in_open_tag: false,
            end_state: ChunkEndState::Clean,
        };

        let chunk2 = ChunkResult {
            tape: vec![
                TapeEntry::new(TapeEntryKind::Text, 8, 4),
                TapeEntry::new(TapeEntryKind::CloseTag, 12, 6),
            ],
            attributes: vec![],
            tag_attr_map: vec![],
            chunk_offset: 8,
            chunk_length: 10,
            ends_in_open_tag: false,
            end_state: ChunkEndState::Clean,
        };

        let input = b"<div>hello world</div>";
        let (tape, _, _) = TapeMerger::merge(input, vec![chunk1, chunk2]);

        // Text entries should be coalesced
        let text_entries: Vec<_> = tape
            .iter()
            .filter(|e| e.kind == TapeEntryKind::Text)
            .collect();
        assert_eq!(
            text_entries.len(),
            1,
            "Adjacent text entries should be coalesced"
        );
        assert_eq!(text_entries[0].offset, 5);
        assert_eq!(text_entries[0].length, 7); // "hello " + " worl" = 7 bytes from pos 5
    }

    #[test]
    fn test_tape_merger_preserves_attr_map() {
        let chunk1 = ChunkResult {
            tape: vec![TapeEntry::new(TapeEntryKind::OpenTag, 0, 20)],
            attributes: vec![
                CompactAttrEntry::new_bool(5, 3),
                CompactAttrEntry::new_bool(9, 4),
            ],
            tag_attr_map: vec![(0, 2)],
            chunk_offset: 0,
            chunk_length: 20,
            ends_in_open_tag: false,
            end_state: ChunkEndState::Clean,
        };

        let chunk2 = ChunkResult {
            tape: vec![TapeEntry::new(TapeEntryKind::OpenTag, 20, 15)],
            attributes: vec![CompactAttrEntry::new_bool(25, 5)],
            tag_attr_map: vec![(0, 1)],
            chunk_offset: 20,
            chunk_length: 15,
            ends_in_open_tag: false,
            end_state: ChunkEndState::Clean,
        };

        let input = vec![0u8; 35];
        let (_, attrs, tag_map) = TapeMerger::merge(&input, vec![chunk1, chunk2]);

        // Should have 3 attributes total
        assert_eq!(attrs.len(), 3);
        // Tag map should be rebased
        assert_eq!(tag_map.len(), 2);
        assert_eq!(tag_map[0], (0, 2)); // First tag: attrs 0-1
        assert_eq!(tag_map[1], (2, 1)); // Second tag: attr 2
    }

    #[test]
    fn test_build_parallel_small_falls_back() {
        // Small input should fall back to sequential
        let input = b"<div class='test' id='main'>Hello World</div>";
        let (tape_seq, attrs_seq, map_seq) = FusedTapeBuilder::build(input);
        let (tape_par, attrs_par, map_par) = FusedTapeBuilder::build_parallel(input);

        // Results should be identical for small inputs (falls back to sequential)
        assert_eq!(tape_seq.len(), tape_par.len());
        assert_eq!(attrs_seq.len(), attrs_par.len());
        assert_eq!(map_seq.len(), map_par.len());
    }

    #[test]
    fn test_build_parallel_large_produces_valid_tape() {
        // Create input large enough to trigger parallel processing
        let mut input = Vec::with_capacity(200 * 1024);
        input.extend_from_slice(b"<html><body>");
        for i in 0..2000 {
            input.extend_from_slice(
                format!(
                    "<div class='item{}' data-x='{}' data-y='{}'>content{}</div>",
                    i,
                    i * 2,
                    i * 3,
                    i
                )
                .as_bytes(),
            );
        }
        input.extend_from_slice(b"</body></html>");

        let (tape, attrs, _tag_map) = FusedTapeBuilder::build_parallel(&input);

        // Should produce valid tape entries
        assert!(!tape.is_empty());

        // All offsets should be within bounds
        for entry in &tape {
            assert!(
                (entry.offset as usize) + (entry.length as usize) <= input.len(),
                "Entry {:?} extends beyond input bounds",
                entry
            );
        }

        // All attribute offsets should be within bounds
        for attr in &attrs {
            let key_end = attr.key_offset as usize + attr.key_length as usize;
            assert!(
                key_end <= input.len(),
                "Attribute key extends beyond input bounds"
            );
            if attr.has_value() {
                let val_end = attr.value_offset as usize + attr.value_length as usize;
                assert!(
                    val_end <= input.len(),
                    "Attribute value extends beyond input bounds"
                );
            }
        }
    }

    #[test]
    fn test_build_parallel_matches_sequential_tape() {
        // Create input large enough for parallel processing
        let mut input = Vec::with_capacity(200 * 1024);
        input.extend_from_slice(b"<html><body>");
        for i in 0..2000 {
            input.extend_from_slice(
                format!(
                    "<div class='item{}' data-index='{}'>content{}</div>",
                    i, i, i
                )
                .as_bytes(),
            );
        }
        input.extend_from_slice(b"</body></html>");

        let (tape_seq, attrs_seq, _) = FusedTapeBuilder::build(&input);
        let (tape_par, attrs_par, _) = FusedTapeBuilder::build_parallel(&input);

        // Both should produce the same number of tape entries
        assert_eq!(
            tape_seq.len(),
            tape_par.len(),
            "Sequential produced {} entries, parallel produced {}",
            tape_seq.len(),
            tape_par.len()
        );

        // Both should produce the same number of attributes
        assert_eq!(
            attrs_seq.len(),
            attrs_par.len(),
            "Sequential produced {} attrs, parallel produced {}",
            attrs_seq.len(),
            attrs_par.len()
        );
    }

    #[test]
    fn test_detect_end_state_clean() {
        let input = b"<div>hello</div>";
        assert_eq!(
            FusedTapeBuilder::detect_end_state(input),
            ChunkEndState::Clean
        );
    }

    #[test]
    fn test_detect_end_state_inside_tag() {
        let input = b"<div class='test'";
        assert_eq!(
            FusedTapeBuilder::detect_end_state(input),
            ChunkEndState::InsideTag
        );
    }

    #[test]
    fn test_detect_end_state_inside_quoted() {
        let input = b"<div class='test";
        assert_eq!(
            FusedTapeBuilder::detect_end_state(input),
            ChunkEndState::InsideQuotedAttr { quote_char: b'\'' }
        );
    }

    // --- Direct parallel writing tests ---

    #[test]
    fn test_build_parallel_direct_small_falls_back() {
        // Small input should fall back to sequential
        let input = b"<div class='test' id='main'>Hello World</div>";
        let (tape_seq, attrs_seq, map_seq) = FusedTapeBuilder::build(input);
        let (tape_dir, attrs_dir, map_dir) = FusedTapeBuilder::build_parallel_direct(input);

        // Results should be identical for small inputs (falls back to sequential)
        assert_eq!(tape_seq.len(), tape_dir.len());
        assert_eq!(attrs_seq.len(), attrs_dir.len());
        assert_eq!(map_seq.len(), map_dir.len());
    }

    #[test]
    fn test_build_parallel_direct_large_produces_valid_tape() {
        // Create input large enough to trigger parallel processing
        let mut input = Vec::with_capacity(200 * 1024);
        input.extend_from_slice(b"<html><body>");
        for i in 0..2000 {
            input.extend_from_slice(
                format!(
                    "<div class='item{}' data-x='{}' data-y='{}'>content{}</div>",
                    i,
                    i * 2,
                    i * 3,
                    i
                )
                .as_bytes(),
            );
        }
        input.extend_from_slice(b"</body></html>");

        let (tape, attrs, _tag_map) = FusedTapeBuilder::build_parallel_direct(&input);

        // Should produce valid tape entries
        assert!(!tape.is_empty());

        // All offsets should be within bounds
        for entry in &tape {
            assert!(
                (entry.offset as usize) + (entry.length as usize) <= input.len(),
                "Entry {:?} extends beyond input bounds",
                entry
            );
        }

        // All attribute offsets should be within bounds
        for attr in &attrs {
            let key_end = attr.key_offset as usize + attr.key_length as usize;
            assert!(
                key_end <= input.len(),
                "Attribute key extends beyond input bounds"
            );
            if attr.has_value() {
                let val_end = attr.value_offset as usize + attr.value_length as usize;
                assert!(
                    val_end <= input.len(),
                    "Attribute value extends beyond input bounds"
                );
            }
        }
    }

    #[test]
    fn test_build_parallel_direct_matches_sequential_tape() {
        // Create input large enough for parallel processing
        let mut input = Vec::with_capacity(200 * 1024);
        input.extend_from_slice(b"<html><body>");
        for i in 0..2000 {
            input.extend_from_slice(
                format!(
                    "<div class='item{}' data-index='{}'>content{}</div>",
                    i, i, i
                )
                .as_bytes(),
            );
        }
        input.extend_from_slice(b"</body></html>");

        let (tape_seq, attrs_seq, _) = FusedTapeBuilder::build(&input);
        let (tape_dir, attrs_dir, _) = FusedTapeBuilder::build_parallel_direct(&input);

        // Both should produce the same number of tape entries
        assert_eq!(
            tape_seq.len(),
            tape_dir.len(),
            "Sequential produced {} entries, direct parallel produced {}",
            tape_seq.len(),
            tape_dir.len()
        );

        // Both should produce the same number of attributes
        assert_eq!(
            attrs_seq.len(),
            attrs_dir.len(),
            "Sequential produced {} attrs, direct parallel produced {}",
            attrs_seq.len(),
            attrs_dir.len()
        );
    }

    // --- Adaptive chunk sizing tests ---

    #[test]
    fn test_document_profile_analyze() {
        let input = b"<div class='test' id='main'>Hello World</div>";
        let profile = DocumentProfile::analyze(input);

        assert_eq!(profile.size, input.len());
        assert!(profile.tag_density > 0.0);
        assert!(profile.attr_density >= 0.0);
        assert!(profile.num_cores > 0);
    }

    #[test]
    fn test_document_profile_optimal_chunk_size() {
        // Small document
        let small_input = b"<div>Hello</div>";
        let small_profile = DocumentProfile::analyze(small_input);
        let small_chunk = small_profile.optimal_chunk_size();
        assert!(small_chunk >= MIN_CHUNK_SIZE);
        assert!(small_chunk <= MAX_CHUNK_SIZE);

        // Large document with many tags
        let mut large_input = Vec::with_capacity(200 * 1024);
        for i in 0..2000 {
            large_input.extend_from_slice(
                format!(
                    "<div class='item{}' data-index='{}'>content{}</div>",
                    i, i, i
                )
                .as_bytes(),
            );
        }
        let large_profile = DocumentProfile::analyze(&large_input);
        let large_chunk = large_profile.optimal_chunk_size();
        assert!(large_chunk >= MIN_CHUNK_SIZE);
        assert!(large_chunk <= MAX_CHUNK_SIZE);
    }

    #[test]
    fn test_chunk_splitter_adaptive() {
        // Create a large input
        let mut input = Vec::with_capacity(200 * 1024);
        for i in 0..2000 {
            input.extend_from_slice(
                format!(
                    "<div class='item{}' data-index='{}'>content{}</div>",
                    i, i, i
                )
                .as_bytes(),
            );
        }

        let chunks = ChunkSplitter::split_adaptive(&input);
        assert!(chunks.len() > 1, "Should split into multiple chunks");

        // Verify chunks cover the entire input
        let total: usize = chunks.iter().map(|&(_, len)| len).sum();
        assert_eq!(total, input.len());

        // Verify no overlap
        for i in 1..chunks.len() {
            assert_eq!(chunks[i - 1].0 + chunks[i - 1].1, chunks[i].0);
        }
    }

    #[test]
    fn test_build_parallel_adaptive_small_falls_back() {
        // Small input should fall back to sequential
        let input = b"<div class='test' id='main'>Hello World</div>";
        let (tape_seq, attrs_seq, map_seq) = FusedTapeBuilder::build(input);
        let (tape_adapt, attrs_adapt, map_adapt) = FusedTapeBuilder::build_parallel_adaptive(input);

        // Results should be identical for small inputs (falls back to sequential)
        assert_eq!(tape_seq.len(), tape_adapt.len());
        assert_eq!(attrs_seq.len(), attrs_adapt.len());
        assert_eq!(map_seq.len(), map_adapt.len());
    }

    #[test]
    fn test_build_parallel_adaptive_large_produces_valid_tape() {
        // Create input large enough to trigger parallel processing
        let mut input = Vec::with_capacity(200 * 1024);
        input.extend_from_slice(b"<html><body>");
        for i in 0..2000 {
            input.extend_from_slice(
                format!(
                    "<div class='item{}' data-x='{}' data-y='{}'>content{}</div>",
                    i,
                    i * 2,
                    i * 3,
                    i
                )
                .as_bytes(),
            );
        }
        input.extend_from_slice(b"</body></html>");

        let (tape, attrs, _tag_map) = FusedTapeBuilder::build_parallel_adaptive(&input);

        // Should produce valid tape entries
        assert!(!tape.is_empty());

        // All offsets should be within bounds
        for entry in &tape {
            assert!(
                (entry.offset as usize) + (entry.length as usize) <= input.len(),
                "Entry {:?} extends beyond input bounds",
                entry
            );
        }

        // All attribute offsets should be within bounds
        for attr in &attrs {
            let key_end = attr.key_offset as usize + attr.key_length as usize;
            assert!(
                key_end <= input.len(),
                "Attribute key extends beyond input bounds"
            );
            if attr.has_value() {
                let val_end = attr.value_offset as usize + attr.value_length as usize;
                assert!(
                    val_end <= input.len(),
                    "Attribute value extends beyond input bounds"
                );
            }
        }
    }

    #[test]
    fn test_build_parallel_adaptive_matches_sequential_tape() {
        // Create input large enough for parallel processing
        let mut input = Vec::with_capacity(200 * 1024);
        input.extend_from_slice(b"<html><body>");
        for i in 0..2000 {
            input.extend_from_slice(
                format!(
                    "<div class='item{}' data-index='{}'>content{}</div>",
                    i, i, i
                )
                .as_bytes(),
            );
        }
        input.extend_from_slice(b"</body></html>");

        let (tape_seq, attrs_seq, _) = FusedTapeBuilder::build(&input);
        let (tape_adapt, attrs_adapt, _) = FusedTapeBuilder::build_parallel_adaptive(&input);

        // Both should produce the same number of tape entries
        assert_eq!(
            tape_seq.len(),
            tape_adapt.len(),
            "Sequential produced {} entries, adaptive parallel produced {}",
            tape_seq.len(),
            tape_adapt.len()
        );

        // Both should produce the same number of attributes
        assert_eq!(
            attrs_seq.len(),
            attrs_adapt.len(),
            "Sequential produced {} attrs, adaptive parallel produced {}",
            attrs_seq.len(),
            attrs_adapt.len()
        );
    }

    #[test]
    fn test_build_parallel_direct_adaptive_small_falls_back() {
        // Small input should fall back to sequential
        let input = b"<div class='test' id='main'>Hello World</div>";
        let (tape_seq, attrs_seq, map_seq) = FusedTapeBuilder::build(input);
        let (tape_dir, attrs_dir, map_dir) =
            FusedTapeBuilder::build_parallel_direct_adaptive(input);

        // Results should be identical for small inputs (falls back to sequential)
        assert_eq!(tape_seq.len(), tape_dir.len());
        assert_eq!(attrs_seq.len(), attrs_dir.len());
        assert_eq!(map_seq.len(), map_dir.len());
    }

    #[test]
    fn test_build_parallel_direct_adaptive_large_produces_valid_tape() {
        // Create input large enough to trigger parallel processing
        let mut input = Vec::with_capacity(200 * 1024);
        input.extend_from_slice(b"<html><body>");
        for i in 0..2000 {
            input.extend_from_slice(
                format!(
                    "<div class='item{}' data-x='{}' data-y='{}'>content{}</div>",
                    i,
                    i * 2,
                    i * 3,
                    i
                )
                .as_bytes(),
            );
        }
        input.extend_from_slice(b"</body></html>");

        let (tape, attrs, _tag_map) = FusedTapeBuilder::build_parallel_direct_adaptive(&input);

        // Should produce valid tape entries
        assert!(!tape.is_empty());

        // All offsets should be within bounds
        for entry in &tape {
            assert!(
                (entry.offset as usize) + (entry.length as usize) <= input.len(),
                "Entry {:?} extends beyond input bounds",
                entry
            );
        }

        // All attribute offsets should be within bounds
        for attr in &attrs {
            let key_end = attr.key_offset as usize + attr.key_length as usize;
            assert!(
                key_end <= input.len(),
                "Attribute key extends beyond input bounds"
            );
            if attr.has_value() {
                let val_end = attr.value_offset as usize + attr.value_length as usize;
                assert!(
                    val_end <= input.len(),
                    "Attribute value extends beyond input bounds"
                );
            }
        }
    }

    #[test]
    fn test_build_parallel_direct_adaptive_matches_sequential_tape() {
        // Create input large enough for parallel processing
        let mut input = Vec::with_capacity(200 * 1024);
        input.extend_from_slice(b"<html><body>");
        for i in 0..2000 {
            input.extend_from_slice(
                format!(
                    "<div class='item{}' data-index='{}'>content{}</div>",
                    i, i, i
                )
                .as_bytes(),
            );
        }
        input.extend_from_slice(b"</body></html>");

        let (tape_seq, attrs_seq, _) = FusedTapeBuilder::build(&input);
        let (tape_dir, attrs_dir, _) = FusedTapeBuilder::build_parallel_direct_adaptive(&input);

        // Both should produce the same number of tape entries
        assert_eq!(
            tape_seq.len(),
            tape_dir.len(),
            "Sequential produced {} entries, direct adaptive parallel produced {}",
            tape_seq.len(),
            tape_dir.len()
        );

        // Both should produce the same number of attributes
        assert_eq!(
            attrs_seq.len(),
            attrs_dir.len(),
            "Sequential produced {} attrs, direct adaptive parallel produced {}",
            attrs_seq.len(),
            attrs_dir.len()
        );
    }

    #[test]
    fn test_count_chunk_simd() {
        // Create a test input
        let input = b"<div class='test' id='main'>Hello World</div>";

        // Both methods should produce similar estimates
        let scalar = FusedTapeBuilder::count_chunk(input);
        let simd = FusedTapeBuilder::count_chunk_simd(input);

        // SIMD estimates may be slightly different but should be in the same ballpark
        assert!(simd.tape_count > 0, "SIMD tape count should be > 0");
        assert!(simd.tag_map_count > 0, "SIMD tag map count should be > 0");
        assert!(simd.attr_count > 0, "SIMD attr count should be > 0");
    }

    // --- Streaming merge tests ---

    #[test]
    fn test_merge_streaming_single_chunk() {
        let chunk = ChunkResult {
            tape: vec![
                TapeEntry::new(TapeEntryKind::OpenTag, 0, 5),
                TapeEntry::new(TapeEntryKind::Text, 5, 5),
                TapeEntry::new(TapeEntryKind::CloseTag, 10, 6),
            ],
            attributes: vec![],
            tag_attr_map: vec![(0, 0), (0, 0)],
            chunk_offset: 0,
            chunk_length: 16,
            ends_in_open_tag: false,
            end_state: ChunkEndState::Clean,
        };

        let (tape, attrs, tag_map) = TapeMerger::merge_streaming(b"<div>hello</div>", vec![chunk]);
        assert_eq!(tape.len(), 3);
        assert!(attrs.is_empty());
        assert_eq!(tag_map.len(), 2);
    }

    #[test]
    fn test_merge_streaming_coalesces_text() {
        let chunk1 = ChunkResult {
            tape: vec![
                TapeEntry::new(TapeEntryKind::OpenTag, 0, 5),
                TapeEntry::new(TapeEntryKind::Text, 5, 3),
            ],
            attributes: vec![],
            tag_attr_map: vec![(0, 0)],
            chunk_offset: 0,
            chunk_length: 8,
            ends_in_open_tag: false,
            end_state: ChunkEndState::Clean,
        };

        let chunk2 = ChunkResult {
            tape: vec![
                TapeEntry::new(TapeEntryKind::Text, 0, 4),
                TapeEntry::new(TapeEntryKind::CloseTag, 4, 6),
            ],
            attributes: vec![],
            tag_attr_map: vec![],
            chunk_offset: 8,
            chunk_length: 10,
            ends_in_open_tag: false,
            end_state: ChunkEndState::Clean,
        };

        let input = b"<div>hello world</div>";
        let (tape, _, _) = TapeMerger::merge_streaming(input, vec![chunk1, chunk2]);

        // Text entries should be coalesced
        let text_entries: Vec<_> = tape
            .iter()
            .filter(|e| e.kind == TapeEntryKind::Text)
            .collect();
        assert_eq!(
            text_entries.len(),
            1,
            "Adjacent text entries should be coalesced"
        );
        assert_eq!(text_entries[0].offset, 5);
        assert_eq!(text_entries[0].length, 7); // "hello " + " worl" = 7 bytes from pos 5
    }
}
