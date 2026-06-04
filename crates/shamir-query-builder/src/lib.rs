//! `shamir-query-builder` — a fluent, client-side builder for ShamirDB batch
//! requests and responses.
//!
//! It is a **thin layer over `shamir-query-types`**: every method ultimately
//! constructs one of the existing wire DTOs (`BatchRequest`, `ReadQuery`,
//! `Filter`, `FilterValue`, `Select`/`SelectItem`, the write ops, …) and
//! nothing else. What you build is exactly what travels on the wire, and the
//! existing planner / engine handle it unchanged. The crate has no engine or
//! runtime dependency, so it compiles to WASM for browser clients.
//!
//! See `docs/roadmap/QUERY_BUILDER.md` for the full design.
//!
//! Modules are wired in here phase by phase as they land:
//! - `val`      — `FilterValue` constructors (lit / col / func / qref).
//! - `filter`   — `Filter` leaf constructors + and/or/not combinators.
//! - `query`    — `Query` (the `ReadQuery` builder).
//! - `select`   — `SelectItem` constructors (field / func / agg / …).
//! - `write`    — Insert / Update / Upsert / Delete + the `Doc` value builder.
//! - `batch`    — `Batch` + typed `Handle` dependency references.
//! - `response` — `BatchResponse` extraction helpers.
//! - `macros`   — `doc!` / `vals!` declarative macros; `filter!` / `q!` proc-macro re-exports.

// Allow the proc-macros (which emit `::shamir_query_builder::...` paths) to
// resolve when expanded inside this crate's own test suite.
#[cfg(test)]
extern crate self as shamir_query_builder;

pub mod batch;
pub mod filter;
#[macro_use]
pub mod macros;
pub mod query;
pub mod response;
pub mod select;
pub mod val;
pub mod wire;
pub mod write;

// Re-export the headline type for convenience.
pub use query::{Conds, IntoSelectItem, Query};
pub use wire::ToWire;

// Re-export proc-macros so users get `shamir_query_builder::{filter, q}`.
pub use shamir_query_builder_macros::{filter, q};
