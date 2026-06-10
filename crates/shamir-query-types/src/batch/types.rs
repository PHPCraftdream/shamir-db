//! Batch query types.
//!
//! Core types for batch request/response and execution planning.

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::admin::{
    AccessTreeOp, AddGroupMemberOp, AlterBufferConfigOp, BindValidatorOp, ChangesSinceOp, ChgrpOp,
    ChmodOp, ChownOp, CommitMigrationOp, CreateDbOp, CreateFunctionFolderOp, CreateFunctionOp,
    CreateGroupOp, CreateIndexOp, CreateRepoOp, CreateTableOp, CreateValidatorOp, DropDbOp,
    DropFunctionOp, DropGroupOp, DropIndexOp, DropRepoOp, DropTableOp, DropValidatorOp,
    GetBufferConfigOp, ListOp, ListValidatorsOp, MigrationStatusOp, PurgeHistoryOp,
    RemoveGroupMemberOp, RenameFunctionOp, RenameValidatorOp, RollbackMigrationOp,
    SetBufferConfigOp, SetRetentionOp, StartMigrationOp, UnbindValidatorOp,
};
use crate::auth::{CreateRoleOp, CreateUserOp, DropRoleOp, DropUserOp, GrantRoleOp, RevokeRoleOp};
use crate::call::CallOp;
use crate::filter::FilterValue;
use crate::read::{QueryResult, ReadQuery};
use crate::write::{DeleteOp, InsertOp, SetOp, UpdateOp};
use shamir_collections::{TMap, TSet};

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
#[allow(clippy::large_enum_variant)] // dispatch enum: rarely on the stack; boxing cascades through the engine
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

    // Function DDL (DDL-A)
    CreateFunction(CreateFunctionOp),
    DropFunction(DropFunctionOp),
    RenameFunction(RenameFunctionOp),

    // Validator DDL (DDL-A)
    CreateValidator(CreateValidatorOp),
    DropValidator(DropValidatorOp),
    RenameValidator(RenameValidatorOp),
    BindValidator(BindValidatorOp),
    UnbindValidator(UnbindValidatorOp),
    ListValidators(ListValidatorsOp),

    // Function folder DDL
    CreateFunctionFolder(CreateFunctionFolderOp),

    /// Imperative history purge (temporal T2).
    PurgeHistory(PurgeHistoryOp),

    /// Change a live table's history-retention policy (temporal T3).
    SetRetention(SetRetentionOp),

    /// One-shot "changes since version V" journal read (temporal T4-changes-since).
    ChangesSince(ChangesSinceOp),

    /// Stored procedure / callable function invocation.
    Call(CallOp),

    /// Nested sub-batch — recursive execution with its own tx scope.
    Batch(SubBatchOp),
}

/// A sub-batch — a nested BatchRequest with explicit parameter bindings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubBatchOp {
    pub batch: BatchRequest,
    #[serde(default, skip_serializing_if = "TMap::is_empty")]
    pub bind: TMap<String, FilterValue>,
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
            BatchOp::CreateFunction(op) => op.serialize(serializer),
            BatchOp::DropFunction(op) => op.serialize(serializer),
            BatchOp::RenameFunction(op) => op.serialize(serializer),
            BatchOp::CreateValidator(op) => op.serialize(serializer),
            BatchOp::DropValidator(op) => op.serialize(serializer),
            BatchOp::RenameValidator(op) => op.serialize(serializer),
            BatchOp::BindValidator(op) => op.serialize(serializer),
            BatchOp::UnbindValidator(op) => op.serialize(serializer),
            BatchOp::ListValidators(op) => op.serialize(serializer),
            BatchOp::CreateFunctionFolder(op) => op.serialize(serializer),
            BatchOp::PurgeHistory(op) => op.serialize(serializer),
            BatchOp::SetRetention(op) => op.serialize(serializer),
            BatchOp::ChangesSince(op) => op.serialize(serializer),
            BatchOp::Call(op) => op.serialize(serializer),
            BatchOp::Batch(op) => op.serialize(serializer),
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
        } else if obj.contains_key("create_function") {
            serde_json::from_value(value)
                .map(BatchOp::CreateFunction)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_function") {
            serde_json::from_value(value)
                .map(BatchOp::DropFunction)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("rename_function") {
            serde_json::from_value(value)
                .map(BatchOp::RenameFunction)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_validator") {
            serde_json::from_value(value)
                .map(BatchOp::CreateValidator)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("drop_validator") {
            serde_json::from_value(value)
                .map(BatchOp::DropValidator)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("rename_validator") {
            serde_json::from_value(value)
                .map(BatchOp::RenameValidator)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("bind_validator") {
            serde_json::from_value(value)
                .map(BatchOp::BindValidator)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("unbind_validator") {
            serde_json::from_value(value)
                .map(BatchOp::UnbindValidator)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("list_validators") {
            serde_json::from_value(value)
                .map(BatchOp::ListValidators)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("create_function_folder") {
            serde_json::from_value(value)
                .map(BatchOp::CreateFunctionFolder)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("purge_history") {
            serde_json::from_value(value)
                .map(BatchOp::PurgeHistory)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("set_retention") {
            serde_json::from_value(value)
                .map(BatchOp::SetRetention)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("changes_since") {
            serde_json::from_value(value)
                .map(BatchOp::ChangesSince)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("call") {
            serde_json::from_value(value)
                .map(BatchOp::Call)
                .map_err(serde::de::Error::custom)
        } else if obj.contains_key("batch") {
            serde_json::from_value(value)
                .map(BatchOp::Batch)
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
            BatchOp::Batch(_) => None,
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
                | BatchOp::CreateFunction(_)
                | BatchOp::DropFunction(_)
                | BatchOp::RenameFunction(_)
                | BatchOp::CreateValidator(_)
                | BatchOp::DropValidator(_)
                | BatchOp::RenameValidator(_)
                | BatchOp::BindValidator(_)
                | BatchOp::UnbindValidator(_)
                | BatchOp::ListValidators(_)
                | BatchOp::CreateFunctionFolder(_)
                | BatchOp::PurgeHistory(_)
                | BatchOp::SetRetention(_)
                | BatchOp::ChangesSince(_)
                | BatchOp::Batch(_)
        )
    }
}

/// Returns the set of distinct repository names referenced by the
/// data queries in `queries`. Admin ops (which return `none` from
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

    /// Explicit ordering dependencies: aliases (in this batch) that MUST
    /// execute before this entry. Complements the auto-extracted `$query`
    /// dependencies. Enables DDL→DML ordering (e.g. insert after create_table).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
}

fn default_return() -> bool {
    true
}

impl From<ReadQuery> for QueryEntry {
    fn from(query: ReadQuery) -> Self {
        QueryEntry {
            op: BatchOp::Read(query),
            return_result: true,
            after: Vec::new(),
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

    /// Per-request durability level.
    ///
    /// - `"buffered"` (default / absent) — ack after the in-memory
    ///   MemBuffer; durability on the ~500 ms background tick or
    ///   graceful drain.
    /// - `"synced"` — before ack, flush the durable backing of every
    ///   repo this batch touched, so a committed write survives even
    ///   an immediate hard crash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durability: Option<String>,

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

    /// Maximum sub-batch nesting depth. 0 = no nesting allowed.
    pub max_nesting_depth: usize,
}

impl Default for BatchLimits {
    fn default() -> Self {
        BatchLimits {
            max_queries: 50,
            max_dependency_depth: 10,
            max_execution_time_secs: 30,
            max_result_size: 10 * 1024 * 1024, // 10MB
            max_nesting_depth: 4,
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
