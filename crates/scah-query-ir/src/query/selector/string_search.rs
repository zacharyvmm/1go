#[derive(Debug, PartialEq, Clone)]
pub enum AttributeSelectionKind {
    Presence,            // [attribute]
    Exact,               // [attribute=value]
    WhitespaceSeparated, // [attribute~=value]
    HyphenSeparated,     // [attribute|=value]
    Prefix,              // [attribute^=value]
    Suffix,              // [attribute$=value]
    Substring,           // [attribute*=value]
}

impl AttributeSelectionKind {
    pub fn find(&self, query: &str, source: &str) -> bool {
        match self {
            Self::Exact => query == source,
            Self::Presence => true,
            Self::WhitespaceSeparated => source.split_whitespace().any(|word| word == query),
            Self::HyphenSeparated => {
                source == query
                    || source
                        .strip_prefix(query)
                        .is_some_and(|rest| rest.starts_with('-'))
            }
            Self::Prefix => source.starts_with(query),
            Self::Suffix => source.ends_with(query),
            Self::Substring => source.contains(query),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_presence() {
        let kind = AttributeSelectionKind::Presence;
        assert!(kind.find("", "Hello"));
    }

    #[test]
    fn test_exact() {
        let kind = AttributeSelectionKind::Exact;
        assert!(kind.find("Hello", "Hello"));
    }

    #[test]
    fn test_whitespace() {
        let kind = AttributeSelectionKind::WhitespaceSeparated;
        assert!(kind.find("world", "hello world in test"));
    }

    #[test]
    fn test_prefix() {
        let kind = AttributeSelectionKind::Prefix;
        assert!(kind.find("hello wor", "hello world in test"));
    }

    #[test]
    fn test_with_hypen_separated() {
        let kind = AttributeSelectionKind::HyphenSeparated;
        assert!(kind.find("en", "en"));
        assert!(kind.find("en", "en-US"));
        assert!(kind.find("en", "en-us"));
    }

    #[test]
    fn test_without_hypen_separated() {
        let kind = AttributeSelectionKind::HyphenSeparated;
        assert!(!kind.find("en", "xx en-US"));
        assert!(!kind.find("en", "xen-US"));
        assert!(!kind.find("en", "hello en-world"));
        assert!(!kind.find("en", "hello en world"));
    }

    #[test]
    fn test_suffix() {
        let kind = AttributeSelectionKind::Suffix;
        assert!(kind.find("ld in test", "hello world in test"));
    }

    #[test]
    fn test_substring() {
        let kind = AttributeSelectionKind::Substring;
        assert!(kind.find("world", "helloworldintest"));
    }

    #[test]
    fn test_prefix_unicode_no_panic() {
        let kind = AttributeSelectionKind::Prefix;
        assert!(!kind.find("e", "éclair"));
    }

    #[test]
    fn test_suffix_unicode_no_panic() {
        let kind = AttributeSelectionKind::Suffix;
        assert!(!kind.find("e", "café"));
    }

    #[test]
    fn test_hyphen_separated_unicode_no_panic() {
        let kind = AttributeSelectionKind::HyphenSeparated;
        assert!(!kind.find("e", "é-fr"));
    }
}
