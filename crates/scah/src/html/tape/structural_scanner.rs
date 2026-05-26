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
