//! Database gateway trait for WASM function read/write access (slice 8b).
//!
//! A [`DbGateway`] is the async boundary between the WASM host-import layer
//! and the real database engine. Functions call `ctx.db().table("t").get(...)`
//! etc., which bottom out in one of these trait methods.
//!
//! # Autocommit semantics (current slice)
//!
//! Each `get` / `insert` / `query` call builds a **single-op `BatchRequest`**
//! and routes it through `ShamirDb::execute`. The batch executor commits
//! independently per call — there is **no enclosing transaction** shared
//! across sequential gateway calls from the same function.
//!
//! This means a function that does `insert(a); insert(b)` performs two
//! autocommitted writes. If the second fails, `a` is already persisted.
//!
//! # Deferred: full-transaction integration
//!
//! When functions execute as batch ops (i.e. the function is invoked from
//! *within* a `ShamirDb::execute` call), the gateway should inherit the
//! batch's transaction and provide read-your-own-writes (RYOW) / SSI
//! isolation. That requires:
//!
//! 1. Passing the current `TxContext` through `FnCtx` / `HostState`.
//! 2. Routing gateway ops through the same transaction rather than
//!    opening new ones via `execute`.
//! 3. Re-entrancy guard: a function called from inside `execute` must not
//!    recursively call `execute` for DDL ops (it would deadlock on the
//!    batch planner lock).
//!
//! Until then, functions invoked standalone (via `invoke_function_in_db`)
//! use autocommit-per-op, which is correct and sufficient for this slice.

use async_trait::async_trait;
use shamir_types::types::value::QueryValue;

/// Async gateway that lets a WASM function read/write database tables.
///
/// Implemented once in the `shamir-db` crate (`FacadeDbGateway`) by routing
/// through `ShamirDb::execute`. The WASM host-import layer holds an
/// `Option<Arc<dyn DbGateway>>` — `None` means the function was invoked
/// without DB access (e.g. `invoke_function` without `_in_db`), and any
/// `db_*` host import traps with a clear error.
///
/// # Key convention for `get`
///
/// `get` receives a `key: QueryValue` which must be a `Value::Map` whose
/// entries are the primary-key field(s) of the target record. For a table
/// with a single `id` field: `Value::Map([("id", Value::Int(1))])`.
/// The gateway converts this map into a conjunction of `Eq` filters
/// (`{op: "and", filters: [{op: "eq", field: ["id"], value: 1}]}`).
///
/// If the key is a scalar (e.g. `Value::Int(1)`), the gateway treats it
/// as a filter on a default `"id"` field.
#[async_trait]
pub trait DbGateway: Send + Sync {
    /// Read a single record by key. Returns `None` if no record matches.
    ///
    /// See [key convention][self] above.
    async fn get(
        &self,
        repo: &str,
        table: &str,
        key: QueryValue,
    ) -> Result<Option<QueryValue>, String>;

    /// Insert a document. The `doc` must be a `Value::Map`. Returns the
    /// stored record (as returned by the batch executor).
    async fn insert(&self, repo: &str, table: &str, doc: QueryValue) -> Result<QueryValue, String>;

    /// Query records with an optional filter. `filter = None` means "all
    /// records". Returns zero or more records.
    async fn query(
        &self,
        repo: &str,
        table: &str,
        filter: Option<QueryValue>,
    ) -> Result<Vec<QueryValue>, String>;
}
