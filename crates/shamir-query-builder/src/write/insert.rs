//! [`Insert`] builder for [`InsertOp`].

use shamir_query_types::write::InsertOp;
use shamir_query_types::TableRef;
use shamir_types::types::value::QueryValue;

/// Builder for [`InsertOp`].
pub struct Insert {
    table_ref: TableRef,
    values: Vec<QueryValue>,
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
    /// Accepts a [`Doc`](super::doc::Doc) (via `Into<QueryValue>`) or any
    /// `QueryValue` directly (e.g. from `mpack!({...})`).
    pub fn row(mut self, value: impl Into<QueryValue>) -> Self {
        self.values.push(value.into());
        self
    }

    /// Append multiple records.
    pub fn rows(mut self, values: impl IntoIterator<Item = impl Into<QueryValue>>) -> Self {
        self.values.extend(values.into_iter().map(Into::into));
        self
    }

    /// Consume the builder and produce the wire DTO.
    pub fn build(self) -> InsertOp {
        InsertOp {
            insert_into: self.table_ref,
            values: self.values,
            records_idmsgpack: Vec::new(),
        }
    }
}
