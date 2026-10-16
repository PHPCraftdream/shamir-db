use shamir_query_types::filter::FilterValue;

use crate::val::{qref, qref_all, IntoFieldPath};

/// A typed reference to a query registered in a [`super::Batch`].
///
/// Returned by `Batch::query`, `Batch::insert`, etc. Its methods produce
/// `FilterValue::QueryRef` values that the engine's batch planner
/// interprets as inter-query dependencies.
#[derive(Debug, Clone)]
pub struct Handle {
    pub(super) alias: String,
}

impl Handle {
    /// The bare alias (without `@` prefix).
    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// Reference a column across all result rows.
    ///
    /// Produces a `$query` path like `"[].field"` (or `"[].a.b"` for
    /// nested fields).
    pub fn column(&self, field: impl IntoFieldPath) -> FilterValue {
        let segments = field.into_field_path();
        let dotted = segments.join(".");
        let path = format!("[].{dotted}");
        qref(&self.alias, path)
    }

    /// Reference a specific result row by index.
    pub fn row(&self, index: usize) -> RowRef {
        RowRef {
            alias: self.alias.clone(),
            index,
        }
    }

    /// Shorthand for `row(0)`.
    pub fn first(&self) -> RowRef {
        self.row(0)
    }

    /// Reference the entire result (no path).
    pub fn all(&self) -> FilterValue {
        qref_all(&self.alias)
    }
}

/// A reference to a specific row of a query result.
#[derive(Debug, Clone)]
pub struct RowRef {
    alias: String,
    index: usize,
}

impl RowRef {
    /// Reference a field on this row.
    ///
    /// Produces a `$query` path like `"[0].field"`.
    pub fn field(&self, field: impl IntoFieldPath) -> FilterValue {
        let segments = field.into_field_path();
        let dotted = segments.join(".");
        let path = format!("[{}].{dotted}", self.index);
        qref(&self.alias, path)
    }

    /// Reference the whole row (no field).
    ///
    /// Produces a `$query` path like `"[0]"`.
    pub fn get(&self) -> FilterValue {
        let path = format!("[{}]", self.index);
        qref(&self.alias, path)
    }
}
