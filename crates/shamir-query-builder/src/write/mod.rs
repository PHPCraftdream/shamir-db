//! Write-operation builders: [`Doc`], [`Insert`], [`Update`], [`Upsert`],
//! [`Delete`].
//!
//! Each builder produces exactly the corresponding wire DTO from
//! `shamir_query_types::write` — no parallel model, no extra
//! serialization layer.
//!
//! # `Doc` — record-value builder
//!
//! A write record is a JSON object whose field values are **either**
//! literal JSON **or** expressions (computed `{"$fn":...}`,
//! `{"$ref":...}`, `{"$query":...}`). Expressions are produced by
//! serializing a [`FilterValue`] to JSON.
//!
//! ```ignore
//! use shamir_query_builder::{write::doc, val::*};
//!
//! let d = doc()
//!     .set("email", "Alice@X.COM")
//!     .set("email_norm", func("strings/lower", [col("email")]));
//! ```
//!
//! # Op builders
//!
//! ```ignore
//! use shamir_query_builder::write::*;
//! use shamir_query_builder::{val::*, filter::*};
//!
//! // Insert
//! let ins = insert("users")
//!     .row(doc().set("name", "Alice"))
//!     .build();
//!
//! // Update
//! let upd = update("users")
//!     .where_(eq("id", 1))
//!     .set(doc().set("name", "Bob"))
//!     .returning(UpdateReturnMode::All)
//!     .build();
//!
//! // Upsert (SetOp)
//! let ups = upsert("cache")
//!     .key(serde_json::json!("k1"))
//!     .value(doc().set("v", 42))
//!     .build();
//!
//! // Delete
//! let del = delete("sessions")
//!     .where_(eq("expired", true))
//!     .build();
//! ```

use serde_json::{Map, Value};
use shamir_query_types::filter::FilterValue;
use shamir_query_types::write::{DeleteOp, InsertOp, SetOp, UpdateOp, UpdateSelect};
use shamir_query_types::TableRef;

// Re-export `UpdateReturnMode` for ergonomic `use write::*` imports.
pub use shamir_query_types::write::UpdateReturnMode;

// ============================================================================
// Doc — record-value builder
// ============================================================================

/// A record-value builder that produces a [`serde_json::Value::Object`].
///
/// Field values are either literals or computed expressions — both go
/// through [`Doc::set`], which accepts any `impl Into<FilterValue>`.
/// Literals (`i32`, `&str`, `bool`, etc.) and expressions (`col(...)`,
/// `func(...)`, `qref(...)`) all implement `Into<FilterValue>`.
///
/// For the rare case of a nested literal JSON object/array (which
/// `FilterValue` cannot represent — it has no Map variant), use
/// [`Doc::set_json`].
///
/// Insertion order is preserved when the `serde_json` crate has
/// `preserve_order` enabled; otherwise iteration order is unspecified.
#[derive(Debug, Clone, Default)]
pub struct Doc {
    fields: Map<String, Value>,
}

/// Create an empty [`Doc`].
pub fn doc() -> Doc {
    Doc::new()
}

impl Doc {
    /// Create an empty document.
    pub fn new() -> Self {
        Self { fields: Map::new() }
    }

    /// Set a field to a literal value or a computed expression.
    ///
    /// Accepts anything that converts `Into<FilterValue>`:
    /// - Literals: `i8`..`i64`, `u8`..`u32`, `f32`, `f64`, `bool`,
    ///   `&str`, `String`.
    /// - Expressions: `col(...)`, `func(...)`, `qref(...)`, `qref_all(...)`.
    ///
    /// The value is serialized to `serde_json::Value` internally.
    pub fn set(mut self, key: impl Into<String>, value: impl Into<FilterValue>) -> Self {
        let fv: FilterValue = value.into();
        let json = serde_json::to_value(fv).expect("FilterValue serialization is infallible");
        self.fields.insert(key.into(), json);
        self
    }

    /// Set a field to a raw JSON value.
    ///
    /// Use this for nested literal objects or arrays that `FilterValue`
    /// cannot represent (it has no Map variant). For everything else,
    /// prefer [`Doc::set`].
    pub fn set_json(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    /// Consume the builder and return the JSON object.
    pub fn build(self) -> Value {
        Value::Object(self.fields)
    }
}

impl From<Doc> for Value {
    fn from(doc: Doc) -> Self {
        doc.build()
    }
}

// ============================================================================
// Insert
// ============================================================================

/// Builder for [`InsertOp`].
pub struct Insert {
    table_ref: TableRef,
    values: Vec<Value>,
}

/// Create an [`Insert`] builder targeting the given table (default repo).
pub fn insert(table: impl Into<String>) -> Insert {
    Insert::into(table)
}

impl Insert {
    /// Create an insert targeting `table` in the default repo.
    pub fn into(table: impl Into<String>) -> Self {
        Self {
            table_ref: TableRef::new(table),
            values: Vec::new(),
        }
    }

    /// Create an insert targeting `table` in a specific `repo`.
    pub fn with_repo(repo: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            table_ref: TableRef::with_repo(repo, table),
            values: Vec::new(),
        }
    }

    /// Append a single record.
    ///
    /// Accepts a [`Doc`] (via `Into<Value>`) or a raw `serde_json::Value`.
    pub fn row(mut self, value: impl Into<Value>) -> Self {
        self.values.push(value.into());
        self
    }

    /// Append multiple records.
    pub fn rows(mut self, values: impl IntoIterator<Item = impl Into<Value>>) -> Self {
        self.values.extend(values.into_iter().map(Into::into));
        self
    }

    /// Consume the builder and produce the wire DTO.
    pub fn build(self) -> InsertOp {
        InsertOp {
            insert_into: self.table_ref,
            values: self.values,
        }
    }
}

// ============================================================================
// Update
// ============================================================================

/// Builder for [`UpdateOp`].
pub struct Update {
    table_ref: TableRef,
    where_clause: Option<shamir_query_types::filter::Filter>,
    set_value: Value,
    select: Option<UpdateSelect>,
}

/// Create an [`Update`] builder targeting the given table (default repo).
pub fn update(table: impl Into<String>) -> Update {
    Update::table(table)
}

impl Update {
    /// Create an update targeting `table` in the default repo.
    pub fn table(table: impl Into<String>) -> Self {
        Self {
            table_ref: TableRef::new(table),
            where_clause: None,
            set_value: Value::Null,
            select: None,
        }
    }

    /// Create an update targeting `table` in a specific `repo`.
    pub fn with_repo(repo: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            table_ref: TableRef::with_repo(repo, table),
            where_clause: None,
            set_value: Value::Null,
            select: None,
        }
    }

    /// Set the WHERE filter.
    pub fn where_(mut self, filter: shamir_query_types::filter::Filter) -> Self {
        self.where_clause = Some(filter);
        self
    }

    /// Set the fields to update (the `set` payload).
    ///
    /// Accepts a [`Doc`] (via `Into<Value>`) or a raw `serde_json::Value`.
    pub fn set(mut self, doc: impl Into<Value>) -> Self {
        self.set_value = doc.into();
        self
    }

    /// Request that the server return matching records with the given
    /// mode (all fields).
    pub fn returning(mut self, mode: UpdateReturnMode) -> Self {
        self.select = Some(UpdateSelect {
            return_mode: mode,
            fields: None,
        });
        self
    }

    /// Request that the server return specific fields with the given mode.
    pub fn returning_fields(
        mut self,
        mode: UpdateReturnMode,
        fields: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.select = Some(UpdateSelect {
            return_mode: mode,
            fields: Some(fields.into_iter().map(Into::into).collect()),
        });
        self
    }

    /// Consume the builder and produce the wire DTO.
    pub fn build(self) -> UpdateOp {
        UpdateOp {
            update: self.table_ref,
            where_clause: self.where_clause,
            set: self.set_value,
            select: self.select,
        }
    }
}

// ============================================================================
// Upsert (SetOp)
// ============================================================================

/// Builder for [`SetOp`] (upsert: update-if-exists, insert-if-not).
pub struct Upsert {
    table_ref: TableRef,
    key: Value,
    value: Value,
}

/// Create an [`Upsert`] builder targeting the given table (default repo).
pub fn upsert(table: impl Into<String>) -> Upsert {
    Upsert::table(table)
}

impl Upsert {
    /// Create an upsert targeting `table` in the default repo.
    pub fn table(table: impl Into<String>) -> Self {
        Self {
            table_ref: TableRef::new(table),
            key: Value::Null,
            value: Value::Null,
        }
    }

    /// Create an upsert targeting `table` in a specific `repo`.
    pub fn with_repo(repo: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            table_ref: TableRef::with_repo(repo, table),
            key: Value::Null,
            value: Value::Null,
        }
    }

    /// Set the key to match on (id or unique field value).
    pub fn key(mut self, doc: impl Into<Value>) -> Self {
        self.key = doc.into();
        self
    }

    /// Set the value to upsert.
    ///
    /// Accepts a [`Doc`] (via `Into<Value>`) or a raw `serde_json::Value`.
    pub fn value(mut self, doc: impl Into<Value>) -> Self {
        self.value = doc.into();
        self
    }

    /// Consume the builder and produce the wire DTO.
    pub fn build(self) -> SetOp {
        SetOp {
            set: self.table_ref,
            key: self.key,
            value: self.value,
        }
    }
}

// ============================================================================
// Delete
// ============================================================================

/// Builder for [`DeleteOp`].
///
/// The WHERE clause is **required** by the wire DTO for safety. Calling
/// [`Delete::build`] without a prior [`Delete::where_`] call will panic
/// with a clear message. This is a deliberate programmer-error guard —
/// accidentally deleting all records in a table should never be silent.
pub struct Delete {
    table_ref: TableRef,
    where_clause: Option<shamir_query_types::filter::Filter>,
}

/// Create a [`Delete`] builder targeting the given table (default repo).
pub fn delete(table: impl Into<String>) -> Delete {
    Delete::from_table(table)
}

impl Delete {
    /// Create a delete targeting `table` in the default repo.
    pub fn from_table(table: impl Into<String>) -> Self {
        Self {
            table_ref: TableRef::new(table),
            where_clause: None,
        }
    }

    /// Create a delete targeting `table` in a specific `repo`.
    pub fn with_repo(repo: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            table_ref: TableRef::with_repo(repo, table),
            where_clause: None,
        }
    }

    /// Set the WHERE filter (required).
    pub fn where_(mut self, filter: shamir_query_types::filter::Filter) -> Self {
        self.where_clause = Some(filter);
        self
    }

    /// Consume the builder and produce the wire DTO.
    ///
    /// # Panics
    ///
    /// Panics if [`Delete::where_`] was not called. The `DeleteOp` wire
    /// type requires a filter — omitting it is always a programmer bug.
    pub fn build(self) -> DeleteOp {
        DeleteOp {
            delete_from: self.table_ref,
            where_clause: self.where_clause.expect(
                "Delete::build() requires a where clause — call .where_(filter) before .build()",
            ),
        }
    }
}

#[cfg(test)]
mod tests;
