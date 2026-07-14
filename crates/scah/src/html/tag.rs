/// Cached, overlapping properties of an HTML tag.
///
/// This is a bit set rather than a single tag enum because one tag can have
/// several independent parser behaviors. For example, `table` closes an open
/// paragraph and is also a barrier for multiple scope kinds, while `hr` is
/// both void and paragraph-closing. Keeping those properties together lets us
/// classify a tag name once, store the result on the open-element stack, and
/// answer hot-path membership and scope queries with integer mask tests rather
/// than repeatedly matching or comparing the tag name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TagFlags(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScopeKind {
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

    pub(crate) const P_MASK: Self = Self(Self::P);
    pub(crate) const BUTTON_MASK: Self = Self(Self::BUTTON);
    pub(crate) const LI_MASK: Self = Self(Self::LI);
    pub(crate) const DT_DD_MASK: Self = Self(Self::DT_DD);
    pub(crate) const OPTION_MASK: Self = Self(Self::OPTION);
    pub(crate) const OPTGROUP_MASK: Self = Self(Self::OPTGROUP);
    pub(crate) const TR_MASK: Self = Self(Self::TR);
    pub(crate) const CELL_MASK: Self = Self(Self::CELL);

    #[inline]
    pub(crate) fn classify(name: &str) -> Self {
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
    pub(crate) const fn is_void(self) -> bool {
        self.0 & Self::VOID != 0
    }

    #[inline]
    pub(crate) const fn closes_open_p(self) -> bool {
        self.0 & Self::CLOSES_P != 0
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
    pub(crate) const fn is_scope_barrier(self, scope: ScopeKind) -> bool {
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

#[cfg(test)]
mod tests {
    use super::TagFlags;

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
        }
    }

    #[test]
    fn unknown_names_have_no_flags() {
        assert_eq!(TagFlags::classify("custom-element"), TagFlags::default());
        assert_eq!(TagFlags::classify("CUSTOM-ELEMENT"), TagFlags::default());
    }
}
