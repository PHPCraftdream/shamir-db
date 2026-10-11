//! Resource categories for structured limit errors.

/// The resource whose effective execution limit was exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    TotalQueries,
    Work,
    Iterations,
    Depth,
    PageSize,
    Offset,
    ResultBytes,
}
