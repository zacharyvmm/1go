#[inline]
pub(crate) fn is_css_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)
}

mod builder;
mod eq;
mod lexer;
mod string_search;

pub use builder::{
    Attribute, AttributeSelection, AttributeSelections, ClassSelections, ElementPredicate, IElement,
};
pub use lexer::Combinator;
pub(super) use lexer::Lexer;
pub use string_search::AttributeSelectionKind;
