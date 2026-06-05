//! Database access types for the guest SDK (slice 8b).
//!
//! Usage: `ctx.db().table("users").insert(doc)` etc.
//!
//! # Key convention
//!
//! `Table::get` accepts a `Value::Map` whose entries are the primary-key
//! fields: `Value::Map(vec![("id".to_string(), Value::Int(1))])`.
//! A scalar value is treated as a filter on a default `"id"` field.

use crate::error::{Error, Result};
use crate::host_imports;
use crate::Value;

/// Database access handle (bound to the default repo).
///
/// Obtained via [`crate::Ctx::db`]. Available to `#[procedure]` and
/// `#[function]` kinds. **Not** available to `#[scalar]` (purity).
///
/// # Operations
///
/// ```ignore
/// let db = ctx.db();
///
/// // Open a table handle
/// let users = db.table("users");
///
/// // Get a single record by primary key
/// let rec: Option<Value> = users.get(Value::Int(42));
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
/// | [`Table::get`] | `(key: Value) -> Option<Value>` | Single-record lookup by primary key |
/// | [`Table::insert`] | `(doc: Value) -> Result<Value>` | Insert a `Value::Map` document |
/// | [`Table::query`] | `(filter: Option<Value>) -> Result<Vec<Value>>` | Query rows (`None` = all) |
#[derive(Debug, Clone)]
pub struct Table {
    name: String,
}

impl Table {
    /// Read a single record by key.
    ///
    /// `key` should be a `Value::Map` of the primary-key fields, e.g.
    /// `Value::Map(vec![("id".into(), Value::Int(1))])`.
    /// A scalar is treated as a filter on `"id"`.
    ///
    /// Returns `Ok(None)` if no record matches.
    pub fn get(&self, key: Value) -> Option<Value> {
        host_imports::db_get(&self.name, &key)
    }

    /// Insert a document into this table.
    ///
    /// `doc` must be a `Value::Map`. Returns the stored record.
    pub fn insert(&self, doc: Value) -> Result<Value> {
        let result = host_imports::db_insert(&self.name, &doc);
        match result {
            Value::Null => Err(Error::user("db_insert returned null")),
            other => Ok(other),
        }
    }

    /// Query records from this table with an optional filter.
    ///
    /// `filter = None` returns all records. The filter follows the same
    /// key convention as `get`.
    pub fn query(&self, filter: Option<Value>) -> Result<Vec<Value>> {
        let result = match filter {
            Some(ref f) => host_imports::db_query(&self.name, Some(f)),
            None => host_imports::db_query(&self.name, None),
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
        let req = batch.build();
        let bytes = rmp_serde::to_vec_named(&req)
            .map_err(|e| crate::Error::user(format!("execute: encode batch: {e}")))?;
        let resp_bytes = crate::host_imports::db_execute(&bytes);
        if resp_bytes.is_empty() {
            return Err(crate::Error::user("execute: host returned empty response"));
        }
        rmp_serde::from_slice(&resp_bytes)
            .map_err(|e| crate::Error::user(format!("execute: decode response: {e}")))
    }
}
