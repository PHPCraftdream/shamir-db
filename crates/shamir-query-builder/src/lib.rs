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
//! # Scope boundary — query construction, not transport envelope
//!
//! This crate builds the **`BatchRequest`** body and the `BatchOp` ops inside
//! it — the OQL/DDL surface. It deliberately does **not** construct most
//! top-level `DbRequest` envelope variants (`Ping`, `CreateScramUser`,
//! `TxBegin`/`TxExecute`/`TxCommit`/`TxRollback`): those are a *transport /
//! session-lifecycle* concern owned by the client SDKs, which already expose
//! them ergonomically (`shamir-client`: `ping()` / `create_scram_user()`;
//! `shamir-client-ts`: `ping()` / `createScramUser()` / `txBegin`/`txExecute`/
//! `txCommit`/`txRollback()` + the `db.tx()` handle). Keeping `DbRequest` out
//! of this crate preserves the clean layering (and the WASM-lean footprint):
//! the builder produces what goes *inside* an `Execute`, the SDK wraps the
//! envelope around it. This is by design, not a coverage gap.
//!
//! `cursor` (FG-5a) is a narrow, deliberate exception: `CreateCursor` embeds
//! a `ReadQuery` (the same query shape `Batch::query` already builds) and
//! `FetchNext`/`CancelCursor` reference an existing cursor by opaque id with
//! no query to construct at all — see that module's doc comment for the
//! full rationale.
//!
//! See `docs/dev-artifacts/roadmap/QUERY_BUILDER.md` for the full design.
//!
//! Modules are wired in here phase by phase as they land:
//! - `val`      — `FilterValue` constructors (lit / col / func / qref).
//! - `filter`   — `Filter` leaf constructors + and/or/not combinators.
//! - `query`    — `Query` (the `ReadQuery` builder).
//! - `select`   — `SelectItem` constructors (field / func / agg / …).
//! - `write`    — Insert / Update / Upsert / Delete + the `Doc` value builder.
//! - `batch`    — `Batch` + typed `Handle` dependency references.
//! - `cursor`   — `DbRequest::{CreateCursor,FetchNext,CancelCursor}` constructors (FG-5a).
//! - `response` — `BatchResponse` extraction helpers.
//! - `macros`   — `doc!` / `vals!` declarative macros; `filter!` / `q!` DSL macros.

// Allow the proc-macros (which emit `::shamir_query_builder::...` paths) to
// resolve when expanded inside this crate's own test suite.
#[cfg(test)]
extern crate self as shamir_query_builder;

#[cfg(test)]
#[path = "batch/tests/op/allocation_counter.rs"]
mod testing_alloc;

pub mod batch;
#[doc(hidden)]
pub use shamir_collections;
#[doc(hidden)]
pub use shamir_query_types;

pub mod ddl;
#[macro_use]
pub mod macros;
pub mod wire;
pub mod write;

// Read-side builders are grouped under `read`; every module keeps its
// original public path via the re-exports below.
mod read;
pub use batch::visibility;
pub use read::{cursor, filter, query, select, val};
pub use wire::response;

// Re-export the headline type for convenience.
pub use query::{Conds, IntoSelectItem, Query};
pub use wire::{ToWire, WireError};

// Re-export the batch request/response DTOs so a downstream guest (the SDK)
// can name them without depending on shamir-query-types directly.
pub use shamir_query_types::batch::{BatchRequest, BatchResponse};

// Re-export temporal read types so guest/SDK code can build temporal queries
// without a direct shamir-query-types dependency.
pub use shamir_query_types::read::{At, OrderDirection, Temporal};

// Re-export subscribe builder types for convenience.
pub use batch::subscribe::{SourceBuilder, Subscribe};
pub use shamir_query_types::subscribe::{DeliverMode, EventMask};

// Re-export the `q!(call ...)` expansion types through the builder facade.
#[doc(hidden)]
pub use shamir_query_types::call::CallOp;
#[doc(hidden)]
pub use shamir_query_types::filter::FilterValue;

#[doc(hidden)]
pub use shamir_query_builder_macros as __macros;

/// Build a `Filter` from a natural boolean expression.
///
/// Comparison operators (`==`, `!=`, `>`, `>=`, `<`, `<=`) map to
/// `filter::{eq, ne, gt, gte, lt, lte}`.  Logical operators (`&&`,
/// `||`, `!`) map to `filter::{and, or, not}`.  Parenthesised
/// sub-expressions group naturally (syn parses them before we see them).
///
/// The **LHS** of a comparison is a field path:
/// - bare ident `status` -> `"status"`
/// - dotted access `address.city` -> `["address", "city"]`
///
/// The **RHS** is emitted verbatim (literals, `col(...)`, `func(...)`,
/// variables — anything `impl Into<FilterValue>`).
///
/// ## Predicate calls
///
/// At any position where a filter sub-expression is expected (top-level,
/// or operand of `&&`/`||`/`!`), the following predicate-call forms are
/// recognised and lowered to the matching `filter::*` constructor:
///
/// - `like(field, pat)` / `ilike(field, pat)` / `regex(field, pat)`
/// - `is_null(field)` / `is_not_null(field)` / `exists(field)` / `not_exists(field)`
/// - `contains(field, v)` / `contains_any(field, [a, b])` / `contains_all(field, [a, b])`
/// - `in_(field, [a, b])` / `not_in(field, [a, b])`
/// - `between(field, lo, hi)`
/// - `fts(field, query, mode)`
/// - `vector_similarity(field, vecexpr, k)`
/// - `computed(expr_op, field, cmp, value)`
/// - `computed_with_args(expr_op, field, expr_args, cmp, value)`
///
/// These compose freely with `&&`/`||`/`!`/parens. For vector options, use
/// `filter::vector_similarity_ef` or `filter::vector_similarity_opts` directly.
///
/// # Examples
///
/// ```ignore
/// use shamir_query_builder::{filter, val::*};
///
/// let f = filter!(status == "active" && (role == "admin" || vip == true) && age > 18);
/// let g = filter!(like(name, "Al%") && !is_null(email) && between(age, 18, 65));
/// ```
#[macro_export]
macro_rules! filter {
    ($($t:tt)*) => { $crate::__macros::__filter!($crate; $($t)*) };
}

/// Build a query or write operation from a SQL-like DSL.
///
/// The **first keyword** selects one of six statement types. Read, insert,
/// and call return their DTOs; update, delete, and upsert return the
/// corresponding builder's `Result` (`.build()` has already been called).
///
/// # Grammar
///
/// ## Read (`from`)
///
/// ```text
/// q!( from <table | repo.table | "table">
///     [where <filter-expr>]
///     [group_by <field>, ...]
///     [having <filter-expr>]
///     [select [distinct] <select-items>]
///     [order_by <field> (asc|desc), ...]
///     [limit <N>]
///     [offset <N>]
/// )
/// ```
///
/// ## Insert
///
/// ```text
/// q!( insert into <table> values { "k" => v, ... } [, { ... }]* )
/// ```
///
/// ## Update
///
/// ```text
/// q!( update <table> set { "k" => v, ... } [where <filter-expr>] )
/// ```
///
/// ## Delete
///
/// ```text
/// q!( delete from <table> where <filter-expr> )   // where is REQUIRED
/// ```
///
/// ## Upsert
///
/// ```text
/// q!( upsert <table> key { "k" => v, ... } value { "k" => v, ... } )
/// ```
///
/// ## Call (stored procedure)
///
/// ```text
/// q!( call <fn_name>(arg1, arg2, ...) )
/// ```
///
/// `fn_name` is a bare ident (`my_proc`) or a string literal
/// (`"reports/daily"` for folder-qualified names). A repo-qualified form
/// `call my_repo.daily(...)` invokes the procedure in repository `my_repo`;
/// without a qualifier, `repo` defaults to `"main"`. Arguments are
/// expressions that implement `Into<FilterValue>` — literals, `col()`,
/// `func()`, `@`-style query refs, etc. Returns a `CallOp`.
///
/// ## `<table>` variants
///
/// - `users` — bare ident.
/// - `"user_events"` — string literal.
/// - `main.users` — repo-qualified (`with_repo("main","users")`).
///
/// ## `<doc>` — brace map literal
///
/// `{ "key" => value, ... }` — comma-separated pairs, trailing comma
/// allowed. Keys are string literals; values are arbitrary expressions
/// (`Into<FilterValue>`: literals, `col(...)`, `func(...)`, etc.).
///
/// ## `where` / `having`
///
/// Both use the full `filter!` expression grammar: comparisons
/// (`==`, `!=`, `>`, `>=`, `<`, `<=`), logical operators (`&&`, `||`,
/// `!`), parenthesised groups, and all 17 supported predicate calls (`like`,
/// `in_`, `between`, `fts`, `is_null`, `contains`, `computed`, ...).
/// For `vector_similarity_ef` and `vector_similarity_opts`, use the
/// corresponding `filter` builder APIs directly.
/// Note (30DA1AC8): clause keywords (`select`, `group_by`, `having`,
/// `order_by`, `limit`, `offset`) are reserved at the start of a
/// where/having expression, so a field named exactly like a clause
/// keyword cannot be used there (accepted limitation).
///
/// ## `select` items (comma-separated)
///
/// - `*` — wildcard (`select::all()`).
/// - `field` or `a.b` — field projection; optionally `as alias`.
/// - `count(*)` [as alias] — count all records.
/// - `count(field)|sum(field)|avg(field)|min(field)|max(field) as alias` — built-in aggregates (alias required).
/// - `agg_fn("name", field) as alias` — funclib aggregate.
/// - `func("ns/name", [args]) as alias` — scalar function projection.
///
/// ## `select distinct`
///
/// `select distinct ...` enables `DISTINCT` on the projection.
///
/// # Examples
///
/// ```ignore
/// use shamir_query_builder::{q, val::*};
///
/// // Read
/// let query = q!(from users where age > 18 select id, name order_by age desc limit 20);
///
/// // Insert
/// let ins = q!(insert into users values { "name" => "Alice", "age" => 30 });
///
/// // Update with complex where (F-13, #803: q!(update/delete/upsert ...)
/// // now yields a Result, mirroring Update::build()/Delete::build()/
/// // Upsert::build())
/// let upd = q!(update users set { "tier" => "gold" } where total > 1000 && !is_null(email)).unwrap();
///
/// // Delete with complex where
/// let del = q!(delete from users where status == "deleted" && age < 18).unwrap();
///
/// // Upsert
/// let ups = q!(upsert cache key { "id" => "k1" } value { "v" => 42 }).unwrap();
///
/// // Call stored procedure
/// let res = q!(call my_proc(1, "hello"));
/// let res = q!(call "reports/daily"(2024, "Q1"));
/// ```
#[macro_export]
macro_rules! q {
    ($($t:tt)*) => { $crate::__macros::__q!($crate; $($t)*) };
}
