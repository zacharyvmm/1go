const TABLES: ([u8; 16], [u8; 16], u8, u8, u8, u8, u8) = scah_macros::simd_nibble_tables! {
    less_than: [b'<'],
    greater_than: [b'>'],
    quote: [b'"', b'\''],
    equals: [b'='],
    whitespace: [b' ', b'\t', b'\n', b'\r', b'\x0c'],
};

const TLO: [u8; 16] = TABLES.0;
const THI: [u8; 16] = TABLES.1;
const LESS_THAN_BITS: u8 = TABLES.2;
const GREATER_THAN_BITS: u8 = TABLES.3;
const QUOTE_BITS: u8 = TABLES.4;
#[cfg(test)]
const EQUALS_BITS: u8 = TABLES.5;
#[cfg(test)]
const WHITESPACE_BITS: u8 = TABLES.6;
const TAG_END_BITS: u8 = GREATER_THAN_BITS | QUOTE_BITS;
#[cfg(test)]
const STRUCTURAL_BITS: u8 = LESS_THAN_BITS | TAG_END_BITS | EQUALS_BITS;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClassMasks {
    pub tag_end: u16,
    pub less_than: u16,
}

#[derive(Debug, Clone, Copy)]
enum Backend {
    #[allow(dead_code)]
    Scalar,
    #[cfg(target_arch = "aarch64")]
    Neon,
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    Ssse3,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct BlockClassifier {
    backend: Backend,
}

impl Default for BlockClassifier {
    fn default() -> Self {
        #[cfg(target_arch = "aarch64")]
        let backend = Backend::Neon;

        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        let backend = if std::arch::is_x86_feature_detected!("ssse3") {
            Backend::Ssse3
        } else {
            Backend::Scalar
        };

        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86", target_arch = "x86_64")))]
        let backend = Backend::Scalar;

        Self { backend }
    }
}

impl BlockClassifier {
    #[inline]
    pub fn is_accelerated(&self) -> bool {
        !matches!(self.backend, Backend::Scalar)
    }

    /// Classify exactly 16 bytes from `source` beginning at `start`.
    #[inline]
    pub fn classify(&self, source: &[u8], start: usize) -> ClassMasks {
        debug_assert!(start + 16 <= source.len());
        match self.backend {
            Backend::Scalar => classify_scalar(&source[start..start + 16]),
            #[cfg(target_arch = "aarch64")]
            Backend::Neon => {
                // SAFETY: AArch64 guarantees NEON and the caller supplied a
                // complete 16-byte block.
                unsafe { classify_neon(source.as_ptr().add(start)) }
            }
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            Backend::Ssse3 => {
                // SAFETY: construction selects this backend only after SSSE3
                // detection and the caller supplied a complete block.
                unsafe { classify_ssse3(source.as_ptr().add(start)) }
            }
        }
    }
}

fn classify_scalar(block: &[u8]) -> ClassMasks {
    let mut masks = ClassMasks::default();
    for (lane, &byte) in block.iter().enumerate() {
        let class = TLO[(byte & 0x0f) as usize] & THI[(byte >> 4) as usize];
        masks.tag_end |= u16::from(class & TAG_END_BITS != 0) << lane;
        masks.less_than |= u16::from(class & LESS_THAN_BITS != 0) << lane;
    }
    masks
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn classify_neon(pointer: *const u8) -> ClassMasks {
    use std::arch::aarch64::*;

    // SAFETY: the caller guarantees a readable 16-byte block.
    let bytes = unsafe { vld1q_u8(pointer) };
    let low = vandq_u8(bytes, vdupq_n_u8(0x0f));
    let high = vshrq_n_u8(bytes, 4);
    // SAFETY: both tables are exactly 16 readable bytes.
    let low_table = unsafe { vld1q_u8(TLO.as_ptr()) };
    let high_table = unsafe { vld1q_u8(THI.as_ptr()) };
    let classes = vandq_u8(vqtbl1q_u8(low_table, low), vqtbl1q_u8(high_table, high));

    ClassMasks {
        tag_end: neon_nonzero_mask(vandq_u8(classes, vdupq_n_u8(TAG_END_BITS))),
        less_than: neon_nonzero_mask(vandq_u8(classes, vdupq_n_u8(LESS_THAN_BITS))),
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
fn neon_nonzero_mask(values: std::arch::aarch64::uint8x16_t) -> u16 {
    use std::arch::aarch64::*;

    let nonzero = vmvnq_u8(vceqq_u8(values, vdupq_n_u8(0)));
    let weights =
        unsafe { vld1q_u8([1_u8, 2, 4, 8, 16, 32, 64, 128, 1, 2, 4, 8, 16, 32, 64, 128].as_ptr()) };
    // `nonzero` contains either 0x00 or 0xff in every lane, so masking it
    // with the lane weights directly retains that lane's output bit. Turning
    // it into 0/1 first would make `and` retain only the weight-one lanes.
    let weighted = vandq_u8(nonzero, weights);
    let pairs = vpaddlq_u8(weighted);
    let quads = vpaddlq_u16(pairs);
    let octets = vpaddlq_u32(quads);
    (vgetq_lane_u64(octets, 0) as u16) | ((vgetq_lane_u64(octets, 1) as u16) << 8)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "ssse3")]
unsafe fn classify_ssse3(pointer: *const u8) -> ClassMasks {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    // SAFETY: the caller guarantees a readable 16-byte block.
    let bytes = unsafe { _mm_loadu_si128(pointer.cast()) };
    // SAFETY: both tables are exactly 16 readable bytes.
    let low_table = unsafe { _mm_loadu_si128(TLO.as_ptr().cast()) };
    let high_table = unsafe { _mm_loadu_si128(THI.as_ptr().cast()) };
    let nibble_mask = _mm_set1_epi8(0x0f);
    let low = _mm_and_si128(bytes, nibble_mask);
    let high = _mm_and_si128(_mm_srli_epi16(bytes, 4), nibble_mask);
    let classes = _mm_and_si128(
        _mm_shuffle_epi8(low_table, low),
        _mm_shuffle_epi8(high_table, high),
    );
    let zero = _mm_setzero_si128();
    let tag_end = _mm_and_si128(classes, _mm_set1_epi8(TAG_END_BITS as i8));
    let less_than = _mm_and_si128(classes, _mm_set1_epi8(LESS_THAN_BITS as i8));

    ClassMasks {
        tag_end: (!_mm_movemask_epi8(_mm_cmpeq_epi8(tag_end, zero)) as u16),
        less_than: (!_mm_movemask_epi8(_mm_cmpeq_epi8(less_than, zero)) as u16),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tables_classify_all_bytes_exactly() {
        for byte in u8::MIN..=u8::MAX {
            let class = TLO[(byte & 0x0f) as usize] & THI[(byte >> 4) as usize];
            let structural = matches!(byte, b'<' | b'>' | b'"' | b'\'' | b'=');
            let whitespace = matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0c);
            assert_eq!(class & STRUCTURAL_BITS != 0, structural, "byte {byte}");
            assert_eq!(class & WHITESPACE_BITS != 0, whitespace, "byte {byte}");
            assert_eq!(class & LESS_THAN_BITS != 0, byte == b'<', "byte {byte}");
            assert_eq!(class & GREATER_THAN_BITS != 0, byte == b'>', "byte {byte}");
            assert_eq!(
                class & QUOTE_BITS != 0,
                matches!(byte, b'"' | b'\''),
                "byte {byte}"
            );
            assert_eq!(class & EQUALS_BITS != 0, byte == b'=', "byte {byte}");
        }
    }

    #[test]
    fn selected_backend_matches_scalar_masks() {
        let bytes = b"<a x='y'> \t\n\r\x0c=!";
        assert_eq!(bytes.len(), 16);
        assert_eq!(
            BlockClassifier::default().classify(bytes, 0),
            classify_scalar(bytes)
        );
    }
}
