//! Database access types for the guest SDK (slice 8b).
//!
//! Usage: `ctx.db().table("users").insert(doc)` etc.
//!
//! # Key convention
//!
//! `Table::get` accepts a scalar key as `id`, or a map of equality-key
//! fields. The empty map intentionally matches all rows and get returns
//! the first result.
//! `Table::get` and `Table::query` accept the WASM-bridge exception: equality
//! keys as scalars (on `id`) or maps of scalar values (`Null`, `Bool`, `Int`,
//! `F64`, `Str`, `Bin`). Any other shape fails the invocation. Use
//! `Db::execute` (feature `query-builder`) for anything beyond equality keys.

use crate::error::{Error, Result};
use crate::host_imports;
use crate::Value;
#[cfg(all(test, feature = "query-builder"))]
use std::cell::Cell;

/// Database access handle (bound to the default repo).
///
/// Obtained via [`crate::Ctx::db`]. Available to `#[procedure]` and
/// `#[function]` kinds. Scalars receive no injected `Ctx`, though their
/// bodies can construct one; host authorization still applies.
///
/// # Operations
///
/// ```ignore
/// let db = ctx.db();
///
/// // Open a table handle
/// let users = db.table("users");
///
/// // Get first matching record (empty map matches all rows)
/// let rec: Option<Value> = users.get(Value::Int(42))?;
///
/// // Query with optional filter (None = all rows)
/// let rows: Vec<Value> = users.query(None)?;
///
/// // Insert a document (must be Value::Map)
/// let stored = users.insert(Value::Map(vec![
///     ("name".into(), Value::Str("Alice".into())),
/// ]))?;
/// ```
#[derive(Debug, Clone)]
pub struct Db {
    _private: (),
}

impl Db {
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }

    /// Open a table handle for read/write operations.
    pub fn table(&self, name: &str) -> Table {
        Table {
            name: name.to_string(),
        }
    }
}

/// A handle to a specific table within the default repo.
///
/// Obtained via [`Db::table`]. Supports three operations:
///
/// | Method | Signature | Purpose |
/// |--------|-----------|---------|
/// | [`Table::get`] | `(key: Value) -> Result<Option<Value>>` | First matching record; empty map matches all rows |
/// | [`Table::insert`] | `(doc: Value) -> Result<Value>` | Insert a `Value::Map` document |
/// | [`Table::query`] | `(filter: Option<Value>) -> Result<Vec<Value>>` | Query rows (`None` = all) |
#[derive(Debug, Clone)]
pub struct Table {
    name: String,
}

impl Table {
    /// Read a single record by key.
    ///
    /// Returns the first record matching the key. An empty map removes the
    /// filter and therefore returns the first row (match-all); this is not
    /// restricted to primary-key fields.
    ///
    /// WASM-bridge exception: guest msgpack `Value` key -> typed `Filter`; use
    /// `Db::execute` (feature `query-builder`) for builder queries.
    ///
    /// Returns `Ok(None)` if no record matches. A codec failure decoding the
    /// record carries its cause in the returned `Err`.
    pub fn get(&self, key: Value) -> crate::Result<Option<Value>> {
        host_imports::db_get(&self.name, &key)
    }

    /// Insert a document into this table.
    ///
    /// `doc` must be a `Value::Map`. Returns the stored record.
    pub fn insert(&self, doc: Value) -> Result<Value> {
        let stored = host_imports::db_insert(&self.name, &doc)?;
        match stored {
            Value::Null => Err(Error::user("db_insert returned null")),
            other => Ok(other),
        }
    }

    /// Query records from this table with an optional filter.
    ///
    /// `filter = None` returns all records. A map of supported scalar values
    /// applies equality predicates; a scalar key filters `id`. Unsupported
    /// key shapes fail the invocation. Use `Db::execute` (feature
    /// `query-builder`) for anything beyond equality keys.
    /// WASM-bridge exception: guest msgpack `Value` key -> typed `Filter`; use
    /// `Db::execute` (feature `query-builder`) for builder queries.
    pub fn query(&self, filter: Option<Value>) -> Result<Vec<Value>> {
        let result = match filter {
            Some(ref f) => host_imports::db_query(&self.name, Some(f))?,
            None => host_imports::db_query(&self.name, None)?,
        };
        match result {
            Value::List(items) => Ok(items),
            _other => Err(Error::user(
                "db_query expected list, got unexpected value".to_string(),
            )),
        }
    }

    /// Returns the table name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(all(test, feature = "query-builder"))]
thread_local! {
    static EXECUTE_ENCODING_ERROR: Cell<bool> = const { Cell::new(false) };
}

#[cfg(all(test, feature = "query-builder"))]
pub(crate) fn with_execute_encoding_error<T>(run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            EXECUTE_ENCODING_ERROR.with(|flag| flag.set(false));
        }
    }
    EXECUTE_ENCODING_ERROR.with(|flag| flag.set(true));
    let reset = Reset;
    let result = run();
    drop(reset);
    result
}

/// Builder-in-guest execution (SDK Stage B2). Available with the
/// `query-builder` feature.
#[cfg(feature = "query-builder")]
impl Db {
    /// Run a full batch built with the query builder.
    ///
    /// The guest **describes** the batch (DTO); the host runs it through the
    /// same executor a wire client uses, as this function's effective actor,
    /// and returns the [`BatchResponse`](shamir_query_builder::BatchResponse).
    /// The engine never enters the guest.
    ///
    /// ```ignore
    /// let mut b = Batch::new();
    /// b.id("q");
    /// b.query("rows", Query::from("items").where_gte("n", 2_i64));
    /// let resp = ctx.db().execute(&b)?;
    /// let n = resp.results.get("rows").map(|r| r.records.len()).unwrap_or(0);
    /// ```
    pub fn execute(
        &self,
        batch: &shamir_query_builder::batch::Batch,
    ) -> crate::Result<shamir_query_builder::BatchResponse> {
        // Validate via try_build — deferred registration errors (duplicate
        // alias, unknown after/when handle) and unresolved `$query` refs must
        // surface here, not be sent silently to the host.
        let req = batch_request(batch)?;
        let bytes = map_execute_encoding(execute_encoding_result(&req))?;
        let resp_bytes = crate::host_imports::db_execute(&bytes);
        if resp_bytes.is_empty() {
            return Err(crate::Error::host("execute: host returned empty response"));
        }
        decode_execute_response(&resp_bytes)
    }
}

#[cfg(feature = "query-builder")]
fn execute_encoding_result<T: serde::Serialize>(
    value: &T,
) -> std::result::Result<Vec<u8>, rmp_serde::encode::Error> {
    #[cfg(test)]
    if EXECUTE_ENCODING_ERROR.with(|flag| flag.replace(false)) {
        return Err(rmp_serde::encode::Error::Syntax(
            "injected execute serialization failure".into(),
        ));
    }
    rmp_serde::to_vec_named(value)
}

#[cfg(feature = "query-builder")]
fn map_execute_encoding(
    result: std::result::Result<Vec<u8>, rmp_serde::encode::Error>,
) -> crate::Result<Vec<u8>> {
    result.map_err(|e| crate::Error::codec(format!("execute: encode batch: {e}")))
}

#[cfg(feature = "query-builder")]
fn decode_execute_response(bytes: &[u8]) -> crate::Result<shamir_query_builder::BatchResponse> {
    rmp_serde::from_slice(bytes)
        .map_err(|e| crate::Error::codec(format!("execute: decode response: {e}")))
}

/// [`BuildError`] into the SDK's [`Error`] — the testable seam of
/// [`Db::execute`]'s request-construction step (the host call itself is
/// guest-only and cannot run in unit tests).
#[cfg(feature = "query-builder")]
pub(crate) fn batch_request(
    batch: &shamir_query_builder::batch::Batch,
) -> crate::Result<shamir_query_builder::BatchRequest> {
    batch
        .try_build()
        .map_err(|e| crate::Error::user(format!("execute: invalid batch: {e}")))
}
