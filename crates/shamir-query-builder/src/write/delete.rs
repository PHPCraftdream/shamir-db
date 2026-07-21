//! [`Delete`] builder for [`DeleteOp`].

use shamir_query_types::filter::Filter;
use shamir_query_types::write::{DeleteOp, DeleteSelect};
use shamir_query_types::TableRef;

/// Builder for [`DeleteOp`].
///
/// The WHERE clause is **required** by the wire DTO for safety. Calling
/// [`Delete::build`] without a prior [`Delete::where_`] call will panic
/// with a clear message. This is a deliberate programmer-error guard —
/// accidentally deleting all records in a table should never be silent.
pub struct Delete {
    table_ref: TableRef,
    where_clause: Option<Filter>,
    select: Option<DeleteSelect>,
    expected_version: Option<u64>,
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
            select: None,
            expected_version: None,
        }
    }

    /// Create a delete targeting `table` in a specific `repo`.
    pub fn with_repo(repo: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            table_ref: TableRef::with_repo(repo, table),
            where_clause: None,
            select: None,
            expected_version: None,
        }
    }

    /// Set the WHERE filter (required).
    pub fn where_(mut self, filter: Filter) -> Self {
        self.where_clause = Some(filter);
        self
    }

    /// Request that the server return the deleted records (all fields).
    ///
    /// DELETE has no changed/unchanged distinction — every matched row is
    /// removed — so `.returning()` takes no mode argument; the mere
    /// presence of a [`DeleteSelect`] opts in.
    pub fn returning(mut self) -> Self {
        self.select = Some(DeleteSelect { fields: None });
        self
    }

    /// Request that the server return specific fields of the deleted records.
    pub fn returning_fields(mut self, fields: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.select = Some(DeleteSelect {
            fields: Some(fields.into_iter().map(Into::into).collect()),
        });
        self
    }

    /// Set the optimistic-concurrency (CAS) version guard.
    ///
    /// When set, the server rejects the delete with `version_conflict`
    /// unless every matched row is currently at exactly this version
    /// (the value surfaced by `Query::with_version()` on the read side).
    /// `None` (the default) disables the check.
    pub fn expected_version(mut self, version: u64) -> Self {
        self.expected_version = Some(version);
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
            select: self.select,
            expected_version: self.expected_version,
        }
    }
}
