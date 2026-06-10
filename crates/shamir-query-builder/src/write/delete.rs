//! [`Delete`] builder for [`DeleteOp`].

use shamir_query_types::filter::Filter;
use shamir_query_types::write::DeleteOp;
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
    pub fn where_(mut self, filter: Filter) -> Self {
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
