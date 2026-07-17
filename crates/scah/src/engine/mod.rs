pub mod cursor;
pub mod executor;
pub mod multiplexer;
//pub mod tree;

pub(crate) type DepthSize = u16;

/// Maximum real element depth. Depths at or above [`DepthSize::MAX`] are
/// reserved for cursor sentinels such as [`cursor::SENTINEL_SCOPE`](cursor::SENTINEL_SCOPE).
pub(crate) const MAX_ELEMENT_DEPTH: DepthSize = DepthSize::MAX - 1;
