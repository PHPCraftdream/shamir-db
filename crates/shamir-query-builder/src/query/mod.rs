//! Fluent builder for [`ReadQuery`] — CodeIgniter Active Record style.
//!
//! [`Query`] is the headline API: chain projections, filters, grouping,
//! ordering, and pagination to produce a fully-formed
//! [`shamir_query_types::read::ReadQuery`] ready for the wire.
//!
//! # Quick example
//!
//! ```rust
//! use shamir_query_builder::{Query, filter, select, val::*};
//!
//! let rq = Query::from("users")
//!     .select(["id", "name", "age"])
//!     .where_eq("status", "active")
//!     .where_gt("age", 18)
//!     .where_in("role", ["admin", "mod"])
//!     .like("name", "Al%")
//!     .order_by_desc("age")
//!     .limit(20)
//!     .offset(40)
//!     .build();
//! ```
//!
//! Filters chain with AND by default (CodeIgniter semantics). Use
//! `or_where_*` for OR, and `where_group` / `where_group_or` for nested
//! parenthesised groups.

mod conds;
mod into_select_item;
#[allow(clippy::module_inception)]
mod query;

pub use conds::*;
pub use into_select_item::*;
pub use query::*;

#[cfg(test)]
mod tests;
