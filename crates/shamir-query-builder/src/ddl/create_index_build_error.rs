//! Client-side validation error from [`super::CreateIndex::try_build`].
//!
//! Mirrors the infallible-vs-fallible split already established by
//! `crate::query::QueryBuildError` / `crate::query::Query::try_build` and
//! `crate::batch::BuildError` / `crate::batch::Batch::try_build`: the legacy
//! [`super::CreateIndex::build`] stays infallible and permissive (backward
//! compatibility for existing call sites), while
//! [`super::CreateIndex::try_build`] performs the same construction plus the
//! validation checks the server enforces at DDL-execution time
//! (`shamir-db::execute::admin_table_index`) and returns this error on a
//! semantically ill-formed op — so a caller finds out at *construction* time,
//! not after a full round trip through the server.

/// Validation error returned by [`super::CreateIndex::try_build`].
///
/// [`super::CreateIndex::build`] (the legacy infallible path) does NOT surface
/// these — it remains permissive for backward compatibility. Use `try_build()`
/// to opt into the same checks the server (`admin_table_index.rs`) enforces at
/// DDL-execution time and the TS builder (`shamir-client-ts`) enforces in
/// `build()`.
#[derive(Debug, Clone, PartialEq)]
pub enum CreateIndexBuildError {
    /// `.unique()` and `.sorted()` were both set.
    ///
    /// The server rejects this in `admin_table_index.rs` with "Index cannot be
    /// both sorted and unique"; the TS builder rejects it synchronously in
    /// `ddl.ts`. `try_build()` mirrors both so the caller finds out at
    /// construction time, not after a server round trip.
    UniqueAndSorted,
    /// `.include(...)` was set without `.sorted()`.
    ///
    /// Included (covering) fields are only meaningful for sorted indexes — the
    /// server rejects this with "include is only valid for sorted indexes".
    IncludeWithoutSorted,
    /// `.sorted()` was set but the index does not have exactly one field.
    ///
    /// Sorted indexes are single-field scalar columns only; the server rejects
    /// composites with "Sorted index requires exactly one field (composite TBD)".
    SortedMultiField {
        /// The number of fields the caller supplied.
        field_count: usize,
    },
}

impl std::fmt::Display for CreateIndexBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CreateIndexBuildError::UniqueAndSorted => {
                write!(
                    f,
                    "index cannot be both sorted and unique; call either .unique() or .sorted(), \
                     not both — the server (admin_table_index) and the TS builder reject this \
                     combination"
                )
            }
            CreateIndexBuildError::IncludeWithoutSorted => {
                write!(
                    f,
                    ".include() is only valid for sorted indexes; call .sorted() before \
                     .include(), or drop the .include() call — the server rejects include \
                     without sorted"
                )
            }
            CreateIndexBuildError::SortedMultiField { field_count } => {
                write!(
                    f,
                    "sorted index requires exactly one field, got {field_count}; sorted indexes \
                     are single-field scalar columns only (composite TBD) — the server rejects \
                     multi-field sorted indexes"
                )
            }
        }
    }
}

impl std::error::Error for CreateIndexBuildError {}
