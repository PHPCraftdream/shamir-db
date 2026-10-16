//! Ergonomic constructors for [`SelectItem`].
//!
//! Every function in this module returns a
//! [`shamir_query_types::read::SelectItem`] — the projection-item type
//! that drives `SELECT` clauses on the wire.

mod select_item;

pub use select_item::*;

// Re-export so callers can `use shamir_query_builder::select::{AggFunc, AggregateField}`.
pub use shamir_query_types::read::{AggFunc, AggregateField};

#[cfg(test)]
mod tests;
