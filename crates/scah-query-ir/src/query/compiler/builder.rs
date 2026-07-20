use super::SelectorParseError;
use super::query::{Query, QuerySection, QuerySectionId, TransitionId};
use super::transition::Transition;

/// Controls which pieces of content to capture for matched elements.
///
/// When an element matches a CSS selector, scah can optionally capture:
/// - **inner HTML** — raw markup between the opening and closing tags
/// - **raw text** — source-preserving descendant text
/// - **text** — normalized, human-readable descendant text
///
/// Use the convenience constructors [`Save::all`], [`Save::none`],
/// [`Save::only_inner_html`], [`Save::only_raw_text`], and [`Save::only_text`]
/// for common configurations.
///
/// # Example
///
/// ```rust
/// use scah_query_ir::Save;
///
/// // Capture everything
/// let save = Save::all();
/// assert!(save.inner_html);
/// assert!(save.raw_text);
/// assert!(save.text);
///
/// // Capture only normalized text (lighter weight)
/// let save = Save::only_text();
/// assert!(!save.inner_html);
/// assert!(!save.raw_text);
/// assert!(save.text);
/// ```
#[derive(PartialEq, Debug, Default, Clone, Copy)]
pub struct Save {
    /// When `true`, the raw HTML between the element's opening and closing
    /// tags is stored as [`Element::inner_html`](crate::Element::inner_html).
    pub inner_html: bool,
    /// When `true`, source-preserving descendant text is stored and
    /// retrievable via [`Element::raw_text()`](crate::Element::raw_text).
    pub raw_text: bool,
    /// When `true`, normalized human-readable descendant text is stored and
    /// retrievable via [`Element::text()`](crate::Element::text).
    ///
    /// This is not layout-aware browser `innerText`.
    pub text: bool,
}

impl Save {
    /// Capture only the raw inner HTML of matched elements.
    pub fn only_inner_html() -> Self {
        Self {
            inner_html: true,
            raw_text: false,
            text: false,
        }
    }

    /// Capture only source-preserving descendant text.
    pub fn only_raw_text() -> Self {
        Self {
            inner_html: false,
            raw_text: true,
            text: false,
        }
    }

    /// Capture only normalized, human-readable descendant text.
    pub fn only_text() -> Self {
        Self {
            inner_html: false,
            raw_text: false,
            text: true,
        }
    }

    /// Capture inner HTML, raw text, and normalized text.
    pub fn all() -> Self {
        Self {
            inner_html: true,
            raw_text: true,
            text: true,
        }
    }

    /// Capture neither inner HTML nor text.
    ///
    /// The matched element's tag name, id, class, and attributes are still
    /// stored; only the heavier content extraction is skipped.
    pub fn none() -> Self {
        Self {
            inner_html: false,
            raw_text: false,
            text: false,
        }
    }
}

/// Whether a query section should match **all** occurrences or only the
/// **first** one.
///
/// Using [`SelectionKind::First`] enables an early-exit optimisation:
/// once the first match is found (and its content captured), the parser
/// can skip the remaining document for that query branch.
#[derive(PartialEq, Debug, Clone, Copy)]
pub enum SelectionKind {
    /// Match every occurrence of the selector.
    All,
    /// Match only the first occurrence, enabling early exit.
    First,
}

/// An in-progress query being assembled via a builder pattern.
///
/// You typically don't construct a `QueryBuilder` directly; instead,
/// start with [`Query::all`](crate::Query::all) or
/// [`Query::first`](crate::Query::first) and then chain further
/// selectors with [`.all()`](QueryBuilder::all),
/// [`.first()`](QueryBuilder::first), and [`.then()`](QueryBuilder::then).
/// Finalise with [`.build()`](QueryBuilder::build) to produce a [`Query`].
///
/// # Example
///
/// ```rust
/// use scah_query_ir::{Query, Save};
///
/// let query = Query::all("main > section", Save::all())?
///     .then(|ctx| Ok([
///         ctx.all("> a[href]", Save::all())?,
///         ctx.all("div a", Save::only_text())?,
///     ]))?
///     .build();
/// # Ok::<(), scah_query_ir::SelectorParseError>(())
/// ```
#[derive(Debug, Clone)]
pub struct QueryBuilder<'query> {
    /// Internal automaton transitions (compiled selector segments).
    pub states: Vec<Transition<'query>>,
    /// Internal ordered list of query sections.
    pub selection: Vec<QuerySection<'query>>,
}

impl<'query> QueryBuilder<'query> {
    pub fn all(mut self, query: &'query str, save: Save) -> Result<Self, SelectorParseError> {
        assert!(!self.selection.is_empty());

        let current_state_len = self.states.len();
        let mut states = Transition::generate_transitions_from_string(query)?;

        let parent_index = QuerySectionId(self.selection.len() - 1);
        let range = TransitionId(current_state_len)..TransitionId(current_state_len + states.len());
        self.selection.push(QuerySection::new(
            query,
            save,
            SelectionKind::All,
            range,
            Some(parent_index),
        ));

        self.states.append(&mut states);

        Ok(self)
    }

    /// Append a child selector that matches **all** occurrences.
    ///
    /// The new selector is scoped to elements that already matched
    /// the previous selector in the chain.
    ///
    pub fn first(mut self, query: &'query str, save: Save) -> Result<Self, SelectorParseError> {
        assert!(!self.selection.is_empty());

        let current_state_len = self.states.len();
        let mut states = Transition::generate_transitions_from_string(query)?;

        let parent_index = QuerySectionId(self.selection.len() - 1);
        let range = TransitionId(current_state_len)..TransitionId(current_state_len + states.len());
        self.selection.push(QuerySection::new(
            query,
            save,
            SelectionKind::First,
            range,
            Some(parent_index),
        ));

        self.states.append(&mut states);

        Ok(self)
    }

    /// Append a child selector that matches only the **first** occurrence.
    ///
    /// Enables early-exit optimisation for this branch of the query tree.
    ///
    pub fn append(&mut self, parent: QuerySectionId, mut other: Self) {
        let state_length = self.states.len();
        let selection_length = self.selection.len();

        let mut last_sibling: Option<QuerySectionId> = {
            if parent.index() + 1 == self.selection.len() {
                None
            } else {
                let mut sibling_index = QuerySectionId(parent.index() + 1);
                while self.selection[sibling_index.index()].next_sibling.is_some() {
                    sibling_index = self.selection[sibling_index.index()].next_sibling.unwrap();
                }

                Some(sibling_index)
            }
        };
        for index in 0..other.selection.len() {
            let query = &mut other.selection[index];
            query.range.start = TransitionId(query.range.start.index() + state_length);
            query.range.end = TransitionId(query.range.end.index() + state_length);
            if let Some(next_sibling) = query.next_sibling {
                query.next_sibling = Some(QuerySectionId(next_sibling.index() + selection_length));
            }

            if let Some(idx) = query.parent {
                query.parent = Some(QuerySectionId(idx.index() + selection_length));
            } else {
                query.parent = Some(parent);

                let current_index = QuerySectionId(selection_length + index);
                last_sibling = match last_sibling {
                    Some(sibling) => {
                        if sibling.index() < selection_length {
                            self.selection[sibling.index()].next_sibling = Some(current_index);
                        } else {
                            other.selection[sibling.index() - selection_length].next_sibling =
                                Some(current_index);
                        }
                        Some(current_index)
                    }
                    None => Some(current_index),
                };
            }
        }
        self.states.append(&mut other.states);
        self.selection.append(&mut other.selection);
    }

    /// Branch into multiple child queries using a closure.
    ///
    /// The closure receives a [`QueryFactory`] that can create new
    /// sub-queries. Each sub-query is scoped to run only within elements
    /// matched by the current (most recently added) selector.
    ///
    /// This is the key mechanism for **structured querying**; extracting
    /// hierarchical data relationships in a single streaming pass.
    ///
    /// # Example
    ///
    /// ```rust
    /// use scah_query_ir::{Query, Save};
    ///
    /// let query = Query::all("article", Save::none())?
    ///     .then(|article| Ok([
    ///         article.first("h1", Save::only_text())?,
    ///         article.all("a[href]", Save::all())?,
    ///     ]))?
    ///     .build();
    /// # Ok::<(), scah_query_ir::SelectorParseError>(())
    /// ```
    pub fn then<F, I>(mut self, func: F) -> Result<Self, SelectorParseError>
    where
        F: FnOnce(QueryFactory) -> Result<I, SelectorParseError>,
        I: IntoIterator<Item = Self>,
    {
        let factory = QueryFactory {};
        let children = func(factory)?;

        let current_index = QuerySectionId(self.selection.len() - 1);
        for child in children {
            self.append(current_index, child);
        }
        Ok(self)
    }

    fn exit_at_section(&self) -> Option<QuerySectionId> {
        // returns the position in the selection tree where it can early exit
        // TODO: I should add a required flag for QuerySections, so that the first selection is nulled
        //  -> Basicly you can't return the first section without a perticular section behind added
        //  -> If you come back to the section without saving the required section,
        //      then you delete the saved data and you start over.

        fn search_for_single_exit_section(
            index: QuerySectionId,
            list: &[QuerySection<'_>],
        ) -> Option<QuerySectionId> {
            // If you have a section with MULTIPLE children that can early exit,
            //   then this parent node will become the exit section
            if index.index() >= list.len() {
                return None;
            }
            let section = &list[index.index()];
            let stop_here = match &section.kind {
                //BUG: you can only early exit when the ALL of them have been found, thus the parent must be awaited for
                SelectionKind::All => return None,

                // First queries that capture content cannot exit until the
                // winning element closes and its ranges are finalized.
                SelectionKind::First => section.save != Save::none(),
            };
            if stop_here {
                return Some(index);
            }

            let mut child = QuerySectionId(index.index() + 1);
            if child.index() >= list.len() {
                return Some(index);
            }

            let mut child_response: Option<QuerySectionId> = None;
            if let Some(parent) = list[child.index()].parent
                && parent == index
            {
                loop {
                    child_response = match child_response {
                        None => search_for_single_exit_section(child, list),
                        Some(_) => {
                            // If their's more than one child that can early exit then
                            // the parent is chosen
                            return Some(index);
                        }
                    };

                    if let Some(sibling) = list[child.index()].next_sibling {
                        child = sibling;
                    } else {
                        break;
                    }
                }
            }

            if child_response.is_some() {
                return child_response;
            }
            Some(index)
        }

        search_for_single_exit_section(QuerySectionId(0), &self.selection)
    }
}

impl<'query> QueryBuilder<'query> {
    /// Finalise the builder and produce a compiled [`Query`].
    ///
    /// This computes early-exit optimisation metadata and converts the
    /// internal vectors into boxed slices. After calling `build`, pass
    /// the resulting `Query` to [`parse`](crate::parse).
    pub fn build(self) -> Query<'query> {
        let exit_at_section_end = self.exit_at_section();
        let states_box = self.states.into_boxed_slice();
        let query_box = self.selection.into_boxed_slice();
        Query {
            states: states_box,
            queries: query_box,
            exit_at_section_end,
        }
    }
}

/// A factory for creating child [`QueryBuilder`]s inside a
/// [`QueryBuilder::then`] closure.
///
/// You never construct this directly; it is provided as the argument to
/// the closure passed to `.then()`.
pub struct QueryFactory {}
impl<'query> QueryFactory {
    /// Create a child query that matches **all** occurrences of the selector.
    pub fn all(
        &self,
        query: &'query str,
        save: Save,
    ) -> Result<QueryBuilder<'query>, SelectorParseError> {
        Query::all(query, save)
    }

    /// Create a child query that matches only the **first** occurrence.
    pub fn first(
        &self,
        query: &'query str,
        save: Save,
    ) -> Result<QueryBuilder<'query>, SelectorParseError> {
        Query::first(query, save)
    }
}

#[cfg(test)]
mod tests {
    use crate::{ClassSelections, Query, QuerySectionId, Save, SelectionKind};

    #[test]
    fn test_builder_with_class_chaining() {
        let query = Query::all("a.blue.exit", Save::all()).unwrap().build();
        assert_eq!(
            query.states[0].predicate.classes,
            ClassSelections::from_static(&["blue", "exit"])
        );
    }

    #[test]
    fn test_early_exit() {
        let query = Query::all("a", Save::all()).unwrap();
        assert_eq!(query.exit_at_section(), None);

        let query = Query::all("a", Save::none()).unwrap();
        assert_eq!(query.exit_at_section(), None);

        let query = Query::first("a", Save::all()).unwrap();
        assert_eq!(query.exit_at_section(), Some(QuerySectionId(0)));

        let query = Query::first("a", Save::none()).unwrap();
        assert_eq!(query.exit_at_section(), Some(QuerySectionId(0)));

        let query = Query::all("p", Save::all())
            .unwrap()
            .first("a", Save::all())
            .unwrap();
        assert_eq!(query.exit_at_section(), None);

        let query = Query::first("p", Save::all())
            .unwrap()
            .all("a", Save::all())
            .unwrap();
        assert_eq!(query.exit_at_section(), Some(QuerySectionId(0)));

        let query = Query::first("p", Save::all())
            .unwrap()
            .first("a", Save::all())
            .unwrap();
        assert_eq!(query.exit_at_section(), Some(QuerySectionId(0)));

        let query = Query::first("p", Save::none())
            .unwrap()
            .first("a", Save::none())
            .unwrap();
        assert_eq!(query.exit_at_section(), Some(QuerySectionId(1)));
    }

    #[test]
    fn test_invalid_selectors_fail_to_build() {
        let invalid = [
            "",
            "a > ",
            ".",
            "#",
            " a ~ b",
            "a + b",
            "a[]",
            "*",
            "a[123=\"321\"]",
            r#"[data-x="unterminated]"#,
            "[=value]",
            "[data-x^]",
        ];

        for selector in invalid {
            assert!(Query::all(selector, Save::none()).is_err(), "{selector}");
        }
    }

    // Priority 2: escaped quotes must not panic. The parser now correctly
    // handles escaped quotes inside attribute values (via raw slicing).
    #[test]
    fn escaped_quote_in_attribute_selector_does_not_panic() {
        let result = Query::all(r#"[data-x="a\"b"]"#, Save::none());
        // Must not panic. Either ok (escape handled) or err (if unsupported).
        let _ = result;
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let result = Query::all("#a1#a2", Save::none());
        assert!(result.is_err(), "duplicate IDs should be rejected");
    }

    // Priority 4: combinator tokenization without spaces
    #[test]
    fn child_combinator_without_spaces_is_valid() {
        assert!(Query::all("main>section", Save::none()).is_ok());
    }

    #[test]
    fn complex_selector_with_child_combinator_is_valid() {
        assert!(Query::all("main > section", Save::none()).is_ok());
    }

    // Priority 5: vertical tab must not be accepted as CSS whitespace
    #[test]
    fn vertical_tab_in_selector_is_rejected() {
        for selector in ["main \u{000B}", "\u{000B}", "main\t\u{000B}"] {
            assert!(
                Query::all(selector, Save::none()).is_err(),
                "{selector:?} must not treat vertical tab as CSS whitespace"
            );
        }
    }

    #[test]
    fn test_then_with_all_and_first() {
        let query = Query::all("article", Save::none())
            .unwrap()
            .then(|article| {
                Ok([
                    article.all("a[href]", Save::all())?,
                    article.first("h1", Save::only_text())?,
                ])
            })
            .unwrap()
            .build();

        assert_eq!(query.queries.len(), 3);
        assert_eq!(query.queries[1].source, "a[href]");
        assert_eq!(query.queries[2].source, "h1");
    }

    #[test]
    fn test_then_propagates_invalid_selector_from_callback() {
        let error = Query::all("article", Save::none())
            .unwrap()
            .then(|article| {
                Ok([
                    article.all("a[href]", Save::all())?,
                    article.first("a + b", Save::only_text())?,
                ])
            })
            .unwrap_err();

        assert_eq!(error.message(), "unsupported combinator '+'");
    }

    #[test]
    fn test_then_builds_sibling_links_in_callback_order() {
        let query = Query::all("article", Save::none())
            .unwrap()
            .then(|article| {
                Ok([
                    article.all("a[href]", Save::all())?,
                    article.first("h1", Save::only_text())?,
                    article.all("p", Save::none())?,
                ])
            })
            .unwrap()
            .build();

        assert_eq!(query.queries.len(), 4);
        assert_eq!(query.queries[1].parent, Some(QuerySectionId(0)));
        assert_eq!(query.queries[2].parent, Some(QuerySectionId(0)));
        assert_eq!(query.queries[3].parent, Some(QuerySectionId(0)));
        assert_eq!(query.queries[1].next_sibling, Some(QuerySectionId(2)));
        assert_eq!(query.queries[2].next_sibling, Some(QuerySectionId(3)));
        assert_eq!(query.queries[3].next_sibling, None);
        assert_eq!(query.queries[1].kind, SelectionKind::All);
        assert_eq!(query.queries[2].kind, SelectionKind::First);
        assert_eq!(query.queries[3].kind, SelectionKind::All);
    }

    #[test]
    fn test_nested_then_supports_all_and_first() {
        let query = Query::all("article", Save::none())
            .unwrap()
            .then(|article| {
                Ok([article.all("section", Save::none())?.then(|section| {
                    Ok([
                        section.first("h2", Save::only_text())?,
                        section.all("a[href]", Save::all())?,
                    ])
                })?])
            })
            .unwrap()
            .build();

        assert_eq!(query.queries.len(), 4);
        assert_eq!(query.queries[1].source, "section");
        assert_eq!(query.queries[2].source, "h2");
        assert_eq!(query.queries[3].source, "a[href]");
        assert_eq!(query.queries[2].parent, Some(QuerySectionId(1)));
        assert_eq!(query.queries[3].parent, Some(QuerySectionId(1)));
        assert_eq!(query.queries[2].next_sibling, Some(QuerySectionId(3)));
    }

    #[test]
    fn test_then_returns_error_without_partial_append() {
        let builder = Query::all("article", Save::none())
            .unwrap()
            .then(|article| {
                let first = article.all("section", Save::none())?;
                let second = article.first("a + b", Save::all());
                match second {
                    Ok(second) => Ok([first, second]),
                    Err(err) => Err(err),
                }
            });

        assert!(builder.is_err());
        let error = builder.unwrap_err();
        assert_eq!(error.message(), "unsupported combinator '+'");
    }
}
