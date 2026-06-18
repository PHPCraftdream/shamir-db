//! [`Upsert`] builder for [`SetOp`].

use shamir_query_types::write::SetOp;
use shamir_query_types::TableRef;
use shamir_types::types::value::QueryValue;

/// Builder for [`SetOp`] (upsert: update-if-exists, insert-if-not).
pub struct Upsert {
    table_ref: TableRef,
    key: QueryValue,
    value: QueryValue,
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
            key: QueryValue::Null,
            value: QueryValue::Null,
        }
    }

    /// Create an upsert targeting `table` in a specific `repo`.
    pub fn with_repo(repo: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            table_ref: TableRef::with_repo(repo, table),
            key: QueryValue::Null,
            value: QueryValue::Null,
        }
    }

    /// Set the key to match on (id or unique field value).
    pub fn key(mut self, doc: impl Into<QueryValue>) -> Self {
        self.key = doc.into();
        self
    }

    /// Set the value to upsert.
    ///
    /// Accepts a [`Doc`](super::doc::Doc) (via `Into<QueryValue>`) or any
    /// `QueryValue` directly (e.g. from `mpack!({...})`).
    pub fn value(mut self, doc: impl Into<QueryValue>) -> Self {
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
