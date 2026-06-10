//! Constructors and combinators for [`Filter`].
//!
//! Leaf constructors (`eq`, `ne`, `gt`, ...) each produce a single
//! [`shamir_query_types::filter::Filter`] variant. Combinators (`and`,
//! `or`, `not`) compose them into trees. The [`FilterExt`] trait adds
//! chainable `.and()` / `.or()` / `.negate()` methods with smart
//! flattening.

mod combinators;
mod leaf;

pub use combinators::*;
pub use leaf::*;

#[cfg(test)]
mod tests;
