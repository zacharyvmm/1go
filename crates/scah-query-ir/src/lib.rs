mod query;

pub use query::compiler::lazy;
pub use query::compiler::{
    Position, Query, QueryBuilder, QueryFactory, QuerySection, QuerySectionId, QuerySpec, Save,
    SelectionKind, SelectorParseError, StaticQuery, Transition, TransitionId,
};
pub use query::selector::{
    Attribute, AttributeSelection, AttributeSelectionKind, AttributeSelections, ClassSelections,
    Combinator, ElementPredicate, IElement,
};
pub use scah_reader::Reader;
