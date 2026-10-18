//! Ergonomic constructors for [`FilterValue`].
//!
//! Every function in this module returns a
//! [`shamir_query_types::filter::FilterValue`] — the universal expression
//! type that drives filters, function arguments, and computed write-values
//! on the wire.

mod cond;
mod expr;
mod filter_value;

pub use cond::*;
pub use expr::*;
pub use filter_value::*;

#[cfg(test)]
mod tests;
