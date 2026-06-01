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
/// Obtained via [`crate::Ctx::db`].
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
/// Supports `get`, `insert`, and `query` operations via host imports.
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
