mod builder;
mod error;
pub mod lazy;
mod query;
mod transition;

pub use builder::{QueryBuilder, QueryFactory, Save, SelectionKind};
pub use error::SelectorParseError;
pub use query::{
    Position, Query, QuerySection, QuerySectionId, QuerySpec, StaticQuery, TextRequirements,
    TransitionId,
};
pub use transition::Transition;
