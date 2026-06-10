//! [`BatchError`] — errors that can occur during batch processing.

/// Errors during batch processing.
#[derive(Debug, Clone, PartialEq)]
pub enum BatchError {
    /// Too many queries in batch.
    ///
    /// Check `BatchLimits::max_queries`.
    TooManyQueries { count: usize, max: usize },

    /// Circular dependency detected.
    ///
    /// The `cycle` field contains the cycle path, e.g., `["a", "b", "c", "a"]`.
    CircularDependency { cycle: Vec<String> },

    /// Dependency depth exceeded.
    ///
    /// Check `BatchLimits::max_dependency_depth`.
    TooDeep { depth: usize, max: usize },

    /// Unknown alias referenced.
    ///
    /// A query referenced an alias that doesn't exist in the batch.
    UnknownAlias {
        alias: String,
        referenced_by: String,
    },

    /// Execution timeout.
    ///
    /// Total execution time exceeded `BatchLimits::max_execution_time_secs`.
    Timeout { elapsed_secs: u64 },

    /// Query execution error.
    ///
    /// `code` carries a machine-readable error category when available
    /// (e.g. `"exists"`, `"not_found"`, `"access_denied"`,
    /// `"still_referenced"`).  Unclassified errors leave it `none`.
    QueryError {
        alias: String,
        message: String,
        #[doc(hidden)]
        code: Option<String>,
    },

    /// Lock timeout (deadlock prevention).
    ///
    /// Could not acquire locks within the timeout period.
    LockTimeout { aliases: Vec<String> },

    /// Transactional batch targets more than one repository.
    ///
    /// 2PC across repos is intentionally out of scope. Clients must
    /// split such batches into separate single-repo transactions.
    CrossRepoNotSupported { repos: Vec<String> },

    /// Static sub-batch nesting depth exceeded.
    ///
    /// The op tree contains `BatchOp::Batch` nodes nested deeper than
    /// `BatchLimits::max_nesting_depth`.
    NestingTooDeep { depth: usize, max: usize },
}

impl std::fmt::Display for BatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BatchError::TooManyQueries { count, max } => {
                write!(f, "Too many queries: {} (max: {})", count, max)
            }
            BatchError::CircularDependency { cycle } => {
                write!(f, "Circular dependency: {}", cycle.join(" -> "))
            }
            BatchError::TooDeep { depth, max } => {
                write!(f, "Dependency depth too deep: {} (max: {})", depth, max)
            }
            BatchError::UnknownAlias {
                alias,
                referenced_by,
            } => {
                write!(
                    f,
                    "Unknown alias '{}' referenced by '{}'",
                    alias, referenced_by
                )
            }
            BatchError::Timeout { elapsed_secs } => {
                write!(f, "Execution timeout after {}s", elapsed_secs)
            }
            BatchError::QueryError {
                alias,
                message,
                code,
            } => {
                if let Some(c) = code {
                    write!(f, "Query '{}' failed [{}]: {}", alias, c, message)
                } else {
                    write!(f, "Query '{}' failed: {}", alias, message)
                }
            }
            BatchError::LockTimeout { aliases } => {
                write!(f, "Lock timeout for queries: {}", aliases.join(", "))
            }
            BatchError::CrossRepoNotSupported { repos } => write!(
                f,
                "transactional batch targets multiple repositories ({}); single-repo only",
                repos.join(", ")
            ),
            BatchError::NestingTooDeep { depth, max } => {
                write!(f, "Sub-batch nesting too deep: {} (max: {})", depth, max)
            }
        }
    }
}

impl std::error::Error for BatchError {}

impl BatchError {
    /// Structured DDL/admin error with a machine-readable `code`.
    pub fn query_coded(
        alias: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        BatchError::QueryError {
            alias: alias.into(),
            message: message.into(),
            code: Some(code.into()),
        }
    }

    /// Return the machine-readable code, if set.
    pub fn code(&self) -> Option<&str> {
        match self {
            BatchError::QueryError { code, .. } => code.as_deref(),
            _ => None,
        }
    }
}
