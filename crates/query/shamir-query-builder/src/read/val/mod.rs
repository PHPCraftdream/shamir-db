//! Ergonomic constructors for [`FilterValue`].
//!
//! Every function in this module returns a
//! [`shamir_query_types::filter::FilterValue`] — the universal expression
//! type that drives filters, function arguments, and computed write-values
//! on the wire.
//!
//! When using projections as well, prefer qualified `val::func` and
//! `select::func` (and `val::expr` and `select::expr`) or explicit aliases;
//! glob-importing both modules makes those names ambiguous.

mod cond;
mod expr;
mod filter_value;

pub use cond::*;
pub use expr::*;
pub use filter_value::*;

#[cfg(test)]
mod tests;
