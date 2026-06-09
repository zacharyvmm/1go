//! TagFilter: query-compiler-driven lazy parsing optimization.
//!
//! For a query like `div.product > a[href]`, the compiler knows that only `<a>`
//! and `<div>` tags can possibly match. All other tags can be skipped without
//! attribute tokenization. This pre-computed filter tells the parser which tag
//! names are worth the cost of full parsing.
//!
//! Conservative by design: false positives (tag is in the set but doesn't match)
//! are fine — they just reduce the optimization's benefit. False negatives
//! (tag could match but is NOT in the set) are correctness bugs.

use crate::query::compiler::QuerySpec;

/// Maximum number of unique tag names before the optimization is considered
/// not worth the filtering overhead. Above this threshold we fall back to
/// full tokenization (no filter).
const MAX_INTERESTING_TAGS: usize = 20;

/// A pre-computed set of tag names that could possibly match any active query.
///
/// `None` means the optimization is unavailable — all tags must be fully
/// tokenized (e.g., a universal `*` selector is present).
#[derive(Debug, Clone)]
pub struct TagFilter {
    /// Tags that COULD match any active query.
    /// `None` = no filter possible (all tags must be tokenized).
    interesting_tags: Option<Vec<String>>,
}

impl TagFilter {
    /// Returns `true` if a tag with this name could possibly match any
    /// active query.
    ///
    /// Conservative: false positives are acceptable, false negatives are bugs.
    #[inline]
    pub fn could_match(&self, tag_name: &str) -> bool {
        match &self.interesting_tags {
            None => true, // No filter — all tags could match
            Some(tags) => tags.iter().any(|t| t.eq_ignore_ascii_case(tag_name)),
        }
    }

    /// Returns `true` if the filter is active (optimization is available).
    #[inline]
    pub fn is_active(&self) -> bool {
        self.interesting_tags.is_some()
    }

    /// Build a `TagFilter` from a set of compiled queries.
    ///
    /// Returns `None` if no optimization is possible:
    /// - Any query uses a universal selector (`*` or no tag name predicate)
    /// - Too many distinct tag names (>`MAX_INTERESTING_TAGS`)
    pub fn from_queries<'query, Q: QuerySpec<'query>>(queries: &'query [Q]) -> Option<Self> {
        let mut interesting_tags: Vec<String> = Vec::new();

        for query in queries {
            Self::collect_tag_names(query, &mut interesting_tags)?;
        }

        // Sort and deduplicate (case-insensitive)
        interesting_tags.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        interesting_tags.dedup_by(|a, b| a.eq_ignore_ascii_case(b));

        // If too many tags, optimization overhead isn't worth it
        if interesting_tags.len() > MAX_INTERESTING_TAGS {
            return None;
        }

        Some(Self {
            interesting_tags: Some(interesting_tags),
        })
    }

    /// Walk all transitions in a query and extract tag names from predicates.
    ///
    /// Returns `Err(())` if the optimization is impossible (universal selector
    /// or no tag name), `Ok(())` if names were collected successfully.
    fn collect_tag_names<'query, Q: QuerySpec<'query>>(
        query: &'query Q,
        tags: &mut Vec<String>,
    ) -> Option<()> {
        let states = query.states();

        // Note: states are shared across sections; each section has a range
        // into the states array. We walk all states to find tag names.
        for state in states {
            let tag_name = match state.predicate.name {
                Some(name) => name,
                None => return None, // Universal selector or no tag name → bail
            };

            // Empty tag name shouldn't happen but be safe
            if tag_name.is_empty() {
                return None;
            }

            // Skip duplicates within this query
            if !tags.iter().any(|t| t.eq_ignore_ascii_case(tag_name)) {
                tags.push(tag_name.to_string());
            }
        }

        // Sanity: if no tag names were found, something is wrong
        if tags.is_empty() {
            return None;
        }

        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Query, Save};

    #[test]
    fn test_single_tag_query() {
        let queries = &[Query::all("a", Save::all()).unwrap().build()];
        let filter = TagFilter::from_queries(queries).unwrap();
        assert!(filter.is_active());
        assert!(filter.could_match("a"));
        assert!(filter.could_match("A")); // case-insensitive
        assert!(!filter.could_match("div"));
        assert!(!filter.could_match("span"));
    }

    #[test]
    fn test_attribute_selector() {
        // a[href] still has tag name "a"
        let queries = &[Query::all("a[href]", Save::all()).unwrap().build()];
        let filter = TagFilter::from_queries(queries).unwrap();
        assert!(filter.is_active());
        assert!(filter.could_match("a"));
        assert!(!filter.could_match("div"));
    }

    #[test]
    fn test_child_combinator() {
        // main > section > a — all three tag names are interesting
        let queries = &[Query::all("main > section > a[href]", Save::all())
            .unwrap()
            .build()];
        let filter = TagFilter::from_queries(queries).unwrap();
        assert!(filter.is_active());
        assert!(filter.could_match("main"));
        assert!(filter.could_match("section"));
        assert!(filter.could_match("a"));
        assert!(!filter.could_match("div"));
    }

    #[test]
    fn test_descendant_combinator() {
        // div span a — all three tag names are interesting
        let queries = &[Query::all("div span a", Save::all()).unwrap().build()];
        let filter = TagFilter::from_queries(queries).unwrap();
        assert!(filter.is_active());
        assert!(filter.could_match("div"));
        assert!(filter.could_match("span"));
        assert!(filter.could_match("a"));
    }

    #[test]
    fn test_no_tag_name_predicate_returns_none() {
        // .class-only selector has no tag name predicate (name: None)
        let queries = &[Query::all(".foo", Save::all()).unwrap().build()];
        let filter = TagFilter::from_queries(queries);
        assert!(filter.is_none());
    }

    #[test]
    fn test_multiple_queries_union_of_names() {
        let queries = &[
            Query::all("a", Save::all()).unwrap().build(),
            Query::all("div", Save::all()).unwrap().build(),
            Query::all("span", Save::all()).unwrap().build(),
        ];
        let filter = TagFilter::from_queries(queries).unwrap();
        assert!(filter.is_active());
        assert!(filter.could_match("a"));
        assert!(filter.could_match("div"));
        assert!(filter.could_match("span"));
        assert!(!filter.could_match("p"));
    }

    #[test]
    fn test_class_with_tag_name() {
        // div.product — has tag name "div"
        let queries = &[Query::all("div.product", Save::all()).unwrap().build()];
        let filter = TagFilter::from_queries(queries).unwrap();
        assert!(filter.is_active());
        assert!(filter.could_match("div"));
        assert!(!filter.could_match("span"));
    }

    #[test]
    fn test_id_with_tag_name() {
        // a#link — has tag name "a"
        let queries = &[Query::all("a#link", Save::all()).unwrap().build()];
        let filter = TagFilter::from_queries(queries).unwrap();
        assert!(filter.is_active());
        assert!(filter.could_match("a"));
    }

    #[test]
    fn test_attribute_only_returns_none() {
        // [href] — no tag name predicate
        let queries = &[Query::all("[href]", Save::all()).unwrap().build()];
        let filter = TagFilter::from_queries(queries);
        assert!(filter.is_none());
    }
}
