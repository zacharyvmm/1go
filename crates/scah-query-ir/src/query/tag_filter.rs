//! TagFilter: query-compiler-driven lazy parsing optimization.
//!
//! For a query like `div.product > a[href]`, the compiler knows that only `<a>`
//! and `<div>` tags can possibly match. All other tags can be skipped without
//! attribute tokenization. This pre-computed filter tells the parser which tag
//! names are worth the cost of full parsing.
//!
//! ## Context sensitivity
//!
//! For hierarchical selectors, the filter tracks ancestor requirements:
//! - `div > a`: `<a>` only matters when a `<div>` is the direct parent
//! - `div span a`: `<a>` only matters when a `<span>` is somewhere above it
//! - `a` (bare): `<a>` is unconditional (can match anywhere)
//!
//! Conservative by design: false positives (tag is in the set but doesn't match)
//! are fine — they just reduce the optimization's benefit. False negatives
//! (tag could match but is NOT in the set) are correctness bugs.

use crate::query::compiler::QuerySpec;
use crate::query::selector::Combinator;
use std::collections::HashMap;

/// Maximum number of unique tag names before the optimization is considered
/// not worth the filtering overhead. Above this threshold we fall back to
/// full tokenization (no filter).
const MAX_INTERESTING_TAGS: usize = 20;

/// What ancestor context enables a tag to match.
#[derive(Debug, Clone)]
struct AncestorReq {
    /// Tag name of the required ancestor.
    ancestor_tag: String,
    /// If true, the ancestor must be the direct parent (`>` combinator).
    /// If false, the ancestor can be anywhere on the stack (descendant).
    direct_parent: bool,
}

/// A pre-computed set of tag names that could possibly match any active query,
/// along with ancestor context requirements for hierarchical selectors.
///
/// `None` means the optimization is unavailable — all tags must be fully
/// tokenized (e.g., a universal `*` selector is present).
#[derive(Debug, Clone)]
pub struct TagFilter {
    /// Tags that COULD match any active query.
    /// `None` = no filter possible (all tags must be tokenized).
    interesting_tags: Option<Vec<String>>,
    /// Tags that can match regardless of ancestor context (first in a
    /// selector chain or bare tag queries).
    unconditional_tags: Vec<String>,
    /// Per-tag ancestor requirements. A tag can match if ANY of its
    /// requirements are satisfied (OR if it's unconditional).
    ancestor_reqs: HashMap<String, Vec<AncestorReq>>,
}

impl TagFilter {
    /// Returns `true` if a tag with this name could possibly match any
    /// active query, REGARDLESS of context.
    ///
    /// This is the basic check used in Phase 5.
    #[inline]
    pub fn could_match(&self, tag_name: &str) -> bool {
        match &self.interesting_tags {
            None => true,
            Some(tags) => tags.iter().any(|t| t.eq_ignore_ascii_case(tag_name)),
        }
    }

    /// Returns `true` if a tag with this name could possibly match given
    /// the current open element stack.
    ///
    /// This is the context-aware check for Phase 6. It considers:
    /// - Unconditional tags (always true)
    /// - Tags requiring a direct parent (child combinator `>`)
    /// - Tags requiring any ancestor (descendant combinator ` `)
    ///
    /// `open_elements` is a list of open tag names from outermost to innermost.
    /// `top_tag` is the direct parent (top of stack), if any.
    #[inline]
    pub fn could_match_in_context(
        &self,
        tag_name: &str,
        open_elements: &[String],
        top_tag: Option<&str>,
    ) -> bool {
        match &self.interesting_tags {
            None => true,
            Some(_) => {
                // Check if this tag is in the interesting set at all
                if !self.could_match(tag_name) {
                    return false;
                }
                // If unconditional, always match
                if self
                    .unconditional_tags
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(tag_name))
                {
                    return true;
                }
                // Check ancestor requirements
                let reqs = match self.ancestor_reqs.get(&tag_name.to_lowercase()) {
                    Some(r) => r,
                    None => return true, // unconditional
                };
                // Check if any ancestor requirement is satisfied
                reqs.iter().any(|req| {
                    if req.direct_parent {
                        // Child combinator: need exact match on top of stack
                        top_tag.is_some_and(|t| t.eq_ignore_ascii_case(&req.ancestor_tag))
                    } else {
                        // Descendant combinator: need ancestor anywhere on stack
                        open_elements
                            .iter()
                            .any(|t| t.eq_ignore_ascii_case(&req.ancestor_tag))
                    }
                })
            }
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
        let mut unconditional_tags: Vec<String> = Vec::new();
        let mut ancestor_reqs: HashMap<String, Vec<AncestorReq>> = HashMap::new();

        for query in queries {
            Self::collect_tag_info(
                query,
                &mut interesting_tags,
                &mut unconditional_tags,
                &mut ancestor_reqs,
            )?;
        }

        // Sort and deduplicate (case-insensitive)
        interesting_tags.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        interesting_tags.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        unconditional_tags.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        unconditional_tags.dedup_by(|a, b| a.eq_ignore_ascii_case(b));

        if interesting_tags.len() > MAX_INTERESTING_TAGS {
            return None;
        }

        Some(Self {
            interesting_tags: Some(interesting_tags),
            unconditional_tags,
            ancestor_reqs,
        })
    }

    /// Walk all transitions in a query and extract tag names with ancestor
    /// requirements.
    fn collect_tag_info<'query, Q: QuerySpec<'query>>(
        query: &'query Q,
        tags: &mut Vec<String>,
        unconditional_tags: &mut Vec<String>,
        ancestor_reqs: &mut HashMap<String, Vec<AncestorReq>>,
    ) -> Option<()> {
        let states = query.states();

        // Walk states in order. The first state is unconditional; each
        // subsequent state needs its predecessor as ancestor.
        let mut prev_tag: Option<&str> = None;

        for state in states {
            let tag_name = match state.predicate.name {
                Some(name) => name,
                None => return None, // Universal selector or no tag name → bail
            };

            if tag_name.is_empty() {
                return None;
            }

            // Add to interesting set
            if !tags.iter().any(|t| t.eq_ignore_ascii_case(tag_name)) {
                tags.push(tag_name.to_string());
            }

            // First state in the chain is unconditional
            let is_first = prev_tag.is_none();
            if is_first
                && !unconditional_tags
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(tag_name))
            {
                unconditional_tags.push(tag_name.to_string());
            }

            // If this is NOT the first state, add ancestor requirement.
            // The combinator connecting this state to its predecessor is
            // THIS state's guard, not the predecessor's.
            if let Some(prev) = prev_tag {
                let key = tag_name.to_lowercase();
                let req = AncestorReq {
                    ancestor_tag: prev.to_string(),
                    direct_parent: state.guard == Combinator::Child,
                };
                ancestor_reqs.entry(key).or_default().push(req);
            }

            prev_tag = Some(tag_name);
        }

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

    fn make_stack<'a>(names: &'a [&'a str]) -> (Vec<String>, Option<&'a str>) {
        let stack: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        let top = names.last().copied();
        (stack, top)
    }

    #[test]
    fn test_single_tag_unconditional() {
        let queries = &[Query::all("a", Save::all()).unwrap().build()];
        let filter = TagFilter::from_queries(queries).unwrap();
        assert!(filter.is_active());
        assert!(filter.could_match("a"));
        assert!(filter.could_match("A"));
        assert!(!filter.could_match("div"));

        // Context check: unconditional even with no ancestors
        let (stack, top) = make_stack(&[]);
        assert!(filter.could_match_in_context("a", &stack, top));
        assert!(!filter.could_match_in_context("div", &stack, top));
    }

    #[test]
    fn test_child_combinator_context() {
        // main > section > a[href]
        let queries = &[Query::all("main > section > a[href]", Save::all())
            .unwrap()
            .build()];
        let filter = TagFilter::from_queries(queries).unwrap();

        // main is unconditional
        let (stack, top) = make_stack(&[]);
        assert!(filter.could_match_in_context("main", &stack, top));

        // section needs main as direct parent
        let (stack, top) = make_stack(&["main"]);
        assert!(filter.could_match_in_context("section", &stack, top));

        // section should NOT match if main is not the direct parent
        // (e.g., main is on the stack but div is the top)
        let (stack, top) = make_stack(&["main", "div"]);
        assert!(!filter.could_match_in_context("section", &stack, top));

        // a needs section as direct parent
        let (stack, top) = make_stack(&["main", "section"]);
        assert!(filter.could_match_in_context("a", &stack, top));

        // a should NOT match if section is not direct parent
        let (stack, top) = make_stack(&["main"]);
        assert!(!filter.could_match_in_context("a", &stack, top));
    }

    #[test]
    fn test_descendant_combinator_context() {
        // div span a
        let queries = &[Query::all("div span a", Save::all()).unwrap().build()];
        let filter = TagFilter::from_queries(queries).unwrap();

        // div is unconditional
        let (stack, top) = make_stack(&[]);
        assert!(filter.could_match_in_context("div", &stack, top));

        // span needs div somewhere in stack
        let (stack, top) = make_stack(&["div"]);
        assert!(filter.could_match_in_context("span", &stack, top));

        // span should match even with other elements above div
        let (stack, top) = make_stack(&["div", "p"]);
        assert!(filter.could_match_in_context("span", &stack, top));

        // span should NOT match without div
        let (stack, top) = make_stack(&["body"]);
        assert!(!filter.could_match_in_context("span", &stack, top));

        // a needs span somewhere in stack
        let (stack, top) = make_stack(&["div", "span"]);
        assert!(filter.could_match_in_context("a", &stack, top));

        // a needs BOTH div and span — well, only span directly
        let (stack, top) = make_stack(&["span"]);
        assert!(filter.could_match_in_context("a", &stack, top));
    }

    #[test]
    fn test_multiple_queries_different_contexts() {
        // Query 1: a (bare, unconditional)
        // Query 2: div > a (needs div as direct parent)
        let queries = &[
            Query::all("a", Save::all()).unwrap().build(),
            Query::all("div > a", Save::all()).unwrap().build(),
        ];
        let filter = TagFilter::from_queries(queries).unwrap();

        // a should match in empty context (from bare query)
        let (stack, top) = make_stack(&[]);
        assert!(filter.could_match_in_context("a", &stack, top));

        // a should also match with div parent
        let (stack, top) = make_stack(&["div"]);
        assert!(filter.could_match_in_context("a", &stack, top));

        // div is unconditional
        assert!(filter.could_match_in_context("div", &stack, top));
    }

    #[test]
    fn test_context_filter_negative() {
        // div > a — only a inside div
        let queries = &[Query::all("div > a", Save::all()).unwrap().build()];
        let filter = TagFilter::from_queries(queries).unwrap();

        // a inside body is NOT a match
        let (stack, top) = make_stack(&["body"]);
        assert!(!filter.could_match_in_context("a", &stack, top));

        // a inside div IS a match
        let (stack, top) = make_stack(&["div"]);
        assert!(filter.could_match_in_context("a", &stack, top));

        // a inside body > div IS a match (child combinator: div is top)
        let (stack, top) = make_stack(&["body", "div"]);
        assert!(filter.could_match_in_context("a", &stack, top));
    }

    #[test]
    fn test_no_tag_name_predicate_returns_none() {
        let queries = &[Query::all(".foo", Save::all()).unwrap().build()];
        let filter = TagFilter::from_queries(queries);
        assert!(filter.is_none());
    }

    #[test]
    fn test_attribute_only_returns_none() {
        let queries = &[Query::all("[href]", Save::all()).unwrap().build()];
        let filter = TagFilter::from_queries(queries);
        assert!(filter.is_none());
    }

    #[test]
    fn test_multiple_queries_union() {
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
}
