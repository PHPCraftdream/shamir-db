#![deny(clippy::unwrap_used, clippy::expect_used)]

//! Query evaluation leaf split from shamir-engine (#1548 S1).
//! `query` preserves the moved code's `crate::query` paths.

use shamir_index as index2;
mod function;
mod index {
    pub use shamir_index::base_index::sorted_index_manager;
}
pub mod query;
