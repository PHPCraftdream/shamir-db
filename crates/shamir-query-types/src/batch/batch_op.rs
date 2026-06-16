//! [`BatchOp`] — the dispatch enum for all supported batch operations.

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::admin::{
    AccessTreeOp, AddGroupMemberOp, AlterBufferConfigOp, BindValidatorOp, ChangesSinceOp, ChgrpOp,
    ChmodOp, ChownOp, CommitMigrationOp, CreateDbOp, CreateFunctionFolderOp, CreateFunctionOp,
    CreateGroupOp, CreateIndexOp, CreateRepoOp, CreateTableOp, CreateValidatorOp, DropDbOp,
    DropFunctionOp, DropGroupOp, DropIndexOp, DropRepoOp, DropTableOp, DropValidatorOp,
    GetBufferConfigOp, InternerDumpOp, InternerTouchOp, ListOp, ListValidatorsOp,
    MigrationStatusOp, PurgeHistoryOp, RemoveGroupMemberOp, RenameFunctionOp, RenameValidatorOp,
    RollbackMigrationOp, SetBufferConfigOp, SetRetentionOp, StartMigrationOp, UnbindValidatorOp,
};
use crate::auth::{CreateRoleOp, CreateUserOp, DropRoleOp, DropUserOp, GrantRoleOp, RevokeRoleOp};
use crate::call::CallOp;
use crate::read::ReadQuery;
use crate::subscribe::{SubscribeOp, UnsubscribeOp};
use crate::write::{DeleteOp, InsertOp, SetOp, UpdateOp};

use super::sub_batch_op::SubBatchOp;

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

    // Per-repo interner introspection / registration (Stage 5d)
    InternerDump(InternerDumpOp),
    InternerTouch(InternerTouchOp),

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

    /// Subscribe to table change events.
    Subscribe(SubscribeOp),

    /// Cancel an active subscription.
    Unsubscribe(UnsubscribeOp),
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
            BatchOp::InternerDump(op) => op.serialize(serializer),
            BatchOp::InternerTouch(op) => op.serialize(serializer),
            BatchOp::PurgeHistory(op) => op.serialize(serializer),
            BatchOp::SetRetention(op) => op.serialize(serializer),
            BatchOp::ChangesSince(op) => op.serialize(serializer),
            BatchOp::Call(op) => op.serialize(serializer),
            BatchOp::Batch(op) => op.serialize(serializer),
            BatchOp::Subscribe(op) => op.serialize(serializer),
            BatchOp::Unsubscribe(op) => op.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for BatchOp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Buffer the map as a format-agnostic QueryValue first.
        // When the wire format is msgpack this avoids materialising a
        // serde_json::Value tree — QueryValue deserialises natively
        // from any serde format.
        use shamir_types::types::value::{QueryValue, Value};

        let qv = QueryValue::deserialize(deserializer)?;

        // Collect the set of top-level keys for dispatch. This is cheap:
        // just the key strings, not the values.
        let keys: Vec<String> = match &qv {
            Value::Map(m) => m.keys().cloned().collect(),
            _ => return Err(serde::de::Error::custom("BatchOp must be a map")),
        };
        let has = |k: &str| keys.iter().any(|s| s == k);

        // Convert to serde_json::Value for the from_value dispatch.
        // For write-path ops (Insert, Update, Set) the inner types
        // now carry QueryValue fields — from_value invokes
        // QueryValue::deserialize which is a single-pass tree walk.
        let value = serde_json::Value::from(qv);

        // Dispatch by unique key
        if has("from") {
            serde_json::from_value(value)
                .map(BatchOp::Read)
                .map_err(serde::de::Error::custom)
        } else if has("insert_into") {
            serde_json::from_value(value)
                .map(BatchOp::Insert)
                .map_err(serde::de::Error::custom)
        } else if has("update") {
            serde_json::from_value(value)
                .map(BatchOp::Update)
                .map_err(serde::de::Error::custom)
        } else if has("delete_from") {
            serde_json::from_value(value)
                .map(BatchOp::Delete)
                .map_err(serde::de::Error::custom)
        } else if has("create_db") {
            serde_json::from_value(value)
                .map(BatchOp::CreateDb)
                .map_err(serde::de::Error::custom)
        } else if has("drop_db") {
            serde_json::from_value(value)
                .map(BatchOp::DropDb)
                .map_err(serde::de::Error::custom)
        } else if has("create_repo") {
            serde_json::from_value(value)
                .map(BatchOp::CreateRepo)
                .map_err(serde::de::Error::custom)
        } else if has("drop_repo") {
            serde_json::from_value(value)
                .map(BatchOp::DropRepo)
                .map_err(serde::de::Error::custom)
        } else if has("create_table") {
            serde_json::from_value(value)
                .map(BatchOp::CreateTable)
                .map_err(serde::de::Error::custom)
        } else if has("drop_table") {
            serde_json::from_value(value)
                .map(BatchOp::DropTable)
                .map_err(serde::de::Error::custom)
        } else if has("create_index") {
            serde_json::from_value(value)
                .map(BatchOp::CreateIndex)
                .map_err(serde::de::Error::custom)
        } else if has("drop_index") {
            serde_json::from_value(value)
                .map(BatchOp::DropIndex)
                .map_err(serde::de::Error::custom)
        } else if has("set_buffer_config") {
            serde_json::from_value(value)
                .map(BatchOp::SetBufferConfig)
                .map_err(serde::de::Error::custom)
        } else if has("get_buffer_config") {
            serde_json::from_value(value)
                .map(BatchOp::GetBufferConfig)
                .map_err(serde::de::Error::custom)
        } else if has("alter_buffer_config") {
            serde_json::from_value(value)
                .map(BatchOp::AlterBufferConfig)
                .map_err(serde::de::Error::custom)
        } else if has("start_migration") {
            serde_json::from_value(value)
                .map(BatchOp::StartMigration)
                .map_err(serde::de::Error::custom)
        } else if has("commit_migration") {
            serde_json::from_value(value)
                .map(BatchOp::CommitMigration)
                .map_err(serde::de::Error::custom)
        } else if has("rollback_migration") {
            serde_json::from_value(value)
                .map(BatchOp::RollbackMigration)
                .map_err(serde::de::Error::custom)
        } else if has("migration_status") {
            serde_json::from_value(value)
                .map(BatchOp::MigrationStatus)
                .map_err(serde::de::Error::custom)
        } else if has("create_user") {
            serde_json::from_value(value)
                .map(BatchOp::CreateUser)
                .map_err(serde::de::Error::custom)
        } else if has("drop_user") {
            serde_json::from_value(value)
                .map(BatchOp::DropUser)
                .map_err(serde::de::Error::custom)
        } else if has("create_role") {
            serde_json::from_value(value)
                .map(BatchOp::CreateRole)
                .map_err(serde::de::Error::custom)
        } else if has("drop_role") {
            serde_json::from_value(value)
                .map(BatchOp::DropRole)
                .map_err(serde::de::Error::custom)
        } else if has("grant_role") {
            serde_json::from_value(value)
                .map(BatchOp::GrantRole)
                .map_err(serde::de::Error::custom)
        } else if has("revoke_role") {
            serde_json::from_value(value)
                .map(BatchOp::RevokeRole)
                .map_err(serde::de::Error::custom)
        } else if has("list") {
            serde_json::from_value(value)
                .map(BatchOp::List)
                .map_err(serde::de::Error::custom)
        } else if has("chmod") {
            serde_json::from_value(value)
                .map(BatchOp::Chmod)
                .map_err(serde::de::Error::custom)
        } else if has("chown") {
            serde_json::from_value(value)
                .map(BatchOp::Chown)
                .map_err(serde::de::Error::custom)
        } else if has("chgrp") {
            serde_json::from_value(value)
                .map(BatchOp::Chgrp)
                .map_err(serde::de::Error::custom)
        } else if has("create_group") {
            serde_json::from_value(value)
                .map(BatchOp::CreateGroup)
                .map_err(serde::de::Error::custom)
        } else if has("drop_group") {
            serde_json::from_value(value)
                .map(BatchOp::DropGroup)
                .map_err(serde::de::Error::custom)
        } else if has("add_group_member") {
            serde_json::from_value(value)
                .map(BatchOp::AddGroupMember)
                .map_err(serde::de::Error::custom)
        } else if has("remove_group_member") {
            serde_json::from_value(value)
                .map(BatchOp::RemoveGroupMember)
                .map_err(serde::de::Error::custom)
        } else if has("access_tree") {
            serde_json::from_value(value)
                .map(BatchOp::AccessTree)
                .map_err(serde::de::Error::custom)
        } else if has("create_function") {
            serde_json::from_value(value)
                .map(BatchOp::CreateFunction)
                .map_err(serde::de::Error::custom)
        } else if has("drop_function") {
            serde_json::from_value(value)
                .map(BatchOp::DropFunction)
                .map_err(serde::de::Error::custom)
        } else if has("rename_function") {
            serde_json::from_value(value)
                .map(BatchOp::RenameFunction)
                .map_err(serde::de::Error::custom)
        } else if has("create_validator") {
            serde_json::from_value(value)
                .map(BatchOp::CreateValidator)
                .map_err(serde::de::Error::custom)
        } else if has("drop_validator") {
            serde_json::from_value(value)
                .map(BatchOp::DropValidator)
                .map_err(serde::de::Error::custom)
        } else if has("rename_validator") {
            serde_json::from_value(value)
                .map(BatchOp::RenameValidator)
                .map_err(serde::de::Error::custom)
        } else if has("bind_validator") {
            serde_json::from_value(value)
                .map(BatchOp::BindValidator)
                .map_err(serde::de::Error::custom)
        } else if has("unbind_validator") {
            serde_json::from_value(value)
                .map(BatchOp::UnbindValidator)
                .map_err(serde::de::Error::custom)
        } else if has("list_validators") {
            serde_json::from_value(value)
                .map(BatchOp::ListValidators)
                .map_err(serde::de::Error::custom)
        } else if has("create_function_folder") {
            serde_json::from_value(value)
                .map(BatchOp::CreateFunctionFolder)
                .map_err(serde::de::Error::custom)
        } else if has("interner_dump") {
            serde_json::from_value(value)
                .map(BatchOp::InternerDump)
                .map_err(serde::de::Error::custom)
        } else if has("interner_touch") {
            serde_json::from_value(value)
                .map(BatchOp::InternerTouch)
                .map_err(serde::de::Error::custom)
        } else if has("purge_history") {
            serde_json::from_value(value)
                .map(BatchOp::PurgeHistory)
                .map_err(serde::de::Error::custom)
        } else if has("set_retention") {
            serde_json::from_value(value)
                .map(BatchOp::SetRetention)
                .map_err(serde::de::Error::custom)
        } else if has("changes_since") {
            serde_json::from_value(value)
                .map(BatchOp::ChangesSince)
                .map_err(serde::de::Error::custom)
        } else if has("call") {
            serde_json::from_value(value)
                .map(BatchOp::Call)
                .map_err(serde::de::Error::custom)
        } else if has("batch") {
            serde_json::from_value(value)
                .map(BatchOp::Batch)
                .map_err(serde::de::Error::custom)
        } else if has("subscribe") {
            serde_json::from_value(value)
                .map(BatchOp::Subscribe)
                .map_err(serde::de::Error::custom)
        } else if has("unsubscribe") {
            serde_json::from_value(value)
                .map(BatchOp::Unsubscribe)
                .map_err(serde::de::Error::custom)
        } else if has("set") {
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
            BatchOp::Batch(_) | BatchOp::Subscribe(_) | BatchOp::Unsubscribe(_) => None,
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
                | BatchOp::InternerDump(_)
                | BatchOp::InternerTouch(_)
                | BatchOp::PurgeHistory(_)
                | BatchOp::SetRetention(_)
                | BatchOp::ChangesSince(_)
                | BatchOp::Batch(_)
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
