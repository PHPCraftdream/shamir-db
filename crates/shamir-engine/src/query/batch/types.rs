//! Batch query types.
//!
//! Core types for batch request/response and execution planning.

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::query::admin::{
    CreateDbOp, CreateIndexOp, CreateRepoOp, CreateTableOp,
    DropDbOp, DropIndexOp, DropRepoOp, DropTableOp, ListOp,
};
use crate::query::auth::{
    CreateRoleOp, CreateUserOp, DropRoleOp, DropUserOp, GrantRoleOp, RevokeRoleOp,
};
use crate::query::read::{ReadQuery, QueryResult};
use crate::query::write::{DeleteOp, InsertOp, SetOp, UpdateOp};
use shamir_types::types::common::{TMap, TSet};

// ============================================================================
// BATCH OPERATION ENUM
// ============================================================================

/// Batch operation - can be a read or a write operation.
///
/// Detected by unique key in JSON object:
/// - `from` → Read
/// - `insert_into` → Insert
/// - `update` → Update (has `set` field too, but `update` is the discriminator)
/// - `set` (without `update`) → Set (upsert)
/// - `delete_from` → Delete
#[derive(Debug, Clone, PartialEq)]
pub enum BatchOp {
    /// Read query (SELECT).
    Read(ReadQuery),

    /// Insert new records.
    Insert(InsertOp),

    /// Update existing records.
    Update(UpdateOp),

    /// Upsert by key.
    Set(SetOp),

    /// Delete records.
    Delete(DeleteOp),

    // Admin (DDL) operations
    CreateDb(CreateDbOp),
    DropDb(DropDbOp),
    CreateRepo(CreateRepoOp),
    DropRepo(DropRepoOp),
    CreateTable(CreateTableOp),
    DropTable(DropTableOp),
    CreateIndex(CreateIndexOp),
    DropIndex(DropIndexOp),
    List(ListOp),

    // Auth operations
    CreateUser(CreateUserOp),
    DropUser(DropUserOp),
    CreateRole(CreateRoleOp),
    DropRole(DropRoleOp),
    GrantRole(GrantRoleOp),
    RevokeRole(RevokeRoleOp),
}

impl Serialize for BatchOp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            BatchOp::Read(q) => q.serialize(serializer),
            BatchOp::Insert(i) => i.serialize(serializer),
            BatchOp::Update(u) => u.serialize(serializer),
            BatchOp::Set(s) => s.serialize(serializer),
            BatchOp::Delete(d) => d.serialize(serializer),
            BatchOp::CreateDb(op) => op.serialize(serializer),
            BatchOp::DropDb(op) => op.serialize(serializer),
            BatchOp::CreateRepo(op) => op.serialize(serializer),
            BatchOp::DropRepo(op) => op.serialize(serializer),
            BatchOp::CreateTable(op) => op.serialize(serializer),
            BatchOp::DropTable(op) => op.serialize(serializer),
            BatchOp::CreateIndex(op) => op.serialize(serializer),
            BatchOp::DropIndex(op) => op.serialize(serializer),
            BatchOp::List(op) => op.serialize(serializer),
            BatchOp::CreateUser(op) => op.serialize(serializer),
            BatchOp::DropUser(op) => op.serialize(serializer),
            BatchOp::CreateRole(op) => op.serialize(serializer),
            BatchOp::DropRole(op) => op.serialize(serializer),
            BatchOp::GrantRole(op) => op.serialize(serializer),
            BatchOp::RevokeRole(op) => op.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for BatchOp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let obj = value.as_object().ok_or_else(|| {
            serde::de::Error::custom("BatchOp must be a JSON object")
        })?;

        // Dispatch by unique key
        if obj.contains_key("from") {
            serde_json::from_value(value).map(BatchOp::Read).map_err(serde::de::Error::custom)
        } else if obj.contains_key("insert_into") {
            serde_json::from_value(value).map(BatchOp::Insert).map_err(serde::de::Error::custom)
        } else if obj.contains_key("update") {
            serde_json::from_value(value).map(BatchOp::Update).map_err(serde::de::Error::custom)
        } else if obj.contains_key("delete_from") {
            serde_json::from_value(value).map(BatchOp::Delete).map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_db") {
            serde_json::from_value(value).map(BatchOp::CreateDb).map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_db") {
            serde_json::from_value(value).map(BatchOp::DropDb).map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_repo") {
            serde_json::from_value(value).map(BatchOp::CreateRepo).map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_repo") {
            serde_json::from_value(value).map(BatchOp::DropRepo).map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_table") {
            serde_json::from_value(value).map(BatchOp::CreateTable).map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_table") {
            serde_json::from_value(value).map(BatchOp::DropTable).map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_index") {
            serde_json::from_value(value).map(BatchOp::CreateIndex).map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_index") {
            serde_json::from_value(value).map(BatchOp::DropIndex).map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_user") {
            serde_json::from_value(value).map(BatchOp::CreateUser).map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_user") {
            serde_json::from_value(value).map(BatchOp::DropUser).map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_role") {
            serde_json::from_value(value).map(BatchOp::CreateRole).map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_role") {
            serde_json::from_value(value).map(BatchOp::DropRole).map_err(serde::de::Error::custom)
        } else if obj.contains_key("grant_role") {
            serde_json::from_value(value).map(BatchOp::GrantRole).map_err(serde::de::Error::custom)
        } else if obj.contains_key("revoke_role") {
            serde_json::from_value(value).map(BatchOp::RevokeRole).map_err(serde::de::Error::custom)
        } else if obj.contains_key("list") {
            serde_json::from_value(value).map(BatchOp::List).map_err(serde::de::Error::custom)
        } else if obj.contains_key("set") {
            // "set" checked last because UpdateOp also has a "set" field
            serde_json::from_value(value).map(BatchOp::Set).map_err(serde::de::Error::custom)
        } else {
            Err(serde::de::Error::custom(
                "Unknown operation type"
            ))
        }
    }
}

impl BatchOp {
    /// Returns the table reference for data operations, None for admin ops.
    pub fn table_ref(&self) -> Option<&crate::query::TableRef> {
        match self {
            BatchOp::Read(q) => Some(&q.from),
            BatchOp::Insert(i) => Some(&i.insert_into),
            BatchOp::Update(u) => Some(&u.update),
            BatchOp::Set(s) => Some(&s.set),
            BatchOp::Delete(d) => Some(&d.delete_from),
            _ => None,
        }
    }

    /// Returns true if this is an admin (DDL) operation.
    pub fn is_admin(&self) -> bool {
        matches!(
            self,
            BatchOp::CreateDb(_)
                | BatchOp::DropDb(_)
                | BatchOp::CreateRepo(_)
                | BatchOp::DropRepo(_)
                | BatchOp::CreateTable(_)
                | BatchOp::DropTable(_)
                | BatchOp::CreateIndex(_)
                | BatchOp::DropIndex(_)
                | BatchOp::List(_)
                | BatchOp::CreateUser(_)
                | BatchOp::DropUser(_)
                | BatchOp::CreateRole(_)
                | BatchOp::DropRole(_)
                | BatchOp::GrantRole(_)
                | BatchOp::RevokeRole(_)
        )
    }
}

impl From<ReadQuery> for BatchOp {
    fn from(q: ReadQuery) -> Self {
        BatchOp::Read(q)
    }
}

impl From<InsertOp> for BatchOp {
    fn from(i: InsertOp) -> Self {
        BatchOp::Insert(i)
    }
}

impl From<UpdateOp> for BatchOp {
    fn from(u: UpdateOp) -> Self {
        BatchOp::Update(u)
    }
}

impl From<SetOp> for BatchOp {
    fn from(s: SetOp) -> Self {
        BatchOp::Set(s)
    }
}

impl From<DeleteOp> for BatchOp {
    fn from(d: DeleteOp) -> Self {
        BatchOp::Delete(d)
    }
}

// ============================================================================
// QUERY ENTRY (updated to use BatchOp)
// ============================================================================

/// Operation entry for batch requests.
///
/// Used as the value in the `queries` map where the key is the alias.
///
/// # Examples
///
/// ```json
/// // Query
/// { "from": "users", "where": { "op": "eq", "field": "status", "value": "active" } }
///
/// // Insert
/// { "insert_into": "users", "values": [{ "name": "Alice" }] }
///
/// // Update
/// { "update": "users", "where": { "op": "eq", "field": "id", "value": 1 }, "set": { "name": "Bob" } }
///
/// // Set (upsert)
/// { "set": "users", "key": { "id": 1 }, "value": { "name": "Charlie" } }
///
/// // Delete
/// { "delete_from": "users", "where": { "op": "eq", "field": "id", "value": 1 } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryEntry {
    /// The operation to execute (flattened for shorthand syntax).
    #[serde(flatten)]
    pub op: BatchOp,

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

impl From<ReadQuery> for QueryEntry {
    fn from(query: ReadQuery) -> Self {
        QueryEntry {
            op: BatchOp::Read(query),
            return_result: true,
        }
    }
}

/// Batch request containing multiple queries.
///
/// # JSON Format
///
/// ```json
/// {
///   "name": "my_batch",
///   "transactional": false,
///   "queries": {
///     "users": { "from": "users" },
///     "orders": {
///       "query": { "from": "orders" },
///       "return_result": false
///     }
///   },
///   "return_all": true,
///   "return_only": ["users"],
///   "limits": { ... }
/// }
/// ```
///
/// # Fields
///
/// - `name`: Optional name for logging/debugging
/// - `transactional`: Enable MVCC transaction semantics
/// - `queries`: Map of alias -> query entry
/// - `return_all`: Return all results (default: true)
/// - `return_only`: Specific aliases to return (overrides return_all)
/// - `limits`: Security limits
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchRequest {
    /// Client-provided request ID, echoed back in the response.
    /// Used for correlating async requests with responses.
    pub id: serde_json::Value,

    /// Optional name for logging/debugging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Enable transactional semantics (MVCC).
    ///
    /// When true, all queries see a consistent snapshot.
    #[serde(default)]
    pub transactional: bool,

    /// Queries map: alias -> query entry.
    ///
    /// Each key is the alias used in `$query` references.
    /// The value can be just a `Query` or a `QueryEntry` with options.
    pub queries: TMap<String, QueryEntry>,

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
    /// Echoed request ID from BatchRequest.
    pub id: serde_json::Value,

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
