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
    /// A complete attribute entry with pre-tokenized key/value offsets
    /// This is used in the fused tape builder to store attributes compactly
    AttributeEntry,
}

/// Flags for compact attribute entries
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AttrFlags {
    /// Attribute has no value (boolean attribute like `disabled`, `checked`)
    NoValue = 0,
    /// Attribute value is unquoted (e.g., `key=value`)
    UnquotedValue = 1,
    /// Attribute value is double-quoted (e.g., `key="value"`)
    DoubleQuoted = 2,
    /// Attribute value is single-quoted (e.g., `key='value'`)
    SingleQuoted = 3,
}

/// A compact attribute entry for the fused tape builder.
///
/// This 14-byte struct stores pre-tokenized attribute information
/// for cache-friendly access during DOM construction.
///
/// Layout:
/// - key_offset (u32): byte offset of attribute key in source
/// - key_length (u16): length of attribute key in bytes
/// - value_offset (u32): byte offset of attribute value in source (0 if no value)
/// - value_length (u16): length of attribute value in bytes (0 if no value)
/// - flags (u8): AttrFlags indicating value quoting style
/// - _padding (u8): reserved for future use
///
/// Total: 14 bytes per attribute for cache density (target was 16, achieved 14)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, packed)]
pub struct CompactAttrEntry {
    /// Byte offset of attribute key in the source HTML
    pub key_offset: u32,
    /// Byte offset of attribute value in the source HTML (0 if no value)
    pub value_offset: u32,
    /// Length of attribute key in bytes
    pub key_length: u16,
    /// Length of attribute value in bytes (0 if no value)
    pub value_length: u16,
    /// Flags indicating value quoting style
    pub flags: u8,
    /// Reserved for future use (alignment padding)
    pub _padding: u8,
}

impl CompactAttrEntry {
    /// Create a new attribute entry with no value
    #[inline]
    pub fn new_bool(key_offset: u32, key_length: u16) -> Self {
        Self {
            key_offset,
            key_length,
            value_offset: 0,
            value_length: 0,
            flags: AttrFlags::NoValue as u8,
            _padding: 0,
        }
    }

    /// Create a new attribute entry with an unquoted value
    #[inline]
    pub fn new_unquoted(key_offset: u32, key_length: u16, value_offset: u32, value_length: u16) -> Self {
        Self {
            key_offset,
            key_length,
            value_offset,
            value_length,
            flags: AttrFlags::UnquotedValue as u8,
            _padding: 0,
        }
    }

    /// Create a new attribute entry with a double-quoted value
    #[inline]
    pub fn new_double_quoted(key_offset: u32, key_length: u16, value_offset: u32, value_length: u16) -> Self {
        Self {
            key_offset,
            key_length,
            value_offset,
            value_length,
            flags: AttrFlags::DoubleQuoted as u8,
            _padding: 0,
        }
    }

    /// Create a new attribute entry with a single-quoted value
    #[inline]
    pub fn new_single_quoted(key_offset: u32, key_length: u16, value_offset: u32, value_length: u16) -> Self {
        Self {
            key_offset,
            key_length,
            value_offset,
            value_length,
            flags: AttrFlags::SingleQuoted as u8,
            _padding: 0,
        }
    }

    /// Check if this attribute has a value
    #[inline]
    pub fn has_value(&self) -> bool {
        self.flags != AttrFlags::NoValue as u8
    }

    /// Get the attribute key as a string slice from the source
    #[inline]
    pub fn key<'a>(&self, source: &'a [u8]) -> &'a str {
        let start = self.key_offset as usize;
        let end = start + self.key_length as usize;
        unsafe { std::str::from_utf8_unchecked(&source[start..end]) }
    }

    /// Get the attribute value as a string slice from the source (if it has one)
    #[inline]
    pub fn value<'a>(&self, source: &'a [u8]) -> Option<&'a str> {
        if self.has_value() {
            let start = self.value_offset as usize;
            let end = start + self.value_length as usize;
            Some(unsafe { std::str::from_utf8_unchecked(&source[start..end]) })
        } else {
            None
        }
    }

    /// Get the byte range for the key
    #[inline]
    pub fn key_range(&self) -> std::ops::Range<usize> {
        let start = self.key_offset as usize;
        start..start + self.key_length as usize
    }

    /// Get the byte range for the value (if it has one)
    #[inline]
    pub fn value_range(&self) -> Option<std::ops::Range<usize>> {
        if self.has_value() {
            let start = self.value_offset as usize;
            Some(start..start + self.value_length as usize)
        } else {
            None
        }
    }
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
    fn test_compact_attr_entry_size() {
        // Verify CompactAttrEntry is 14 bytes (under the 16-byte target)
        assert_eq!(std::mem::size_of::<CompactAttrEntry>(), 14);
    }

    #[test]
    fn test_compact_attr_entry_bool() {
        let entry = CompactAttrEntry::new_bool(10, 8);
        // Copy fields to avoid issues with packed struct references
        let key_offset = entry.key_offset;
        let key_length = entry.key_length;
        let value_offset = entry.value_offset;
        let value_length = entry.value_length;
        let flags = entry.flags;
        assert_eq!(key_offset, 10);
        assert_eq!(key_length, 8);
        assert_eq!(value_offset, 0);
        assert_eq!(value_length, 0);
        assert_eq!(flags, AttrFlags::NoValue as u8);
        assert!(!entry.has_value());
    }

    #[test]
    fn test_compact_attr_entry_unquoted() {
        let entry = CompactAttrEntry::new_unquoted(10, 5, 16, 6);
        // Copy fields to avoid issues with packed struct references
        let key_offset = entry.key_offset;
        let key_length = entry.key_length;
        let value_offset = entry.value_offset;
        let value_length = entry.value_length;
        let flags = entry.flags;
        assert_eq!(key_offset, 10);
        assert_eq!(key_length, 5);
        assert_eq!(value_offset, 16);
        assert_eq!(value_length, 6);
        assert_eq!(flags, AttrFlags::UnquotedValue as u8);
        assert!(entry.has_value());
    }

    #[test]
    fn test_compact_attr_entry_double_quoted() {
        let entry = CompactAttrEntry::new_double_quoted(10, 5, 17, 4);
        let flags = entry.flags;
        assert_eq!(flags, AttrFlags::DoubleQuoted as u8);
        assert!(entry.has_value());
    }

    #[test]
    fn test_compact_attr_entry_single_quoted() {
        let entry = CompactAttrEntry::new_single_quoted(10, 5, 17, 4);
        let flags = entry.flags;
        assert_eq!(flags, AttrFlags::SingleQuoted as u8);
        assert!(entry.has_value());
    }

    #[test]
    fn test_compact_attr_entry_key_value_slices() {
        let source = b"<div class='test' id='main'>content</div>";
        // class key: offset=5, length=5
        let entry = CompactAttrEntry::new_single_quoted(5, 5, 12, 4);
        assert_eq!(entry.key(source), "class");
        assert_eq!(entry.value(source), Some("test"));
    }

    #[test]
    fn test_compact_attr_entry_bool_key_slice() {
        let source = b"<input disabled checked>";
        // disabled: offset=7, length=8
        let entry = CompactAttrEntry::new_bool(7, 8);
        assert_eq!(entry.key(source), "disabled");
        assert_eq!(entry.value(source), None);
    }

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
