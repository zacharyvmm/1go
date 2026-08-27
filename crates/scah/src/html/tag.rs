//! Cached tag properties for the HTML parser and normalized-text extraction.
//!
//! Parser behavior and normalized-text behavior are classified separately so
//! no-text parses pay only for parser semantics. When normalized text is
//! requested, [`ClassifiedTag::classify`] returns both sets from a single
//! name lookup.

/// Cached, overlapping properties of an HTML tag required by the parser.
///
/// This is a bit set rather than a single tag enum because one tag can have
/// several independent parser behaviors. For example, `table` closes an open
/// paragraph and is also a barrier for multiple scope kinds, while `hr` is
/// both void and paragraph-closing. Keeping those properties together lets us
/// classify a tag name once, store the result on the open-element stack, and
/// answer hot-path membership and scope queries with integer mask tests rather
/// than repeatedly matching or comparing the tag name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TagFlags(u32);

/// Normalized-text semantics for a tag name.
///
/// Only consulted when the parse captures normalized `text`. Kept as a
/// separate bitset so the parser-only classifier can stay lean.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextTagFlags(u16);

/// Combined parser + text classification from a single name lookup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClassifiedTag {
    pub parser: TagFlags,
    pub text: TextTagFlags,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    Default,
    ListItem,
    Button,
    Table,
    Select,
}

impl TagFlags {
    const VOID: u32 = 1 << 0;
    const CLOSES_P: u32 = 1 << 1;
    const P: u32 = 1 << 2;
    const BUTTON: u32 = 1 << 3;
    const LI: u32 = 1 << 4;
    const DT_DD: u32 = 1 << 5;
    const OPTION: u32 = 1 << 6;
    const OPTGROUP: u32 = 1 << 7;
    const TR: u32 = 1 << 8;
    const CELL: u32 = 1 << 9;
    const TABLE_SCOPE: u32 = 1 << 10;
    const DEFAULT_BARRIER: u32 = 1 << 11;
    const LIST_BARRIER: u32 = 1 << 12;
    const TABLE_BARRIER: u32 = 1 << 13;
    const HTML_TEMPLATE: u32 = 1 << 14;
    const RAW_SCRIPT: u32 = 1 << 15;
    const RAW_STYLE: u32 = 1 << 16;
    const RAW_TEXTAREA: u32 = 1 << 17;
    const RAW_TITLE: u32 = 1 << 18;

    #[inline]
    pub(crate) const fn bits(self) -> u32 {
        self.0
    }

    #[inline]
    pub(crate) const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub(crate) const P_MASK: Self = Self(Self::P);
    pub(crate) const BUTTON_MASK: Self = Self(Self::BUTTON);
    pub(crate) const LI_MASK: Self = Self(Self::LI);
    pub(crate) const DT_DD_MASK: Self = Self(Self::DT_DD);
    pub(crate) const OPTION_MASK: Self = Self(Self::OPTION);
    pub(crate) const OPTGROUP_MASK: Self = Self(Self::OPTGROUP);
    pub(crate) const TR_MASK: Self = Self(Self::TR);
    pub(crate) const CELL_MASK: Self = Self(Self::CELL);

    /// Parser-only classification. Prefer this on no-text paths.
    #[inline]
    pub fn classify(name: &str) -> Self {
        let flags = Self::classify_lowercase(name);
        if flags.0 != 0 || !name.as_bytes().iter().any(u8::is_ascii_uppercase) {
            return flags;
        }

        // Known HTML names in this classifier are at most ten bytes long.
        // Mixed-case names are uncommon, so normalize only this fallback and
        // feed it through the same exact-match table as lowercase markup.
        if name.len() > 10 {
            return Self::default();
        }

        let mut lowercase = [0_u8; 10];
        for (output, input) in lowercase.iter_mut().zip(name.bytes()) {
            *output = input.to_ascii_lowercase();
        }

        // ASCII case folding preserves UTF-8 validity: non-ASCII bytes are
        // copied unchanged, while ASCII uppercase bytes remain single-byte.
        let lowercase = unsafe { std::str::from_utf8_unchecked(&lowercase[..name.len()]) };
        Self::classify_lowercase(lowercase)
    }

    #[inline]
    fn classify_lowercase(name: &str) -> Self {
        // Intentionally mirrors main @ 30750d8: parser semantics only.
        let flags = match name {
            "area" | "base" | "br" | "col" | "embed" | "img" | "input" | "link" | "meta"
            | "param" | "source" | "track" | "wbr" => Self::VOID,
            "hr" => Self::VOID | Self::CLOSES_P,
            "address" | "article" | "aside" | "blockquote" | "div" | "dl" | "fieldset"
            | "footer" | "form" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "header" | "main"
            | "nav" | "pre" | "section" => Self::CLOSES_P,
            "p" => Self::CLOSES_P | Self::P,
            "ol" | "ul" => Self::CLOSES_P | Self::LIST_BARRIER,
            "table" => Self::CLOSES_P | Self::DEFAULT_BARRIER | Self::TABLE_BARRIER,
            "button" => Self::BUTTON,
            "li" => Self::LI,
            "dt" | "dd" => Self::DT_DD,
            "option" => Self::OPTION,
            "optgroup" => Self::OPTGROUP,
            "tr" => Self::TR | Self::TABLE_SCOPE,
            "td" | "th" => Self::CELL | Self::DEFAULT_BARRIER,
            "thead" | "tbody" | "tfoot" | "caption" | "colgroup" => Self::TABLE_SCOPE,
            "applet" | "marquee" | "object" => Self::DEFAULT_BARRIER,
            "html" | "template" => Self::HTML_TEMPLATE | Self::TABLE_BARRIER,
            "script" => Self::RAW_SCRIPT,
            "style" => Self::RAW_STYLE,
            "textarea" => Self::RAW_TEXTAREA,
            "title" => Self::RAW_TITLE,
            _ => 0,
        };
        Self(flags)
    }

    #[inline]
    pub(crate) const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    #[inline]
    pub const fn is_void(self) -> bool {
        self.0 & Self::VOID != 0
    }

    #[inline]
    pub const fn closes_open_p(self) -> bool {
        self.0 & Self::CLOSES_P != 0
    }

    #[inline]
    pub(crate) const fn can_trigger_implied_close(self) -> bool {
        self.0
            & (Self::CLOSES_P
                | Self::BUTTON
                | Self::LI
                | Self::DT_DD
                | Self::OPTION
                | Self::OPTGROUP
                | Self::TR
                | Self::CELL)
            != 0
    }

    #[inline]
    pub(crate) const fn close_scope(self) -> ScopeKind {
        if self.0 & (Self::LI | Self::DT_DD) != 0 {
            ScopeKind::ListItem
        } else if self.0 & Self::BUTTON != 0 {
            ScopeKind::Button
        } else if self.0 & (Self::TR | Self::CELL | Self::TABLE_SCOPE) != 0 {
            ScopeKind::Table
        } else if self.0 & (Self::OPTION | Self::OPTGROUP) != 0 {
            ScopeKind::Select
        } else {
            ScopeKind::Default
        }
    }

    #[inline]
    pub const fn is_scope_barrier(self, scope: ScopeKind) -> bool {
        if self.0 & Self::HTML_TEMPLATE != 0 {
            return true;
        }

        match scope {
            ScopeKind::Default => self.0 & Self::DEFAULT_BARRIER != 0,
            ScopeKind::ListItem => self.0 & (Self::DEFAULT_BARRIER | Self::LIST_BARRIER) != 0,
            ScopeKind::Button => self.0 & (Self::DEFAULT_BARRIER | Self::BUTTON) != 0,
            ScopeKind::Table => self.0 & Self::TABLE_BARRIER != 0,
            ScopeKind::Select => self.0 & (Self::OPTION | Self::OPTGROUP) == 0,
        }
    }

    #[inline]
    pub(crate) const fn raw_text_close_tag(self) -> Option<&'static str> {
        if self.0 & Self::RAW_SCRIPT != 0 {
            Some("</script")
        } else if self.0 & Self::RAW_STYLE != 0 {
            Some("</style")
        } else if self.0 & Self::RAW_TEXTAREA != 0 {
            Some("</textarea")
        } else if self.0 & Self::RAW_TITLE != 0 {
            Some("</title")
        } else {
            None
        }
    }
}

#[allow(dead_code)]
impl TextTagFlags {
    const BLOCK: u16 = 1 << 0;
    const BREAK: u16 = 1 << 1;
    const ROW: u16 = 1 << 2;
    const CELL: u16 = 1 << 3;
    const SUPPRESSED: u16 = 1 << 4;
    const PREFORMATTED: u16 = 1 << 5;

    /// Text-only classification. Call only when normalized text is requested.
    #[inline]
    pub fn classify(name: &str) -> Self {
        let flags = Self::classify_lowercase(name);
        if flags.0 != 0 || !name.as_bytes().iter().any(u8::is_ascii_uppercase) {
            return flags;
        }
        if name.len() > 10 {
            return Self::default();
        }
        let mut lowercase = [0_u8; 10];
        for (output, input) in lowercase.iter_mut().zip(name.bytes()) {
            *output = input.to_ascii_lowercase();
        }
        let lowercase = unsafe { std::str::from_utf8_unchecked(&lowercase[..name.len()]) };
        Self::classify_lowercase(lowercase)
    }

    #[inline]
    fn classify_lowercase(name: &str) -> Self {
        let flags = match name {
            "br" | "hr" => Self::BREAK,
            "address" | "article" | "aside" | "blockquote" | "div" | "dl" | "fieldset"
            | "footer" | "form" | "header" | "main" | "nav" | "section" | "h1" | "h2" | "h3"
            | "h4" | "h5" | "h6" | "p" | "ol" | "ul" | "table" | "li" | "dt" | "dd" | "thead"
            | "tbody" | "tfoot" | "colgroup" | "caption" | "body" | "details" | "dialog"
            | "figcaption" | "figure" | "hgroup" | "legend" | "menu" | "summary" => Self::BLOCK,
            "tr" => Self::ROW | Self::BLOCK,
            "td" | "th" => Self::CELL,
            "template" | "script" | "style" => Self::SUPPRESSED,
            "textarea" => Self::PREFORMATTED,
            "pre" => Self::BLOCK | Self::PREFORMATTED,
            _ => 0,
        };
        Self(flags)
    }

    #[inline]
    pub(crate) const fn is_block(self) -> bool {
        self.0 & Self::BLOCK != 0
    }

    #[inline]
    pub(crate) const fn is_break(self) -> bool {
        self.0 & Self::BREAK != 0
    }

    #[inline]
    pub(crate) const fn is_row(self) -> bool {
        self.0 & Self::ROW != 0
    }

    #[inline]
    pub(crate) const fn is_cell(self) -> bool {
        self.0 & Self::CELL != 0
    }

    #[inline]
    pub(crate) const fn is_suppressed(self) -> bool {
        self.0 & Self::SUPPRESSED != 0
    }

    #[inline]
    pub(crate) const fn is_preformatted(self) -> bool {
        self.0 & Self::PREFORMATTED != 0
    }

    /// Separator queued after this element's content in normalized text.
    #[inline]
    #[allow(dead_code)] // mirrored on TextElementFlags for the open-stack close path
    pub(crate) fn post_text_separator(self) -> Option<super::text_state::PendingSeparator> {
        use super::text_state::PendingSeparator;
        if self.is_break() || self.is_block() || self.is_row() {
            Some(PendingSeparator::LineBreak)
        } else if self.is_cell() {
            Some(PendingSeparator::Tab)
        } else {
            None
        }
    }
}

impl ClassifiedTag {
    /// Single lookup returning both parser and text flags.
    ///
    /// Use this on normalized-text paths so the tag name is matched once.
    #[inline]
    pub fn classify(name: &str) -> Self {
        let classified = Self::classify_lowercase(name);
        if classified.parser.0 != 0
            || classified.text.0 != 0
            || !name.as_bytes().iter().any(u8::is_ascii_uppercase)
        {
            return classified;
        }
        if name.len() > 10 {
            return Self::default();
        }
        let mut lowercase = [0_u8; 10];
        for (output, input) in lowercase.iter_mut().zip(name.bytes()) {
            *output = input.to_ascii_lowercase();
        }
        let lowercase = unsafe { std::str::from_utf8_unchecked(&lowercase[..name.len()]) };
        Self::classify_lowercase(lowercase)
    }

    #[inline]
    fn classify_lowercase(name: &str) -> Self {
        let (parser, text) = match name {
            "area" | "base" | "col" | "embed" | "img" | "input" | "link" | "meta" | "param"
            | "source" | "track" | "wbr" => (TagFlags::VOID, 0),
            "br" => (TagFlags::VOID, TextTagFlags::BREAK),
            "hr" => (TagFlags::VOID | TagFlags::CLOSES_P, TextTagFlags::BREAK),
            "address" | "article" | "aside" | "blockquote" | "div" | "dl" | "fieldset"
            | "footer" | "form" | "header" | "main" | "nav" | "section" => {
                (TagFlags::CLOSES_P, TextTagFlags::BLOCK)
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => (TagFlags::CLOSES_P, TextTagFlags::BLOCK),
            "p" => (TagFlags::CLOSES_P | TagFlags::P, TextTagFlags::BLOCK),
            "ol" | "ul" => (
                TagFlags::CLOSES_P | TagFlags::LIST_BARRIER,
                TextTagFlags::BLOCK,
            ),
            "table" => (
                TagFlags::CLOSES_P | TagFlags::DEFAULT_BARRIER | TagFlags::TABLE_BARRIER,
                TextTagFlags::BLOCK,
            ),
            "button" => (TagFlags::BUTTON, 0),
            "li" => (TagFlags::LI, TextTagFlags::BLOCK),
            "dt" | "dd" => (TagFlags::DT_DD, TextTagFlags::BLOCK),
            "option" => (TagFlags::OPTION, 0),
            "optgroup" => (TagFlags::OPTGROUP, 0),
            "tr" => (
                TagFlags::TR | TagFlags::TABLE_SCOPE,
                TextTagFlags::ROW | TextTagFlags::BLOCK,
            ),
            "td" | "th" => (
                TagFlags::CELL | TagFlags::DEFAULT_BARRIER,
                TextTagFlags::CELL,
            ),
            "thead" | "tbody" | "tfoot" | "colgroup" => {
                (TagFlags::TABLE_SCOPE, TextTagFlags::BLOCK)
            }
            "caption" => (TagFlags::TABLE_SCOPE, TextTagFlags::BLOCK),
            "applet" | "marquee" | "object" => (TagFlags::DEFAULT_BARRIER, 0),
            "html" => (TagFlags::HTML_TEMPLATE | TagFlags::TABLE_BARRIER, 0),
            "template" => (
                TagFlags::HTML_TEMPLATE | TagFlags::TABLE_BARRIER,
                TextTagFlags::SUPPRESSED,
            ),
            "script" => (TagFlags::RAW_SCRIPT, TextTagFlags::SUPPRESSED),
            "style" => (TagFlags::RAW_STYLE, TextTagFlags::SUPPRESSED),
            "textarea" => (TagFlags::RAW_TEXTAREA, TextTagFlags::PREFORMATTED),
            "title" => (TagFlags::RAW_TITLE, 0),
            "pre" => (
                TagFlags::CLOSES_P,
                TextTagFlags::BLOCK | TextTagFlags::PREFORMATTED,
            ),
            "body" | "details" | "dialog" | "figcaption" | "figure" | "hgroup" | "legend"
            | "menu" | "summary" => (0, TextTagFlags::BLOCK),
            _ => (0, 0),
        };
        Self {
            parser: TagFlags(parser),
            text: TextTagFlags(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ClassifiedTag, TagFlags, TextTagFlags};

    #[test]
    fn mixed_case_names_have_the_same_classification() {
        for (lowercase, mixed_case) in [
            ("div", "DiV"),
            ("hr", "HR"),
            ("button", "BuTtOn"),
            ("optgroup", "OPTGROUP"),
            ("colgroup", "ColGroup"),
            ("template", "TEMPLATE"),
            ("textarea", "TextArea"),
        ] {
            assert_eq!(
                TagFlags::classify(lowercase),
                TagFlags::classify(mixed_case)
            );
            assert_eq!(
                TextTagFlags::classify(lowercase),
                TextTagFlags::classify(mixed_case)
            );
            assert_eq!(
                ClassifiedTag::classify(lowercase),
                ClassifiedTag::classify(mixed_case)
            );
        }
    }

    #[test]
    fn unknown_names_have_no_flags() {
        assert_eq!(TagFlags::classify("custom-element"), TagFlags::default());
        assert_eq!(TagFlags::classify("CUSTOM-ELEMENT"), TagFlags::default());
        assert_eq!(
            TextTagFlags::classify("custom-element"),
            TextTagFlags::default()
        );
        assert_eq!(
            ClassifiedTag::classify("custom-element"),
            ClassifiedTag::default()
        );
    }

    #[test]
    fn combined_matches_split_classifiers() {
        for name in [
            "div",
            "p",
            "br",
            "hr",
            "pre",
            "td",
            "tr",
            "script",
            "style",
            "template",
            "textarea",
            "body",
            "span",
            "custom-element",
            "TABLE",
        ] {
            let combined = ClassifiedTag::classify(name);
            assert_eq!(
                combined.parser,
                TagFlags::classify(name),
                "parser for {name}"
            );
            assert_eq!(
                combined.text,
                TextTagFlags::classify(name),
                "text for {name}"
            );
        }
    }

    #[test]
    fn text_flags_cover_normalized_semantics() {
        assert!(TextTagFlags::classify("div").is_block());
        assert!(TextTagFlags::classify("br").is_break());
        assert!(TextTagFlags::classify("tr").is_row());
        assert!(TextTagFlags::classify("td").is_cell());
        assert!(TextTagFlags::classify("script").is_suppressed());
        assert!(TextTagFlags::classify("pre").is_preformatted());
        assert!(TextTagFlags::classify("pre").is_block());
        assert!(TextTagFlags::classify("textarea").is_preformatted());
        assert!(!TextTagFlags::classify("span").is_block());
    }

    #[test]
    fn parser_flags_match_historical_main_semantics() {
        assert!(TagFlags::classify("br").is_void());
        assert!(TagFlags::classify("hr").is_void());
        assert!(TagFlags::classify("hr").closes_open_p());
        assert!(TagFlags::classify("pre").closes_open_p());
        assert!(!TagFlags::classify("body").closes_open_p());
        assert!(!TagFlags::classify("body").is_void());
    }

    #[test]
    fn only_implied_close_openers_enter_stack_preparation() {
        for name in ["p", "div", "li", "button", "option", "tr", "td"] {
            assert!(
                TagFlags::classify(name).can_trigger_implied_close(),
                "{name}"
            );
        }
        for name in ["a", "span", "img", "script", "custom-element"] {
            assert!(
                !TagFlags::classify(name).can_trigger_implied_close(),
                "{name}"
            );
        }
    }
}
