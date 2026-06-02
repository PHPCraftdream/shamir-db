//! Batch query types.
//!
//! Core types for batch request/response and execution planning.

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::admin::{
    AccessTreeOp, AddGroupMemberOp, AlterBufferConfigOp, ChgrpOp, ChmodOp, ChownOp,
    CommitMigrationOp, CreateDbOp, CreateGroupOp, CreateIndexOp, CreateRepoOp, CreateTableOp,
    DropDbOp, DropGroupOp, DropIndexOp, DropRepoOp, DropTableOp, GetBufferConfigOp, ListOp,
    MigrationStatusOp, RemoveGroupMemberOp, RollbackMigrationOp, SetBufferConfigOp,
    StartMigrationOp,
};
use crate::auth::{CreateRoleOp, CreateUserOp, DropRoleOp, DropUserOp, GrantRoleOp, RevokeRoleOp};
use crate::read::{QueryResult, ReadQuery};
use crate::write::{DeleteOp, InsertOp, SetOp, UpdateOp};
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
    SetBufferConfig(SetBufferConfigOp),
    GetBufferConfig(GetBufferConfigOp),
    AlterBufferConfig(AlterBufferConfigOp),
    List(ListOp),

    // Migration (online engine change)
    StartMigration(StartMigrationOp),
    CommitMigration(CommitMigrationOp),
    RollbackMigration(RollbackMigrationOp),
    MigrationStatus(MigrationStatusOp),

    // Auth operations
    CreateUser(CreateUserOp),
    DropUser(DropUserOp),
    CreateRole(CreateRoleOp),
    DropRole(DropRoleOp),
    GrantRole(GrantRoleOp),
    RevokeRole(RevokeRoleOp),

    // Access-control DDL (S3)
    Chmod(ChmodOp),
    Chown(ChownOp),
    Chgrp(ChgrpOp),
    CreateGroup(CreateGroupOp),
    DropGroup(DropGroupOp),
    AddGroupMember(AddGroupMemberOp),
    RemoveGroupMember(RemoveGroupMemberOp),

    /// Read-only access-control tree introspection.
    AccessTree(AccessTreeOp),
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
            BatchOp::SetBufferConfig(op) => op.serialize(serializer),
            BatchOp::GetBufferConfig(op) => op.serialize(serializer),
            BatchOp::AlterBufferConfig(op) => op.serialize(serializer),
            BatchOp::List(op) => op.serialize(serializer),
            BatchOp::StartMigration(op) => op.serialize(serializer),
            BatchOp::CommitMigration(op) => op.serialize(serializer),
            BatchOp::RollbackMigration(op) => op.serialize(serializer),
            BatchOp::MigrationStatus(op) => op.serialize(serializer),
            BatchOp::CreateUser(op) => op.serialize(serializer),
            BatchOp::DropUser(op) => op.serialize(serializer),
            BatchOp::CreateRole(op) => op.serialize(serializer),
            BatchOp::DropRole(op) => op.serialize(serializer),
            BatchOp::GrantRole(op) => op.serialize(serializer),
            BatchOp::RevokeRole(op) => op.serialize(serializer),
            BatchOp::Chmod(op) => op.serialize(serializer),
            BatchOp::Chown(op) => op.serialize(serializer),
            BatchOp::Chgrp(op) => op.serialize(serializer),
            BatchOp::CreateGroup(op) => op.serialize(serializer),
            BatchOp::DropGroup(op) => op.serialize(serializer),
            BatchOp::AddGroupMember(op) => op.serialize(serializer),
            BatchOp::RemoveGroupMember(op) => op.serialize(serializer),
            BatchOp::AccessTree(op) => op.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for BatchOp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let obj = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("BatchOp must be a JSON object"))?;

        // Dispatch by unique key
        if obj.contains_key("from") {
            serde_json::from_value(value)
                .map(BatchOp::Read)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("insert_into") {
            serde_json::from_value(value)
                .map(BatchOp::Insert)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("update") {
            serde_json::from_value(value)
                .map(BatchOp::Update)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("delete_from") {
            serde_json::from_value(value)
                .map(BatchOp::Delete)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_db") {
            serde_json::from_value(value)
                .map(BatchOp::CreateDb)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_db") {
            serde_json::from_value(value)
                .map(BatchOp::DropDb)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_repo") {
            serde_json::from_value(value)
                .map(BatchOp::CreateRepo)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_repo") {
            serde_json::from_value(value)
                .map(BatchOp::DropRepo)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_table") {
            serde_json::from_value(value)
                .map(BatchOp::CreateTable)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_table") {
            serde_json::from_value(value)
                .map(BatchOp::DropTable)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_index") {
            serde_json::from_value(value)
                .map(BatchOp::CreateIndex)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_index") {
            serde_json::from_value(value)
                .map(BatchOp::DropIndex)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("set_buffer_config") {
            serde_json::from_value(value)
                .map(BatchOp::SetBufferConfig)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("get_buffer_config") {
            serde_json::from_value(value)
                .map(BatchOp::GetBufferConfig)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("alter_buffer_config") {
            serde_json::from_value(value)
                .map(BatchOp::AlterBufferConfig)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("start_migration") {
            serde_json::from_value(value)
                .map(BatchOp::StartMigration)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("commit_migration") {
            serde_json::from_value(value)
                .map(BatchOp::CommitMigration)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("rollback_migration") {
            serde_json::from_value(value)
                .map(BatchOp::RollbackMigration)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("migration_status") {
            serde_json::from_value(value)
                .map(BatchOp::MigrationStatus)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_user") {
            serde_json::from_value(value)
                .map(BatchOp::CreateUser)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_user") {
            serde_json::from_value(value)
                .map(BatchOp::DropUser)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_role") {
            serde_json::from_value(value)
                .map(BatchOp::CreateRole)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_role") {
            serde_json::from_value(value)
                .map(BatchOp::DropRole)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("grant_role") {
            serde_json::from_value(value)
                .map(BatchOp::GrantRole)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("revoke_role") {
            serde_json::from_value(value)
                .map(BatchOp::RevokeRole)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("list") {
            serde_json::from_value(value)
                .map(BatchOp::List)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("chmod") {
            serde_json::from_value(value)
                .map(BatchOp::Chmod)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("chown") {
            serde_json::from_value(value)
                .map(BatchOp::Chown)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("chgrp") {
            serde_json::from_value(value)
                .map(BatchOp::Chgrp)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_group") {
            serde_json::from_value(value)
                .map(BatchOp::CreateGroup)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_group") {
            serde_json::from_value(value)
                .map(BatchOp::DropGroup)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("add_group_member") {
            serde_json::from_value(value)
                .map(BatchOp::AddGroupMember)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("remove_group_member") {
            serde_json::from_value(value)
                .map(BatchOp::RemoveGroupMember)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("access_tree") {
            serde_json::from_value(value)
                .map(BatchOp::AccessTree)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("set") {
            // "set" checked last because UpdateOp also has a "set" field
            serde_json::from_value(value)
                .map(BatchOp::Set)
                .map_err(serde::de::Error::custom)
        } else {
            Err(serde::de::Error::custom("Unknown operation type"))
        }
    }
}

impl BatchOp {
    /// Returns the table reference for data operations, None for admin ops.
    pub fn table_ref(&self) -> Option<&crate::TableRef> {
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
                | BatchOp::SetBufferConfig(_)
                | BatchOp::GetBufferConfig(_)
                | BatchOp::AlterBufferConfig(_)
                | BatchOp::List(_)
                | BatchOp::StartMigration(_)
                | BatchOp::CommitMigration(_)
                | BatchOp::RollbackMigration(_)
                | BatchOp::MigrationStatus(_)
                | BatchOp::CreateUser(_)
                | BatchOp::DropUser(_)
                | BatchOp::CreateRole(_)
                | BatchOp::DropRole(_)
                | BatchOp::GrantRole(_)
                | BatchOp::RevokeRole(_)
                | BatchOp::Chmod(_)
                | BatchOp::Chown(_)
                | BatchOp::Chgrp(_)
                | BatchOp::CreateGroup(_)
                | BatchOp::DropGroup(_)
                | BatchOp::AddGroupMember(_)
                | BatchOp::RemoveGroupMember(_)
                | BatchOp::AccessTree(_)
        )
    }
}

/// Returns the set of distinct repository names referenced by the
/// data queries in `queries`. Admin ops (which return `None` from
/// `BatchOp::table_ref`) do not contribute.
///
/// Used by the executor to enforce the cross-repo guard for
/// transactional batches (Stage 4.C).
pub fn distinct_repos(queries: &TMap<String, QueryEntry>) -> std::collections::HashSet<String> {
    queries
        .values()
        .filter_map(|qe| qe.op.table_ref().map(|tr| tr.repo.clone()))
        .collect()
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

    /// Requested isolation level for transactional batches.
    ///
    /// - `"snapshot"` (default) — Snapshot Isolation. Reads see a
    ///   consistent snapshot; writes use last-writer-wins.
    /// - `"serializable"` — Serializable Snapshot Isolation. Read-set
    ///   validated at commit; concurrent write conflict → abort.
    ///
    /// Ignored when `transactional` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,

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

/// Transaction metadata returned in batch responses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionInfo {
    /// Transaction ID (monotonic per repo).
    pub tx_id: u64,

    /// Outcome: `"committed"` or `"aborted"`.
    pub status: String,

    /// Human-readable abort reason (null when committed).
    /// E.g. `"tx_conflict"`, `"tx_cross_repo_not_supported"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// MVCC snapshot version the tx read from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_version: Option<u64>,

    /// Committed version (null when aborted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_version: Option<u64>,

    /// Whether the commit's projections (data → main store, counters,
    /// secondary indexes, HNSW graph) materialized inline on the commit
    /// path.
    ///
    /// - `true` (the common case): every projection landed before the
    ///   response — the committed version is fully observable now.
    /// - `false`: the commit is durable (its WAL entry IS the commit),
    ///   but at least one projection was deferred to crash-recovery,
    ///   which re-applies it idempotently on the next open. The data
    ///   WILL appear — this is restart-bounded eventual consistency, not
    ///   an abort.
    ///
    /// Only meaningful when `status == "committed"`. Defaults to `true`
    /// so payloads serialized before this field existed (and clients
    /// that omit it) deserialize to the fully-materialized common case.
    #[serde(default = "default_materialized")]
    pub materialized: bool,
}

fn default_materialized() -> bool {
    true
}

impl TransactionInfo {
    pub fn committed(
        tx_id: u64,
        snapshot_version: u64,
        commit_version: u64,
        materialized: bool,
    ) -> Self {
        Self {
            tx_id,
            status: "committed".into(),
            reason: None,
            snapshot_version: Some(snapshot_version),
            commit_version: Some(commit_version),
            materialized,
        }
    }

    pub fn aborted(tx_id: u64, reason: impl Into<String>) -> Self {
        Self {
            tx_id,
            status: "aborted".into(),
            reason: Some(reason.into()),
            snapshot_version: None,
            commit_version: None,
            // Aborted txs never materialize; default is irrelevant to the
            // client because `status` already disambiguates.
            materialized: true,
        }
    }

    pub fn is_committed(&self) -> bool {
        self.status == "committed"
    }
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

    /// Transactional batch targets more than one repository.
    ///
    /// 2PC across repos is intentionally out of scope. Clients must
    /// split such batches into separate single-repo transactions.
    CrossRepoNotSupported { repos: Vec<String> },
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
            BatchError::CrossRepoNotSupported { repos } => write!(
                f,
                "transactional batch targets multiple repositories ({}); single-repo only",
                repos.join(", ")
            ),
        }
    }
}

impl std::error::Error for BatchError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(json: &str) -> BatchOp {
        let op: BatchOp = serde_json::from_str(json).unwrap();
        let back = serde_json::to_string(&op).unwrap();
        let op2: BatchOp = serde_json::from_str(&back).unwrap();
        assert_eq!(op, op2);
        op
    }

    #[test]
    fn start_migration_serde() {
        let op = roundtrip(
            r#"{
            "start_migration": "users",
            "repo": "main",
            "dst_repo": "cold",
            "dst_engine": "redb",
            "dst_path": "/data/cold",
            "hmac": "deadbeef"
        }"#,
        );
        match &op {
            BatchOp::StartMigration(m) => {
                assert_eq!(m.start_migration, "users");
                assert_eq!(m.repo, "main");
                assert_eq!(m.dst_repo, "cold");
                assert_eq!(m.dst_engine, "redb");
                assert_eq!(m.dst_path.as_deref(), Some("/data/cold"));
                assert_eq!(m.hmac.as_deref(), Some("deadbeef"));
            }
            _ => panic!("expected StartMigration"),
        }
        assert!(op.is_admin());
        assert!(op.table_ref().is_none());
    }

    #[test]
    fn start_migration_defaults() {
        let op = roundtrip(
            r#"{
            "start_migration": "logs",
            "dst_repo": "archive",
            "dst_engine": "fjall"
        }"#,
        );
        match &op {
            BatchOp::StartMigration(m) => {
                assert_eq!(m.repo, "main");
                assert!(m.dst_path.is_none());
                assert!(m.hmac.is_none());
            }
            _ => panic!("expected StartMigration"),
        }
    }

    #[test]
    fn commit_migration_serde() {
        let op = roundtrip(r#"{"commit_migration": "mig-001", "hmac": "abcd1234"}"#);
        match &op {
            BatchOp::CommitMigration(m) => {
                assert_eq!(m.commit_migration, "mig-001");
                assert_eq!(m.hmac.as_deref(), Some("abcd1234"));
            }
            _ => panic!("expected CommitMigration"),
        }
        assert!(op.is_admin());
    }

    #[test]
    fn rollback_migration_serde() {
        let op = roundtrip(r#"{"rollback_migration": "mig-001", "hmac": "ff00"}"#);
        match &op {
            BatchOp::RollbackMigration(m) => {
                assert_eq!(m.rollback_migration, "mig-001");
                assert_eq!(m.hmac.as_deref(), Some("ff00"));
            }
            _ => panic!("expected RollbackMigration"),
        }
    }

    #[test]
    fn migration_status_serde() {
        let op = roundtrip(r#"{"migration_status": "mig-001"}"#);
        match &op {
            BatchOp::MigrationStatus(m) => assert_eq!(m.migration_status, "mig-001"),
            _ => panic!("expected MigrationStatus"),
        }
        assert!(op.is_admin());
    }

    #[test]
    fn batch_request_parses_isolation_field() {
        let json = serde_json::json!({
            "id": 1,
            "transactional": true,
            "isolation": "serializable",
            "queries": {}
        });
        let req: BatchRequest = serde_json::from_value(json).unwrap();
        assert!(req.transactional);
        assert_eq!(req.isolation, Some("serializable".to_string()));
    }

    #[test]
    fn batch_request_isolation_defaults_to_none() {
        let json = serde_json::json!({
            "id": 2,
            "transactional": true,
            "queries": {}
        });
        let req: BatchRequest = serde_json::from_value(json).unwrap();
        assert!(req.isolation.is_none());
    }

    #[test]
    fn transaction_info_committed_roundtrip() {
        let info = TransactionInfo::committed(42, 100, 105, true);
        assert!(info.is_committed());
        assert_eq!(info.tx_id, 42);
        assert_eq!(info.snapshot_version, Some(100));
        assert_eq!(info.commit_version, Some(105));
        assert!(info.materialized);

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["status"], "committed");
        assert_eq!(json["materialized"], true);
        assert!(json.get("reason").is_none()); // skip_serializing_if
    }

    #[test]
    fn transaction_info_aborted_roundtrip() {
        let info = TransactionInfo::aborted(7, "tx_conflict");
        assert!(!info.is_committed());
        assert_eq!(info.reason, Some("tx_conflict".to_string()));

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["status"], "aborted");
        assert_eq!(json["reason"], "tx_conflict");
    }

    #[test]
    fn transaction_info_deferred_materialization_roundtrip() {
        // A committed-but-deferred outcome must report materialized=false
        // and round-trip the flag through serde.
        let info = TransactionInfo::committed(9, 200, 201, false);
        assert!(info.is_committed());
        assert!(!info.materialized);

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["status"], "committed");
        assert_eq!(json["materialized"], false);

        let back: TransactionInfo = serde_json::from_value(json).unwrap();
        assert_eq!(back, info);
        assert!(!back.materialized);
    }

    #[test]
    fn transaction_info_missing_materialized_defaults_true() {
        // Backward-compat: a payload serialized before `materialized`
        // existed (field absent) must deserialize to the fully-applied
        // common case (materialized=true), not a deferred commit.
        let json = serde_json::json!({
            "tx_id": 5,
            "status": "committed",
            "snapshot_version": 100,
            "commit_version": 105
        });
        let info: TransactionInfo = serde_json::from_value(json).unwrap();
        assert!(info.is_committed());
        assert!(
            info.materialized,
            "absent `materialized` field must default to true"
        );
    }

    // ========================================================================
    // Access-control DDL ser/de round-trip (S3)
    // ========================================================================

    #[test]
    fn chmod_table_serde() {
        let op = roundtrip(
            r#"{
            "chmod": {
                "table": ["mydb", "main", "users"]
            },
            "mode": 448
        }"#,
        );
        match &op {
            BatchOp::Chmod(c) => {
                assert_eq!(c.mode, 0o700);
                match &c.chmod {
                    crate::admin::ResourceRef::Table { table } => {
                        assert_eq!(table, &["mydb", "main", "users"]);
                    }
                    _ => panic!("expected Table ResourceRef"),
                }
            }
            _ => panic!("expected Chmod"),
        }
        assert!(op.is_admin());
    }

    #[test]
    fn chown_database_serde() {
        let op = roundtrip(
            r#"{
            "chown": {
                "database": "testdb"
            },
            "owner": 7
        }"#,
        );
        match &op {
            BatchOp::Chown(c) => {
                assert_eq!(c.owner, 7);
                match &c.chown {
                    crate::admin::ResourceRef::Database { database } => {
                        assert_eq!(database, "testdb");
                    }
                    _ => panic!("expected Database ResourceRef"),
                }
            }
            _ => panic!("expected Chown"),
        }
    }

    #[test]
    fn chgrp_store_serde() {
        let op = roundtrip(
            r#"{
            "chgrp": {
                "store": ["testdb", "main"]
            },
            "group": 3
        }"#,
        );
        match &op {
            BatchOp::Chgrp(c) => {
                assert_eq!(c.group, Some(3));
            }
            _ => panic!("expected Chgrp"),
        }
    }

    #[test]
    fn chgrp_null_group_serde() {
        let op = roundtrip(
            r#"{
            "chgrp": {
                "database": "testdb"
            },
            "group": null
        }"#,
        );
        match &op {
            BatchOp::Chgrp(c) => {
                assert!(c.group.is_none());
            }
            _ => panic!("expected Chgrp"),
        }
    }

    #[test]
    fn create_group_serde() {
        let op = roundtrip(
            r#"{
            "create_group": "devs"
        }"#,
        );
        match &op {
            BatchOp::CreateGroup(c) => {
                assert_eq!(c.create_group, "devs");
            }
            _ => panic!("expected CreateGroup"),
        }
    }

    #[test]
    fn drop_group_by_name_serde() {
        let op = roundtrip(
            r#"{
            "drop_group": {
                "name": "devs"
            }
        }"#,
        );
        match &op {
            BatchOp::DropGroup(d) => match &d.drop_group {
                crate::admin::GroupRef::Name { name } => assert_eq!(name, "devs"),
                _ => panic!("expected Name GroupRef"),
            },
            _ => panic!("expected DropGroup"),
        }
    }

    #[test]
    fn drop_group_by_id_serde() {
        let op = roundtrip(
            r#"{
            "drop_group": {
                "id": 3
            }
        }"#,
        );
        match &op {
            BatchOp::DropGroup(d) => match &d.drop_group {
                crate::admin::GroupRef::Id { id } => assert_eq!(*id, 3),
                _ => panic!("expected Id GroupRef"),
            },
            _ => panic!("expected DropGroup"),
        }
    }

    #[test]
    fn add_group_member_serde() {
        let op = roundtrip(
            r#"{
            "add_group_member": {
                "name": "devs"
            },
            "user": 42
        }"#,
        );
        match &op {
            BatchOp::AddGroupMember(a) => {
                assert_eq!(a.user, 42);
            }
            _ => panic!("expected AddGroupMember"),
        }
    }

    #[test]
    fn remove_group_member_serde() {
        let op = roundtrip(
            r#"{
            "remove_group_member": {
                "id": 1
            },
            "user": 42
        }"#,
        );
        match &op {
            BatchOp::RemoveGroupMember(r) => {
                assert_eq!(r.user, 42);
            }
            _ => panic!("expected RemoveGroupMember"),
        }
    }

    #[test]
    fn chmod_function_namespace_serde() {
        let op = roundtrip(
            r#"{
            "chmod": {
                "function_namespace": true
            },
            "mode": 493
        }"#,
        );
        match &op {
            BatchOp::Chmod(c) => {
                assert_eq!(c.mode, 0o755);
                match &c.chmod {
                    crate::admin::ResourceRef::FunctionNamespace { .. } => {}
                    _ => panic!("expected FunctionNamespace ResourceRef"),
                }
            }
            _ => panic!("expected Chmod"),
        }
    }

    #[test]
    fn access_tree_serde() {
        let op = roundtrip(
            r#"{
            "access_tree": true,
            "depth": 2
        }"#,
        );
        match &op {
            BatchOp::AccessTree(a) => {
                assert!(a.access_tree);
                assert_eq!(a.depth, Some(2));
                assert!(a.db.is_none());
            }
            _ => panic!("expected AccessTree"),
        }
        assert!(op.is_admin());
        assert!(op.table_ref().is_none());
    }

    #[test]
    fn access_tree_defaults_serde() {
        let op = roundtrip(r#"{"access_tree": true}"#);
        match &op {
            BatchOp::AccessTree(a) => {
                assert!(a.access_tree);
                assert!(a.depth.is_none());
                assert!(a.db.is_none());
            }
            _ => panic!("expected AccessTree"),
        }
    }

    #[test]
    fn chown_function_serde() {
        let op = roundtrip(
            r#"{
            "chown": {
                "function": "my_fn"
            },
            "owner": 10
        }"#,
        );
        match &op {
            BatchOp::Chown(c) => match &c.chown {
                crate::admin::ResourceRef::Function { function } => {
                    assert_eq!(function, "my_fn");
                }
                _ => panic!("expected Function ResourceRef"),
            },
            _ => panic!("expected Chown"),
        }
    }
}
