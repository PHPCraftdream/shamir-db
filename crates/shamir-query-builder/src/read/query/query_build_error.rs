//! Client-side validation error from [`super::Query::try_build`].
//!
//! Mirrors the infallible-vs-fallible split already established by
//! `crate::batch::BuildError` / `crate::batch::Batch::try_build`: the legacy
//! [`super::Query::build`] stays infallible and permissive (backward
//! compatibility for the many existing call sites), while [`super::Query::try_build`]
//! performs the same construction plus a validation pass and returns this error
//! on a semantically ill-formed query.

/// Validation error returned by [`super::Query::try_build`].
///
/// [`super::Query::build`] (the legacy infallible path) does NOT surface these —
/// it remains permissive for backward compatibility. Use `try_build()` to opt
/// into the same checks the TS builder (`shamir-client-ts`) enforces in
/// `build()`.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryBuildError {
    /// `.having(f)` was called without a prior `.group_by(...)`.
    ///
    /// HAVING is only meaningful over an aggregation; without GROUP BY the
    /// built `ReadQuery` carries an empty-group `GroupBy { fields: vec![],
    /// having: Some(f) }` that is shaped-valid but semantically empty. The TS
    /// builder rejects this case in `build()`; `try_build()` mirrors that.
    HavingWithoutGroupBy,
    /// `page()` was called with `page == 0`. Pages are 1-based; `0` is
    /// silently treated as page 1 by the wire `Pagination::resolve()`
    /// (`saturating_sub`), so it is almost always a caller mistake.
    InvalidPage {
        /// The offending (zero) page number.
        page: u64,
    },
    /// `page()` was called with `page_size == 0`. A zero-size page returns no
    /// rows and is almost always a caller mistake.
    InvalidPageSize {
        /// The offending (zero) page size.
        page_size: u64,
    },
}

impl std::fmt::Display for QueryBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryBuildError::HavingWithoutGroupBy => {
                write!(
                    f,
                    "having() requires a preceding group_by(); a HAVING filter without GROUP BY \
                     produces an empty-group query"
                )
            }
            QueryBuildError::InvalidPage { page } => {
                write!(
                    f,
                    "page() requires a 1-based page number, got {page}; the wire Pagination treats \
                     page=0 as page=1 (saturating_sub), so 0 is almost always a caller mistake"
                )
            }
            QueryBuildError::InvalidPageSize { page_size } => {
                write!(
                    f,
                    "page() requires a non-zero page size, got {page_size}; a zero-size page \
                     returns no rows"
                )
            }
        }
    }
}

impl std::error::Error for QueryBuildError {}
