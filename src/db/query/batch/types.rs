//! Batch query types.
//!
//! Core types for batch request/response and execution planning.

use serde::{Deserialize, Serialize};

use crate::db::query::read::{Query, QueryResult};
use crate::types::common::{TMap, TSet};

/// Named query with alias for result referencing.
///
/// Each query in a batch has a unique alias that can be referenced
/// by other queries using the `$query` syntax.
///
/// # Example
///
/// ```json
/// {
///   "alias": "users",
///   "query": { "from": "users" },
///   "return_result": true
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedQuery {
    /// Unique alias for referencing results.
    ///
    /// Must be alphanumeric with underscores, unique within the batch.
    /// Used in `$query` references like `@users[0].id`.
    pub alias: String,

    /// The query to execute.
    pub query: Query,

    /// Whether to include this result in the response.
    ///
    /// - `true` (default): Include in `results`
    /// - `false`: Exclude (useful for intermediate queries)
    #[serde(default = "default_return")]
    pub return_result: bool,
}

fn default_return() -> bool {
    true
}

/// Batch request containing multiple queries.
///
/// # JSON Format
///
/// ```json
/// {
///   "name": "my_batch",
///   "transactional": false,
///   "queries": [...],
///   "return_all": true,
///   "return_only": ["users", "orders"],
///   "limits": { ... }
/// }
/// ```
///
/// # Fields
///
/// - `name`: Optional name for logging/debugging
/// - `transactional`: Enable MVCC transaction semantics
/// - `queries`: Array of named queries
/// - `return_all`: Return all results (default: true)
/// - `return_only`: Specific aliases to return (overrides return_all)
/// - `limits`: Security limits
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchRequest {
    /// Optional name for logging/debugging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Enable transactional semantics (MVCC).
    ///
    /// When true, all queries see a consistent snapshot.
    #[serde(default)]
    pub transactional: bool,

    /// Queries with aliases.
    pub queries: Vec<NamedQuery>,

    /// Return all results (default: true).
    #[serde(default = "default_return_all")]
    pub return_all: bool,

    /// Specific aliases to return (overrides return_all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_only: Option<Vec<String>>,

    /// Execution limits (security).
    #[serde(default = "BatchLimits::default")]
    pub limits: BatchLimits,
}

fn default_return_all() -> bool {
    true
}

/// Batch response with results.
///
/// # JSON Format
///
/// ```json
/// {
///   "results": {
///     "users": [...],
///     "orders": [...]
///   },
///   "execution_plan": [["users", "products"], ["orders"], ["stats"]],
///   "execution_time_us": 1234,
///   "transaction": { "id": 1, "committed": true }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchResponse {
    /// Results by alias.
    #[serde(default)]
    pub results: TMap<String, QueryResult>,

    /// Execution plan (for debugging).
    ///
    /// Each inner array contains queries that run in parallel.
    pub execution_plan: Vec<Vec<String>>,

    /// Total execution time in microseconds.
    pub execution_time_us: u64,

    /// Transaction info (if transactional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction: Option<TransactionInfo>,
}

/// Transaction metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionInfo {
    /// Transaction ID.
    pub id: u64,
    /// Whether commit succeeded.
    pub committed: bool,
}

/// Execution limits for security.
///
/// Prevents DoS attacks and resource exhaustion.
///
/// # Default Values
///
/// | Limit | Default | Description |
/// |-------|---------|-------------|
/// | `max_queries` | 50 | Maximum queries per batch |
/// | `max_dependency_depth` | 10 | Maximum dependency chain length |
/// | `max_execution_time_secs` | 30 | Maximum total execution time |
/// | `max_result_size` | 10MB | Maximum total result size |
///
/// # Example
///
/// ```json
/// {
///   "limits": {
///     "max_queries": 20,
///     "max_dependency_depth": 5,
///     "max_execution_time_secs": 10,
///     "max_result_size": 1000000
///   }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchLimits {
    /// Maximum number of queries in batch.
    pub max_queries: usize,

    /// Maximum dependency depth.
    ///
    /// A chain like a -> b -> c has depth 2.
    pub max_dependency_depth: usize,

    /// Maximum total execution time (seconds).
    pub max_execution_time_secs: u64,

    /// Maximum result size (bytes).
    pub max_result_size: usize,
}

impl Default for BatchLimits {
    fn default() -> Self {
        BatchLimits {
            max_queries: 50,
            max_dependency_depth: 10,
            max_execution_time_secs: 30,
            max_result_size: 10 * 1024 * 1024, // 10MB
        }
    }
}

/// Execution plan with parallel stages.
///
/// The planner analyzes dependencies and creates stages where
/// each stage contains queries that can run in parallel.
///
/// # Example
///
/// For queries with dependencies:
/// - `users` (no deps)
/// - `products` (no deps)
/// - `orders` (depends on users, products)
/// - `stats` (depends on orders)
///
/// The plan would be:
/// ```text
/// stages: [[users, products], [orders], [stats]]
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct BatchPlan {
    /// Stages: each stage contains queries that can run in parallel.
    pub stages: Vec<Vec<String>>,

    /// All aliases in order.
    pub aliases: Vec<String>,

    /// Dependency graph (alias -> dependencies).
    pub dependencies: TMap<String, TSet<String>>,
}

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

    /// Duplicate alias in batch.
    ///
    /// Each alias must be unique within a batch.
    DuplicateAlias { alias: String },

    /// Execution timeout.
    ///
    /// Total execution time exceeded `BatchLimits::max_execution_time_secs`.
    Timeout { elapsed_secs: u64 },

    /// Query execution error.
    QueryError { alias: String, message: String },

    /// Lock timeout (deadlock prevention).
    ///
    /// Could not acquire locks within the timeout period.
    LockTimeout { aliases: Vec<String> },
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
            BatchError::DuplicateAlias { alias } => {
                write!(f, "Duplicate alias: '{}'", alias)
            }
            BatchError::Timeout { elapsed_secs } => {
                write!(f, "Execution timeout after {}s", elapsed_secs)
            }
            BatchError::QueryError { alias, message } => {
                write!(f, "Query '{}' failed: {}", alias, message)
            }
            BatchError::LockTimeout { aliases } => {
                write!(f, "Lock timeout for queries: {}", aliases.join(", "))
            }
        }
    }
}

impl std::error::Error for BatchError {}
