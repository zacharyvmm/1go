/// Whether a saved element's normalized range should trim collapsible edges.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextEdgePolicy {
    #[default]
    TrimCollapsedSeparators,
    Preserve,
}
