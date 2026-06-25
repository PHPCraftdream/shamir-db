//! [`Insert`] builder for [`InsertOp`].

use shamir_query_types::write::{InsertOp, InsertSelect};
use shamir_query_types::TableRef;
use shamir_types::types::value::QueryValue;

/// Builder for [`InsertOp`].
pub struct Insert {
    table_ref: TableRef,
    values: Vec<QueryValue>,
    select: Option<InsertSelect>,
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
            select: None,
        }
    }

    /// Create an insert targeting `table` in a specific `repo`.
    pub fn with_repo(repo: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            table_ref: TableRef::with_repo(repo, table),
            values: Vec::new(),
            select: None,
        }
    }

    /// Append a single record.
    ///
    /// Accepts a [`Doc`](super::doc::Doc) (via `Into<QueryValue>`) or any
    /// `QueryValue` directly (e.g. from `mpak!({...})`).
    pub fn row(mut self, value: impl Into<QueryValue>) -> Self {
        self.values.push(value.into());
        self
    }

    /// Append multiple records.
    pub fn rows(mut self, values: impl IntoIterator<Item = impl Into<QueryValue>>) -> Self {
        self.values.extend(values.into_iter().map(Into::into));
        self
    }

    /// Restrict the returned inserted records to the given fields.
    ///
    /// INSERT always returns the inserted rows (when the caller asks for
    /// results via `return_result`); this builder method opts in to a
    /// projection so each returned row carries only the named fields.
    /// Symmetric with `Update::returning_fields` / `Delete::returning_fields`.
    pub fn returning_fields(mut self, fields: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.select = Some(InsertSelect {
            fields: Some(fields.into_iter().map(Into::into).collect()),
        });
        self
    }

    /// Consume the builder and produce the wire DTO.
    pub fn build(self) -> InsertOp {
        InsertOp {
            insert_into: self.table_ref,
            values: self.values,
            records_idmsgpack: Vec::new(),
            select: self.select,
        }
    }
}
