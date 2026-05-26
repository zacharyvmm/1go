//! SIMD abstraction layer for high-performance HTML scanning
//!
//! This module provides portable SIMD operations for processing 32-64 bytes
//! at once, inspired by simdjson's architecture. Supports AVX2, SSE2, and NEON
//! with runtime CPU detection and scalar fallbacks.

/// Runtime CPU feature detection
#[derive(Debug, Clone, Copy)]
pub struct CpuFeatures {
    pub has_avx2: bool,
    pub has_sse42: bool,
    pub has_neon: bool,
    pub has_avx512: bool,
}

impl CpuFeatures {
    /// Detect available CPU features at runtime
    pub fn detect() -> Self {
        Self {
            #[cfg(target_arch = "x86_64")]
            has_avx2: is_x86_feature_detected!("avx2"),
            #[cfg(target_arch = "x86_64")]
            has_sse42: is_x86_feature_detected!("sse4.2"),
            #[cfg(target_arch = "aarch64")]
            has_neon: std::arch::is_aarch64_feature_detected!("neon"),
            #[cfg(target_arch = "x86_64")]
            has_avx512: is_x86_feature_detected!("avx512f"),
            #[cfg(not(target_arch = "x86_64"))]
            has_avx2: false,
            #[cfg(not(target_arch = "x86_64"))]
            has_sse42: false,
            #[cfg(not(target_arch = "aarch64"))]
            has_neon: false,
            #[cfg(not(target_arch = "x86_64"))]
            has_avx512: false,
        }
    }

    /// Check if SIMD is available
    pub fn has_simd(&self) -> bool {
        self.has_avx2 || self.has_sse42 || self.has_neon || self.has_avx512
    }
}

/// Trait for platform-specific scanning implementations
pub trait ScannerBackend: Send + Sync {
    fn name(&self) -> &str;
    fn find_tag_open(&self, input: &[u8], start: usize) -> usize;
    fn scan_attributes(&self, input: &[u8], start: usize) -> AttributeScanResult;
}

/// Result of attribute scanning
#[derive(Debug, Clone)]
pub struct AttributeScanResult {
    pub quotes: u32,
    pub equals: u32,
    pub whitespace: u32,
    pub gt: u32,
}

/// Process 32 bytes at once (AVX2) or 16 bytes (SSE2/NEON fallback)
pub struct SimdInput {
    #[cfg(target_arch = "x86_64")]
    v0: std::arch::x86_64::__m256i,
    #[cfg(target_arch = "aarch64")]
    v0: std::arch::aarch64::uint8x16_t,
}

impl SimdInput {
    /// Load 32 bytes from memory (unaligned)
    #[inline]
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    pub unsafe fn load(ptr: *const u8) -> Self {
        unsafe {
            Self {
                v0: std::arch::x86_64::_mm256_loadu_si256(ptr as *const std::arch::x86_64::__m256i),
            }
        }
    }

    /// Load 16 bytes from memory (unaligned) for SSE2/NEON
    #[inline]
    #[cfg(target_arch = "aarch64")]
    #[target_feature(enable = "neon")]
    pub unsafe fn load(ptr: *const u8) -> Self {
        unsafe {
            Self {
                v0: std::arch::aarch64::vld1q_u8(ptr),
            }
        }
    }

    /// Compare all bytes against a constant, return 32-bit bitmask
    #[inline]
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    pub unsafe fn eq(&self, byte: u8) -> u32 {
        let cmp = std::arch::x86_64::_mm256_cmpeq_epi8(
            self.v0,
            std::arch::x86_64::_mm256_set1_epi8(byte as i8),
        );
        std::arch::x86_64::_mm256_movemask_epi8(cmp) as u32
    }

    /// Compare all bytes against a constant, return 16-bit bitmask (NEON)
    #[inline]
    #[cfg(target_arch = "aarch64")]
    #[target_feature(enable = "neon")]
    pub unsafe fn eq(&self, byte: u8) -> u32 {
        let cmp = std::arch::aarch64::vceqq_u8(
            self.v0,
            std::arch::aarch64::vdupq_n_u8(byte),
        );
        // Convert NEON mask to bitmask
        let mut mask = 0u32;
        let mask_bytes = std::arch::aarch64::vshrn_n_u16(cmp, 4);
        for i in 0..8 {
            if mask_bytes[i] != 0 {
                mask |= 1 << i;
            }
        }
        mask
    }

    /// Check if any byte has high bit set (non-ASCII)
    #[inline]
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    pub unsafe fn is_ascii(&self) -> bool {
        std::arch::x86_64::_mm256_movemask_epi8(self.v0) == 0
    }

    /// Check if any byte has high bit set (non-ASCII) - NEON
    #[inline]
    #[cfg(target_arch = "aarch64")]
    #[target_feature(enable = "neon")]
    pub unsafe fn is_ascii(&self) -> bool {
        let max = std::arch::aarch64::vmaxvq_u8(self.v0);
        max < 128
    }
}

/// SIMD-accelerated multi-byte search
/// Returns position of first match, or len if none found
pub fn find_any_of_32(input: &[u8], start: usize, needles: &[u8; 4]) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            return unsafe { find_any_avx2(input, start, needles) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { find_any_neon(input, start, needles) };
        }
    }
    // Fallback: scalar
    find_any_scalar(input, start, needles)
}

/// AVX2 implementation for finding any of 4 bytes
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn find_any_avx2(input: &[u8], start: usize, needles: &[u8; 4]) -> usize {
    unsafe {
        let mut pos = start;
        let len = input.len();
        let end = len.saturating_sub(32);

        // Broadcast needles to 4 × 32-byte registers
        let n0 = std::arch::x86_64::_mm256_set1_epi8(needles[0] as i8);
        let n1 = std::arch::x86_64::_mm256_set1_epi8(needles[1] as i8);
        let n2 = std::arch::x86_64::_mm256_set1_epi8(needles[2] as i8);
        let n3 = std::arch::x86_64::_mm256_set1_epi8(needles[3] as i8);

        while pos <= end {
            let data = std::arch::x86_64::_mm256_loadu_si256(
                input[pos..].as_ptr() as *const std::arch::x86_64::__m256i
            );
            let m0 = std::arch::x86_64::_mm256_cmpeq_epi8(data, n0);
            let m1 = std::arch::x86_64::_mm256_cmpeq_epi8(data, n1);
            let m2 = std::arch::x86_64::_mm256_cmpeq_epi8(data, n2);
            let m3 = std::arch::x86_64::_mm256_cmpeq_epi8(data, n3);
            let any = std::arch::x86_64::_mm256_or_si256(
                std::arch::x86_64::_mm256_or_si256(m0, m1),
                std::arch::x86_64::_mm256_or_si256(m2, m3),
            );
            let mask = std::arch::x86_64::_mm256_movemask_epi8(any) as u32;

            if mask != 0 {
                return pos + mask.trailing_zeros() as usize;
            }
            pos += 32;
        }

        // Scalar tail
        while pos < len {
            let b = input[pos];
            if b == needles[0] || b == needles[1] || b == needles[2] || b == needles[3] {
                return pos;
            }
            pos += 1;
        }
        len
    }
}

/// NEON implementation for finding any of 4 bytes
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn find_any_neon(input: &[u8], start: usize, needles: &[u8; 4]) -> usize {
    unsafe {
        let mut pos = start;
        let len = input.len();
        let end = len.saturating_sub(16);

        // Broadcast needles to 16-byte registers
        let n0 = std::arch::aarch64::vdupq_n_u8(needles[0]);
        let n1 = std::arch::aarch64::vdupq_n_u8(needles[1]);
        let n2 = std::arch::aarch64::vdupq_n_u8(needles[2]);
        let n3 = std::arch::aarch64::vdupq_n_u8(needles[3]);

        while pos <= end {
            let data = std::arch::aarch64::vld1q_u8(input[pos..].as_ptr());
            let m0 = std::arch::aarch64::vceqq_u8(data, n0);
            let m1 = std::arch::aarch64::vceqq_u8(data, n1);
            let m2 = std::arch::aarch64::vceqq_u8(data, n2);
            let m3 = std::arch::aarch64::vceqq_u8(data, n3);
            let any = std::arch::aarch64::vorrq_u8(
                std::arch::aarch64::vorrq_u8(m0, m1),
                std::arch::aarch64::vorrq_u8(m2, m3),
            );

            // Check if any byte is non-zero
            let max = std::arch::aarch64::vmaxvq_u8(any);
            if max != 0 {
                // Find first set bit
                let mask = std::arch::aarch64::vshrn_n_u16(any, 4);
                for i in 0..16 {
                    if mask[i] != 0 {
                        return pos + i;
                    }
                }
            }
            pos += 16;
        }

        // Scalar tail
        while pos < len {
            let b = input[pos];
            if b == needles[0] || b == needles[1] || b == needles[2] || b == needles[3] {
                return pos;
            }
            pos += 1;
        }
        len
    }
}

/// Scalar fallback for finding any of 4 bytes
fn find_any_scalar(input: &[u8], start: usize, needles: &[u8; 4]) -> usize {
    let mut pos = start;
    let len = input.len();

    while pos < len {
        let b = input[pos];
        if b == needles[0] || b == needles[1] || b == needles[2] || b == needles[3] {
            return pos;
        }
        pos += 1;
    }
    len
}

/// Classify 32 bytes simultaneously as whitespace
/// Returns 32-bit bitmask where 1 = whitespace
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn classify_whitespace_avx2(input: std::arch::x86_64::__m256i) -> u32 {
    // Direct comparison for whitespace characters: space(0x20), tab(0x09), newline(0x0A), CR(0x0D)
    let is_space = std::arch::x86_64::_mm256_cmpeq_epi8(input, std::arch::x86_64::_mm256_set1_epi8(0x20));
    let is_tab = std::arch::x86_64::_mm256_cmpeq_epi8(input, std::arch::x86_64::_mm256_set1_epi8(0x09));
    let is_lf = std::arch::x86_64::_mm256_cmpeq_epi8(input, std::arch::x86_64::_mm256_set1_epi8(0x0A));
    let is_cr = std::arch::x86_64::_mm256_cmpeq_epi8(input, std::arch::x86_64::_mm256_set1_epi8(0x0D));
    let any = std::arch::x86_64::_mm256_or_si256(
        std::arch::x86_64::_mm256_or_si256(is_space, is_tab),
        std::arch::x86_64::_mm256_or_si256(is_lf, is_cr),
    );
    std::arch::x86_64::_mm256_movemask_epi8(any) as u32
}

/// Skip whitespace in bulk using SIMD
pub fn skip_whitespace_simd(input: &[u8], start: usize) -> usize {
    let mut pos = start;
    let len = input.len();

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            while pos + 32 <= len {
                unsafe {
                    let simd_input = SimdInput::load(input[pos..].as_ptr());
                    let ws_mask = classify_whitespace_avx2(simd_input.v0);
                    let non_ws = !ws_mask;
                    if non_ws != 0 {
                        return pos + non_ws.trailing_zeros() as usize;
                    }
                }
                pos += 32;
            }
        }
    }

    // Scalar tail
    while pos < len && input[pos].is_ascii_whitespace() {
        pos += 1;
    }
    pos
}

/// SIMD-accelerated eof check
pub fn eof_simd(input: &[u8], position: usize) -> bool {
    let remaining = &input[position..];
    let mut pos = 0;

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            while pos + 32 <= remaining.len() {
                unsafe {
                    let simd_input = SimdInput::load(remaining[pos..].as_ptr());
                    if !simd_input.is_ascii() {
                        return false; // Non-ASCII = not EOF
                    }
                    let ws_mask = classify_whitespace_avx2(simd_input.v0);
                    if ws_mask != 0xFFFFFFFF {
                        return false; // Non-whitespace ASCII
                    }
                }
                pos += 32;
            }
        }
    }

    // Scalar tail
    remaining[pos..].iter().all(|b| b.is_ascii_whitespace())
}

/// SIMD-accelerated attribute boundary scanning
/// Returns positions of: quotes, equals, whitespace, and '>'
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn scan_attribute_boundaries_avx2(
    input: std::arch::x86_64::__m256i,
) -> (u32, u32, u32, u32) {
    unsafe {
        let quotes = std::arch::x86_64::_mm256_cmpeq_epi8(
            input,
            std::arch::x86_64::_mm256_set1_epi8(b'"' as i8),
        );
        let singles = std::arch::x86_64::_mm256_cmpeq_epi8(
            input,
            std::arch::x86_64::_mm256_set1_epi8(b'\'' as i8),
        );
        let equals = std::arch::x86_64::_mm256_cmpeq_epi8(
            input,
            std::arch::x86_64::_mm256_set1_epi8(b'=' as i8),
        );
        let gt = std::arch::x86_64::_mm256_cmpeq_epi8(
            input,
            std::arch::x86_64::_mm256_set1_epi8(b'>' as i8),
        );

        (
            (std::arch::x86_64::_mm256_movemask_epi8(quotes)
                | std::arch::x86_64::_mm256_movemask_epi8(singles)) as u32,
            std::arch::x86_64::_mm256_movemask_epi8(equals) as u32,
            classify_whitespace_avx2(input),
            std::arch::x86_64::_mm256_movemask_epi8(gt) as u32,
        )
    }
}

/// Convert bitmask to array of set-bit positions
#[inline]
pub fn bitmask_to_indexes(mut mask: u32, base: u32, output: &mut Vec<u32>) {
    while mask != 0 {
        let tz = mask.trailing_zeros();
        output.push(base + tz);
        mask &= mask.wrapping_sub(1); // Clear lowest set bit
    }
}

/// Structural characters for HTML tokenization
pub const HTML_STRUCTURAL: [u8; 7] = [b'<', b'>', b'/', b'"', b'\'', b'=', b'!'];

/// First pass: scan entire input, build index of structural positions
pub fn index_structural_characters(input: &[u8]) -> Vec<u32> {
    let mut indexes = Vec::with_capacity(input.len() / 16);
    let mut pos = 0;
    let len = input.len();

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // Process 64 bytes at a time (2 × 32-byte SIMD chunks)
            while pos + 64 <= len {
                unsafe {
                    let in0 = SimdInput::load(input[pos..].as_ptr());
                    let in1 = SimdInput::load(input[pos + 32..].as_ptr());

                    // Find all structural characters
                    let mask0 = in0.eq(b'<') | in0.eq(b'>') | in0.eq(b'"') | in0.eq(b'\'');
                    let mask1 = in1.eq(b'<') | in1.eq(b'>') | in1.eq(b'"') | in1.eq(b'\'');

                    bitmask_to_indexes(mask0, pos as u32, &mut indexes);
                    bitmask_to_indexes(mask1, (pos + 32) as u32, &mut indexes);
                }
                pos += 64;
            }
        }
    }

    // Scalar tail
    while pos < len {
        if matches!(input[pos], b'<' | b'>' | b'"' | b'\'') {
            indexes.push(pos as u32);
        }
        pos += 1;
    }

    indexes
}

/// Compare 4 bytes case-insensitively using SWAR
#[inline]
pub fn eq_ignore_case_4(a: &[u8], b: [u8; 4]) -> bool {
    if a.len() < 4 {
        return false;
    }
    let mut val = u32::from_ne_bytes([a[0], a[1], a[2], a[3]]);
    // Mask out bit 5 (case bit) from each byte
    val |= 0x20202020;
    let target = u32::from_ne_bytes(b) | 0x20202020;
    val == target
}

/// Compare bytes case-insensitively using SWAR (variable length)
#[inline]
pub fn eq_ignore_case_sw(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    // Process 4 bytes at a time
    let mut i = 0;
    while i + 4 <= a.len() {
        if !eq_ignore_case_4(&a[i..], [b[i], b[i+1], b[i+2], b[i+3]]) {
            return false;
        }
        i += 4;
    }
    // Handle remaining bytes
    while i < a.len() {
        if (a[i] | 0x20) != (b[i] | 0x20) {
            return false;
        }
        i += 1;
    }
    true
}

/// Fast self-closing tag check using SWAR
#[inline]
pub fn is_self_closing_tag(name: &[u8]) -> bool {
    // Check against known self-closing tags using SWAR
    match name.len() {
        2 => eq_ignore_case_sw(name, b"br"),
        3 => {
            eq_ignore_case_sw(name, b"img")
                || eq_ignore_case_sw(name, b"col")
                || eq_ignore_case_sw(name, b"wbr")
        }
        4 => {
            eq_ignore_case_sw(name, b"area")
                || eq_ignore_case_sw(name, b"base")
                || eq_ignore_case_sw(name, b"link")
                || eq_ignore_case_sw(name, b"meta")
        }
        5 => {
            eq_ignore_case_sw(name, b"embed")
                || eq_ignore_case_sw(name, b"input")
                || eq_ignore_case_sw(name, b"param")
                || eq_ignore_case_sw(name, b"track")
        }
        _ => false,
    }
}

/// Select best backend at runtime
pub fn create_scanner() -> Box<dyn ScannerBackend> {
    let features = CpuFeatures::detect();
    if features.has_avx2 {
        Box::new(Avx2Scanner)
    } else if features.has_sse42 {
        Box::new(Sse42Scanner)
    } else if features.has_neon {
        Box::new(NeonScanner)
    } else {
        Box::new(ScalarScanner)
    }
}

/// AVX2 scanner implementation
struct Avx2Scanner;

impl ScannerBackend for Avx2Scanner {
    fn name(&self) -> &str {
        "avx2"
    }

    fn find_tag_open(&self, input: &[u8], start: usize) -> usize {
        // Use SIMD to find '<'
        find_any_of_32(input, start, &[b'<', 0, 0, 0])
    }

    fn scan_attributes(&self, _input: &[u8], _start: usize) -> AttributeScanResult {
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                // SIMD implementation would go here
                // For now, return empty result
            }
        }

        // Fallback
        AttributeScanResult {
            quotes: 0,
            equals: 0,
            whitespace: 0,
            gt: 0,
        }
    }
}

/// SSE4.2 scanner implementation
struct Sse42Scanner;

impl ScannerBackend for Sse42Scanner {
    fn name(&self) -> &str {
        "sse42"
    }

    fn find_tag_open(&self, input: &[u8], start: usize) -> usize {
        // Use memchr for SSE4.2
        memchr::memchr(b'<', &input[start..])
            .map(|pos| start + pos)
            .unwrap_or(input.len())
    }

    fn scan_attributes(&self, _input: &[u8], _start: usize) -> AttributeScanResult {
        // Scalar fallback for SSE4.2
        AttributeScanResult {
            quotes: 0,
            equals: 0,
            whitespace: 0,
            gt: 0,
        }
    }
}

/// NEON scanner implementation
struct NeonScanner;

impl ScannerBackend for NeonScanner {
    fn name(&self) -> &str {
        "neon"
    }

    fn find_tag_open(&self, input: &[u8], start: usize) -> usize {
        // Use SIMD to find '<'
        find_any_of_32(input, start, &[b'<', 0, 0, 0])
    }

    fn scan_attributes(&self, _input: &[u8], _start: usize) -> AttributeScanResult {
        // Scalar fallback for NEON
        AttributeScanResult {
            quotes: 0,
            equals: 0,
            whitespace: 0,
            gt: 0,
        }
    }
}

/// Scalar scanner implementation
struct ScalarScanner;

impl ScannerBackend for ScalarScanner {
    fn name(&self) -> &str {
        "scalar"
    }

    fn find_tag_open(&self, input: &[u8], start: usize) -> usize {
        // Use memchr for scalar
        memchr::memchr(b'<', &input[start..])
            .map(|pos| start + pos)
            .unwrap_or(input.len())
    }

    fn scan_attributes(&self, _input: &[u8], _start: usize) -> AttributeScanResult {
        // Scalar fallback
        AttributeScanResult {
            quotes: 0,
            equals: 0,
            whitespace: 0,
            gt: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_features_detection() {
        let features = CpuFeatures::detect();
        println!("CPU Features: {:?}", features);
        // Just ensure it doesn't panic
    }

    #[test]
    fn test_find_any_scalar() {
        let input = b"hello world <div>test";
        let result = find_any_scalar(input, 0, &[b'<', b'>', b'/', b'"']);
        assert_eq!(result, 12); // Position of '<'
    }

    #[test]
    fn test_eof_simd() {
        let input = b"  \t\n\r  ";
        assert!(eof_simd(input, 0));

        let input = b"  hello  ";
        assert!(!eof_simd(input, 0));
    }

    #[test]
    fn test_skip_whitespace_simd() {
        let input = b"  \t\n\r  hello";
        let pos = skip_whitespace_simd(input, 0);
        assert_eq!(pos, 7); // Position of 'h'
    }

    #[test]
    fn test_eq_ignore_case_4() {
        assert!(eq_ignore_case_4(b"divx", *b"divx"));
        assert!(eq_ignore_case_4(b"DIVX", *b"divx"));
        assert!(eq_ignore_case_4(b"DivX", *b"divx"));
        assert!(!eq_ignore_case_4(b"span", *b"divx"));
    }

    #[test]
    fn test_eq_ignore_case_sw() {
        assert!(eq_ignore_case_sw(b"div", b"div"));
        assert!(eq_ignore_case_sw(b"DIV", b"div"));
        assert!(eq_ignore_case_sw(b"Div", b"div"));
        assert!(!eq_ignore_case_sw(b"span", b"div"));
        assert!(eq_ignore_case_sw(b"input", b"INPUT"));
    }

    #[test]
    fn test_is_self_closing_tag() {
        assert!(is_self_closing_tag(b"br"));
        assert!(is_self_closing_tag(b"BR"));
        assert!(is_self_closing_tag(b"img"));
        assert!(is_self_closing_tag(b"input"));
        assert!(!is_self_closing_tag(b"div"));
        assert!(!is_self_closing_tag(b"span"));
    }

    #[test]
    fn test_index_structural_characters() {
        let input = b"<div class=\"test\">hello</div>";
        let indexes = index_structural_characters(input);
        assert!(!indexes.is_empty());
        assert!(indexes.contains(&(0 as u32))); // < at start
        assert!(indexes.contains(&(11 as u32))); // " after =
        assert!(indexes.contains(&(16 as u32))); // " before >
        assert!(indexes.contains(&(17 as u32))); // >
        assert!(indexes.contains(&(23 as u32))); // < before /
        assert!(indexes.contains(&(28 as u32))); // > at end
    }
}