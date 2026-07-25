//! [`Upsert`] builder for [`SetOp`].

use shamir_query_types::write::SetOp;
use shamir_query_types::TableRef;
use shamir_types::types::value::QueryValue;

use super::BuilderError;

/// Builder for [`SetOp`] (upsert: update-if-exists, insert-if-not).
pub struct Upsert {
    table_ref: TableRef,
    key: Option<QueryValue>,
    value: Option<QueryValue>,
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
            key: None,
            value: None,
        }
    }

    /// Create an upsert targeting `table` in a specific `repo`.
    pub fn with_repo(repo: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            table_ref: TableRef::with_repo(repo, table),
            key: None,
            value: None,
        }
    }

    /// Set the key to match on (id or unique field value).
    pub fn key(mut self, doc: impl Into<QueryValue>) -> Self {
        self.key = Some(doc.into());
        self
    }

    /// Set the value to upsert.
    ///
    /// Accepts a [`Doc`](super::doc::Doc) (via `Into<QueryValue>`) or any
    /// `QueryValue` directly (e.g. from `mpack!({...})`).
    pub fn value(mut self, doc: impl Into<QueryValue>) -> Self {
        self.value = Some(doc.into());
        self
    }

    /// Consume the builder and produce the wire DTO.
    ///
    /// Returns [`Err(BuilderError::MissingKey)`](BuilderError::MissingKey) /
    /// [`Err(BuilderError::MissingValue)`](BuilderError::MissingValue) if
    /// [`Upsert::key`] / [`Upsert::value`] was never called. A deliberate
    /// `.key(QueryValue::Null)` / `.value(QueryValue::Null)` still builds
    /// successfully (the old `QueryValue::Null`-sentinel ambiguity is gone:
    /// absence is tracked explicitly).
    pub fn build(self) -> Result<SetOp, BuilderError> {
        Ok(SetOp {
            set: self.table_ref,
            key: self.key.ok_or(BuilderError::MissingKey)?,
            value: self.value.ok_or(BuilderError::MissingValue)?,
        })
    }
}
