//! Proc-macros for `shamir-query-builder`: `filter!` and `q!`.
//!
//! These macros emit **fully-qualified paths** (`::shamir_query_builder::...`)
//! so they work from any crate that depends on `shamir-query-builder`.
//! This crate must NOT depend on `shamir-query-builder` (would be a cycle).

use proc_macro::TokenStream;

mod filter_lower;
mod query_parse;

/// Build a [`Filter`] from a natural boolean expression.
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
/// These compose freely with `&&`/`||`/`!`/parens.
///
/// # Examples
///
/// ```ignore
/// use shamir_query_builder::{filter, val::*};
///
/// let f = filter!(status == "active" && (role == "admin" || vip == true) && age > 18);
/// let g = filter!(like(name, "Al%") && !is_null(email) && between(age, 18, 65));
/// ```
#[proc_macro]
pub fn filter(input: TokenStream) -> TokenStream {
    filter_lower::filter_macro(input)
}

/// Build a query or write operation from a SQL-like DSL.
///
/// The **first keyword** selects the statement type. All five forms
/// return the corresponding builder DTO (`.build()` already called).
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
/// `!`), parenthesised groups, and all 19 predicate calls (`like`,
/// `in_`, `between`, `fts`, `is_null`, `contains`, `computed`, ...).
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
/// // Update with complex where
/// let upd = q!(update users set { "tier" => "gold" } where total > 1000 && !is_null(email));
///
/// // Delete with complex where
/// let del = q!(delete from users where status == "deleted" && age < 18);
///
/// // Upsert
/// let ups = q!(upsert cache key { "id" => "k1" } value { "v" => 42 });
/// ```
#[proc_macro]
pub fn q(input: TokenStream) -> TokenStream {
    query_parse::q_macro(input)
}
