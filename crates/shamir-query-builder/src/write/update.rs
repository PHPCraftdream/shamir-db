//! [`Update`] builder for [`UpdateOp`].

use shamir_query_types::filter::Filter;
use shamir_query_types::write::{UpdateOp, UpdateSelect};
use shamir_query_types::TableRef;
use shamir_types::types::value::QueryValue;

pub use shamir_query_types::write::UpdateReturnMode;

/// Builder for [`UpdateOp`].
pub struct Update {
    table_ref: TableRef,
    where_clause: Option<Filter>,
    set_value: QueryValue,
    select: Option<UpdateSelect>,
    expected_version: Option<u64>,
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
            set_value: QueryValue::Null,
            select: None,
            expected_version: None,
        }
    }

    /// Create an update targeting `table` in a specific `repo`.
    pub fn with_repo(repo: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            table_ref: TableRef::with_repo(repo, table),
            where_clause: None,
            set_value: QueryValue::Null,
            select: None,
            expected_version: None,
        }
    }

    /// Set the WHERE filter.
    pub fn where_(mut self, filter: Filter) -> Self {
        self.where_clause = Some(filter);
        self
    }

    /// Set the fields to update (the `set` payload).
    ///
    /// Accepts a [`Doc`](super::doc::Doc) (via `Into<QueryValue>`) or any
    /// `QueryValue` directly (e.g. from `mpack!({...})`).
    pub fn set(mut self, doc: impl Into<QueryValue>) -> Self {
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

    /// Set the optimistic-concurrency (CAS) version guard.
    ///
    /// When set, the server rejects the update with `version_conflict`
    /// unless every matched row is currently at exactly this version
    /// (the value surfaced by `Query::with_version()` on the read side).
    /// `None` (the default) disables the check.
    pub fn expected_version(mut self, version: u64) -> Self {
        self.expected_version = Some(version);
        self
    }

    /// Consume the builder and produce the wire DTO.
    pub fn build(self) -> UpdateOp {
        UpdateOp {
            update: self.table_ref,
            where_clause: self.where_clause,
            set: self.set_value,
            select: self.select,
            expected_version: self.expected_version,
        }
    }
}
