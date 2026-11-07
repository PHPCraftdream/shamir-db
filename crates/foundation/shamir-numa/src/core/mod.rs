//! Core NUMA primitives: cache padding, node ids, replicated state,
//! topology trait.

pub(crate) mod cache_padded;
pub(crate) mod node;
pub(crate) mod node_replicated;
pub(crate) mod topology;

pub use cache_padded::*;
pub use node::*;
pub use node_replicated::*;
pub use topology::*;
