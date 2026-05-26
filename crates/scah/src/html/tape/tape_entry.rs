//! Tape entry types for flat HTML representation
//!
//! Each `TapeEntry` represents a single structural element in the HTML document.
//! Entries are stored in a flat vector for cache-friendly sequential access.

/// The kind of tape entry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TapeEntryKind {
    /// Opening tag: `<div`, `<a`, etc.
    OpenTag,
    /// Closing tag: `</div>`, `</a>`, etc.
    CloseTag,
    /// Self-closing tag: `<br/>`, `<img/>`, etc.
    SelfClosingTag,
    /// Comment: `<!-- ... -->`
    Comment,
    /// Doctype: `<!DOCTYPE ...>`
    Doctype,
    /// Text content between tags
    Text,
    /// Attribute key (within a tag)
    AttributeKey,
    /// Attribute value (after `=`)
    AttributeValue,
    /// Attribute with no value (boolean attribute)
    AttributeBool,
}

/// A single entry in the HTML tape
///
/// Each entry stores:
/// - The kind of structural element
/// - The byte offset in the original input
/// - The length of the content
///
/// This flat representation enables:
/// - Sequential memory access during parsing
/// - Cache-friendly iteration during DOM construction
/// - O(1) random access by index
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TapeEntry {
    /// The kind of entry
    pub kind: TapeEntryKind,
    /// Byte offset in the original HTML input
    pub offset: u32,
    /// Length of the content in bytes
    pub length: u32,
}

impl TapeEntry {
    /// Create a new tape entry
    #[inline]
    pub fn new(kind: TapeEntryKind, offset: u32, length: u32) -> Self {
        Self {
            kind,
            offset,
            length,
        }
    }

    /// Get the end offset (exclusive)
    #[inline]
    pub fn end(&self) -> u32 {
        self.offset + self.length
    }

    /// Get the byte range for this entry
    #[inline]
    pub fn range(&self) -> std::ops::Range<usize> {
        self.offset as usize..self.end() as usize
    }

    /// Extract the string slice for this entry from the source
    #[inline]
    pub fn slice<'a>(&self, source: &'a [u8]) -> &'a str {
        // SAFETY: We validate that offsets are within bounds during tape construction
        unsafe { std::str::from_utf8_unchecked(&source[self.range()]) }
    }

    /// Check if this is an opening tag (not self-closing)
    #[inline]
    pub fn is_open_tag(&self) -> bool {
        self.kind == TapeEntryKind::OpenTag
    }

    /// Check if this is a closing tag
    #[inline]
    pub fn is_close_tag(&self) -> bool {
        self.kind == TapeEntryKind::CloseTag
    }

    /// Check if this is a self-closing tag
    #[inline]
    pub fn is_self_closing(&self) -> bool {
        self.kind == TapeEntryKind::SelfClosingTag
    }

    /// Check if this is any kind of tag (open, close, or self-closing)
    #[inline]
    pub fn is_tag(&self) -> bool {
        matches!(
            self.kind,
            TapeEntryKind::OpenTag | TapeEntryKind::CloseTag | TapeEntryKind::SelfClosingTag
        )
    }

    /// Check if this is an attribute entry
    #[inline]
    pub fn is_attribute(&self) -> bool {
        matches!(
            self.kind,
            TapeEntryKind::AttributeKey
                | TapeEntryKind::AttributeValue
                | TapeEntryKind::AttributeBool
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tape_entry_creation() {
        let entry = TapeEntry::new(TapeEntryKind::OpenTag, 10, 5);
        assert_eq!(entry.kind, TapeEntryKind::OpenTag);
        assert_eq!(entry.offset, 10);
        assert_eq!(entry.length, 5);
        assert_eq!(entry.end(), 15);
        assert_eq!(entry.range(), 10..15);
    }

    #[test]
    fn test_tape_entry_slice() {
        let source = b"<div class='test'>content</div>";
        let entry = TapeEntry::new(TapeEntryKind::OpenTag, 0, 4);
        assert_eq!(entry.slice(source), "<div");

        let text_entry = TapeEntry::new(TapeEntryKind::Text, 18, 7);
        assert_eq!(text_entry.slice(source), "content");
    }

    #[test]
    fn test_tape_entry_kind_checks() {
        let open = TapeEntry::new(TapeEntryKind::OpenTag, 0, 4);
        assert!(open.is_open_tag());
        assert!(open.is_tag());
        assert!(!open.is_close_tag());
        assert!(!open.is_self_closing());
        assert!(!open.is_attribute());

        let close = TapeEntry::new(TapeEntryKind::CloseTag, 20, 6);
        assert!(close.is_close_tag());
        assert!(close.is_tag());

        let self_close = TapeEntry::new(TapeEntryKind::SelfClosingTag, 0, 5);
        assert!(self_close.is_self_closing());
        assert!(self_close.is_tag());

        let attr_key = TapeEntry::new(TapeEntryKind::AttributeKey, 5, 5);
        assert!(attr_key.is_attribute());
        assert!(!attr_key.is_tag());

        let attr_val = TapeEntry::new(TapeEntryKind::AttributeValue, 12, 6);
        assert!(attr_val.is_attribute());
    }

    #[test]
    fn test_tape_entry_equality() {
        let a = TapeEntry::new(TapeEntryKind::OpenTag, 10, 5);
        let b = TapeEntry::new(TapeEntryKind::OpenTag, 10, 5);
        let c = TapeEntry::new(TapeEntryKind::CloseTag, 10, 5);

        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
