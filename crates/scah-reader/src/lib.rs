use std::ops::Range;

pub mod simd;

use simd::{
    AttributeScanResult, CpuFeatures, ScannerBackend, create_scanner, eof_simd, eq_ignore_case_4,
    find_any_of_32, find_attribute_boundary_scalar, find_tag_open_scalar, is_self_closing_tag,
    scan_attributes_scalar, skip_whitespace_simd as skip_whitespace_simd_impl,
};
pub use simd::{BoundaryHit, BoundaryKind};

/// Precomputed 256-byte LUT for ASCII whitespace detection.
/// Replaces the 5-alternative `matches!` in the hot `skip_whitespace` loop
/// with a single L1-cache lookup per byte.
static WS_LUT: [bool; 256] = {
    let mut lut = [false; 256];
    lut[0x20] = true; // space
    lut[0x09] = true; // tab
    lut[0x0A] = true; // LF
    lut[0x0D] = true; // CR
    lut[0x0C] = true; // FF
    lut
};

pub struct Reader<'a> {
    source: &'a [u8],
    position: usize,
    scanner: Option<Box<dyn ScannerBackend>>,
}

impl<'a> Reader<'a> {
    pub fn new(input: &'a str) -> Self {
        Self::from_parts(input.as_bytes(), 0, false)
    }

    pub fn new_with_simd(input: &'a str) -> Self {
        Self::from_parts(input.as_bytes(), 0, true)
    }

    pub fn from_bytes(input: &'a [u8]) -> Self {
        Self::from_parts(input, 0, false)
    }

    pub fn from_bytes_with_simd(input: &'a [u8]) -> Self {
        Self::from_parts(input, 0, true)
    }

    fn from_parts(source: &'a [u8], position: usize, enable_scanner: bool) -> Self {
        let scanner = if enable_scanner && CpuFeatures::get().has_simd() {
            Some(create_scanner())
        } else {
            None
        };
        Self {
            source,
            position: position.min(source.len()),
            scanner,
        }
    }

    /// Get CPU features information
    pub fn cpu_features(&self) -> &CpuFeatures {
        CpuFeatures::get()
    }

    /// Get scanner backend name
    pub fn scanner_name(&self) -> &str {
        self.scanner
            .as_ref()
            .map_or("scalar", |scanner| scanner.name())
    }

    /// Whether this reader has the AVX2 attribute-boundary scanner enabled.
    pub fn has_simd_attribute_boundary(&self) -> bool {
        let features = self.cpu_features();
        self.scanner.is_some() && (features.has_avx2 || features.has_neon)
    }

    #[inline]
    pub fn get_position(&self) -> usize {
        self.position
    }

    #[inline]
    pub fn slice(&self, range: Range<usize>) -> &'a str {
        // SAFETY: The source was originally a &str, and structural characters are ASCII.
        // Should be careful about slicing in the middle of a UTF-8 character.
        unsafe { std::str::from_utf8_unchecked(&self.source[range]) }
    }

    #[inline]
    pub fn peek(&self) -> Option<u8> {
        self.source.get(self.position).copied()
    }

    #[inline]
    pub fn next_while_list(&mut self, characters: &[u8]) {
        let len = self.source.len();
        while self.position < len && characters.contains(&self.source[self.position]) {
            self.position += 1;
        }
    }

    #[inline]
    pub fn next_while(&mut self, character: u8) {
        let len = self.source.len();
        while self.position < len && self.source[self.position] == character {
            self.position += 1;
        }
    }

    #[inline]
    pub fn next_until_list(&mut self, characters: &[u8]) {
        let len = self.source.len();
        while self.position < len && !characters.contains(&self.source[self.position]) {
            self.position += 1;
        }
    }

    /// SIMD-accelerated next_until for multiple characters
    pub fn next_until_list_simd(&mut self, characters: &[u8; 4]) {
        let pos = find_any_of_32(self.source, self.position, characters);
        self.position = pos;
    }

    pub fn next_until(&mut self, character: u8) {
        let remaining = &self.source[self.position..];
        self.position += memchr::memchr(character, remaining).unwrap_or(remaining.len());
    }

    pub fn skip(&mut self) {
        if self.position < self.source.len() {
            self.position += 1;
        }
    }

    /// LUT-accelerated whitespace skipping — replaces the 5-alternative
    /// `matches!` with a single L1-cache lookup per byte.
    #[inline]
    pub fn skip_whitespace(&mut self) {
        let src = self.source;
        let mut pos = self.position;
        let len = src.len();
        while pos < len && WS_LUT[src[pos] as usize] {
            pos += 1;
        }
        self.position = pos;
    }

    /// SIMD-accelerated whitespace skipping for SIMD-aware parsing paths.
    pub fn skip_whitespace_simd(&mut self) {
        let features = self.cpu_features();
        if self.scanner.is_some() && (features.has_avx2 || features.has_neon) {
            self.position = skip_whitespace_simd_impl(self.source, self.position);
        } else {
            self.skip_whitespace();
        }
    }

    pub fn eof(&self) -> bool {
        if self.position >= self.source.len() {
            return true;
        }

        self.source[self.position..]
            .iter()
            .all(|b| b.is_ascii_whitespace())
    }

    /// SIMD-accelerated eof check
    pub fn eof_simd(&self) -> bool {
        eof_simd(self.source, self.position)
    }

    /// SIMD-accelerated eof check (alias for eof_simd)
    pub fn eof_fast(&self) -> bool {
        self.eof_simd()
    }

    pub fn match_ignore_case(&self, s: &str) -> bool {
        if self.position + s.len() > self.source.len() {
            return false;
        }
        let slice = &self.source[self.position..self.position + s.len()];
        slice.eq_ignore_ascii_case(s.as_bytes())
    }

    /// Fast self-closing tag check using SWAR
    pub fn is_self_closing_tag(&self, name: &[u8]) -> bool {
        is_self_closing_tag(name)
    }

    /// Fast case-insensitive comparison using SWAR
    pub fn eq_ignore_case_4(&self, a: &[u8], b: [u8; 4]) -> bool {
        eq_ignore_case_4(a, b)
    }

    /// Use SIMD scanner to find tag open
    pub fn find_tag_open(&self) -> usize {
        self.scanner.as_ref().map_or_else(
            || find_tag_open_scalar(self.source, self.position),
            |scanner| scanner.find_tag_open(self.source, self.position),
        )
    }

    /// Use SIMD scanner to scan attributes
    pub fn scan_attributes(&self) -> AttributeScanResult {
        self.scanner.as_ref().map_or_else(
            || scan_attributes_scalar(self.source, self.position),
            |scanner| scanner.scan_attributes(self.source, self.position),
        )
    }

    /// Use SIMD to find the first attribute boundary character from current position.
    /// Returns the position and type of the boundary character (quote, `=`, whitespace, or `>`).
    ///
    /// Uses a fast-path that bypasses the `ScannerBackend` trait dispatch when
    /// AVX2 or NEON is available — the virtual call overhead is significant
    /// for short attribute tokens (3–15 bytes, the common case).
    #[inline]
    pub fn find_attribute_boundary(&self) -> Option<BoundaryHit> {
        // Fast path: direct SIMD call, no trait dispatch
        #[cfg(target_arch = "x86_64")]
        {
            if self.scanner.is_some() && is_x86_feature_detected!("avx2") {
                return unsafe {
                    simd::find_attribute_boundary_avx2(self.source, self.position)
                };
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            if self.scanner.is_some() && std::arch::is_aarch64_feature_detected!("neon") {
                return unsafe {
                    simd::find_attribute_boundary_neon(self.source, self.position)
                };
            }
        }
        // Fallback: scalar or no scanner
        find_attribute_boundary_scalar(self.source, self.position)
    }

    /// Find attribute boundary starting from a specific position
    #[inline]
    pub fn find_attribute_boundary_from(&self, start: usize) -> Option<BoundaryHit> {
        #[cfg(target_arch = "x86_64")]
        {
            if self.scanner.is_some() && is_x86_feature_detected!("avx2") {
                return unsafe { simd::find_attribute_boundary_avx2(self.source, start) };
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            if self.scanner.is_some() && std::arch::is_aarch64_feature_detected!("neon") {
                return unsafe { simd::find_attribute_boundary_neon(self.source, start) };
            }
        }
        find_attribute_boundary_scalar(self.source, start)
    }

    /// Set the reader position directly
    ///
    /// This is used by the tape parser to seek to specific positions
    /// identified during structural indexing.
    ///
    /// # Safety
    /// The caller must ensure `pos` is within bounds and at a valid UTF-8 boundary
    #[inline]
    pub fn set_position(&mut self, pos: usize) {
        debug_assert!(
            pos <= self.source.len(),
            "Position {} out of bounds (len: {})",
            pos,
            self.source.len()
        );
        self.position = pos.min(self.source.len());
    }

    /// Get a reference to the source bytes
    ///
    /// This is used by the tape parser to access the original source
    /// for extracting content at specific positions.
    #[inline]
    pub fn source_bytes(&self) -> &'a [u8] {
        self.source
    }

    /// Create a new reader starting at a specific position
    ///
    /// This creates a new reader that shares the same source but starts
    /// at the given position. Useful for the tape parser to create
    /// readers at specific structural positions.
    pub fn from_position(source: &'a [u8], position: usize) -> Self {
        Self::from_parts(source, position, false)
    }

    pub fn from_position_with_simd(source: &'a [u8], position: usize) -> Self {
        Self::from_parts(source, position, true)
    }
}

impl<'a> Iterator for Reader<'a> {
    type Item = u8;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.position < self.source.len() {
            let b = self.source[self.position];
            self.position += 1;
            Some(b)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_iterator() {
        let my_string = String::from("Hello World");
        let mut reader = Reader::new(&my_string);

        assert_eq!(reader.get_position(), 0);
        assert_eq!(reader.next(), Some(b'H'));

        assert_eq!(reader.get_position(), 1);
        assert_eq!(reader.next(), Some(b'e'));

        assert_eq!(reader.get_position(), 2);
        assert_eq!(reader.next(), Some(b'l'));

        assert_eq!(reader.get_position(), 3);
        assert_eq!(reader.next(), Some(b'l'));

        assert_eq!(reader.get_position(), 4);
        assert_eq!(reader.next(), Some(b'o'));

        assert_eq!(reader.get_position(), 5);
        assert_eq!(reader.next(), Some(b' '));

        assert_eq!(reader.get_position(), 6);
        assert_eq!(reader.next(), Some(b'W'));

        assert_eq!(reader.get_position(), 7);
        assert_eq!(reader.next(), Some(b'o'));

        assert_eq!(reader.get_position(), 8);
        assert_eq!(reader.next(), Some(b'r'));

        assert_eq!(reader.get_position(), 9);
        assert_eq!(reader.next(), Some(b'l'));

        assert_eq!(reader.get_position(), 10);
        assert_eq!(reader.peek(), Some(b'd'));

        assert_eq!(reader.get_position(), 10);
        assert_eq!(reader.next(), Some(b'd'));

        assert_eq!(reader.slice(0..5), "Hello");
    }

    #[test]
    fn next_until_stops_at_delimiter() {
        let mut reader = Reader::new("abc<def");

        reader.next_until(b'<');

        assert_eq!(reader.get_position(), 3);
        assert_eq!(reader.peek(), Some(b'<'));
    }

    #[test]
    fn next_until_moves_to_end_when_delimiter_is_absent() {
        let mut reader = Reader::new("abcdef");

        reader.next_until(b'<');

        assert_eq!(reader.get_position(), 6);
        assert_eq!(reader.peek(), None);
    }

    #[test]
    fn next_until_does_not_advance_when_delimiter_is_current() {
        let mut reader = Reader::new("<abcdef");

        reader.next_until(b'<');

        assert_eq!(reader.get_position(), 0);
        assert_eq!(reader.peek(), Some(b'<'));
    }

    #[test]
    fn next_until_handles_long_spans() {
        let mut input = "a".repeat(16 * 1024);
        input.push('<');
        input.push_str("tail");
        let mut reader = Reader::new(&input);

        reader.next_until(b'<');

        assert_eq!(reader.get_position(), 16 * 1024);
        assert_eq!(reader.peek(), Some(b'<'));
    }

    #[test]
    fn test_simd_methods() {
        let html = "<div class=\"test\">hello</div>";
        let reader = Reader::new(html);

        // Test SIMD eof check
        assert!(!reader.eof_simd());

        // Test scanner backend
        println!("Scanner backend: {}", reader.scanner_name());
        println!("CPU features: {:?}", reader.cpu_features());
    }

    #[test]
    fn test_simd_reader_selects_platform_backend() {
        let reader = Reader::new_with_simd("<div class=\"test\">hello</div>");
        let features = reader.cpu_features();

        if features.has_avx2 {
            assert_eq!(reader.scanner_name(), "avx2");
            assert!(reader.has_simd_attribute_boundary());
        } else if features.has_sse42 {
            assert_eq!(reader.scanner_name(), "sse42");
            assert!(!reader.has_simd_attribute_boundary());
        } else if features.has_neon {
            assert_eq!(reader.scanner_name(), "neon");
            assert!(reader.has_simd_attribute_boundary());
        } else {
            assert_eq!(reader.scanner_name(), "scalar");
            assert!(!reader.has_simd_attribute_boundary());
        }
    }

    #[test]
    fn test_skip_whitespace() {
        let html = "  \t\n\r  <div>test</div>";
        let mut reader = Reader::new(html);

        reader.skip_whitespace();
        assert_eq!(reader.get_position(), 7); // Position of '<'
        assert_eq!(reader.peek(), Some(b'<'));
    }

    #[test]
    fn test_next_until_list_simd() {
        let html = "hello world <div>test";
        let mut reader = Reader::new(html);

        reader.next_until_list_simd(&[b'<', b'>', b'/', b'"']);
        assert_eq!(reader.get_position(), 12); // Position of '<'
        assert_eq!(reader.peek(), Some(b'<'));
    }

    #[test]
    fn test_find_attribute_boundary() {
        let html = "key=value rest";
        let reader = Reader::new(html);

        let hit = reader.find_attribute_boundary().unwrap();
        assert_eq!(hit.position, 3); // '='
        assert_eq!(hit.kind, crate::simd::BoundaryKind::Equals);
    }

    #[test]
    fn test_find_attribute_boundary_from() {
        let html = "key=value>rest";
        let reader = Reader::new(html);

        // Start after the '='
        let hit = reader.find_attribute_boundary_from(4).unwrap();
        assert_eq!(hit.position, 9); // '>'
        assert_eq!(hit.kind, crate::simd::BoundaryKind::Gt);
    }
}
