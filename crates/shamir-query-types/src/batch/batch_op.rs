//! [`BatchOp`] — the dispatch enum for all supported batch operations.

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::admin::{
    AccessTreeOp, AddGroupMemberOp, AddSchemaRuleOp, AlterBufferConfigOp, BindValidatorOp,
    ChangesSinceOp, ChgrpOp, ChmodOp, ChownOp, CommitMigrationOp, CreateDbOp,
    CreateFunctionFolderOp, CreateFunctionOp, CreateGroupOp, CreateIndexOp, CreateRepoOp,
    CreateTableOp, CreateValidatorOp, DescribeTableOp, DropDbOp, DropFunctionOp, DropGroupOp,
    DropIndexOp, DropRepoOp, DropTableOp, DropValidatorOp, GetBufferConfigOp, GetTableSchemaOp,
    InternerDumpOp, InternerTouchOp, ListOp, ListValidatorsOp, MigrationStatusOp, PurgeHistoryOp,
    RemoveGroupMemberOp, RemoveSchemaRuleOp, RenameFunctionOp, RenameIndexOp, RenameRepoOp,
    RenameTableOp, RenameValidatorOp, RollbackMigrationOp, SetBufferConfigOp, SetRetentionOp,
    SetTableSchemaOp, StartMigrationOp, UnbindValidatorOp,
};
use crate::auth::{CreateRoleOp, CreateUserOp, DropRoleOp, DropUserOp, GrantRoleOp, RevokeRoleOp};
use crate::call::CallOp;
use crate::read::ReadQuery;
use crate::subscribe::{SubscribeOp, UnsubscribeOp};
use crate::write::{DeleteOp, InsertOp, SetOp, UpdateOp};

use super::sub_batch_op::SubBatchOp;

/// Batch operation - can be a read or a write operation.
///
/// Detected by unique key in the wire map:
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
    RenameRepo(RenameRepoOp),
    CreateTable(CreateTableOp),
    DropTable(DropTableOp),
    RenameTable(RenameTableOp),
    CreateIndex(CreateIndexOp),
    DropIndex(DropIndexOp),
    RenameIndex(RenameIndexOp),
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

    // Declarative schema DDL (Phase A)
    SetTableSchema(SetTableSchemaOp),
    AddSchemaRule(AddSchemaRuleOp),
    RemoveSchemaRule(RemoveSchemaRuleOp),
    GetTableSchema(GetTableSchemaOp),

    /// Describe a table — full introspection (schema, indexes, validators,
    /// retention, buffer, access meta) in a single response.
    DescribeTable(DescribeTableOp),

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
            BatchOp::RenameRepo(op) => op.serialize(serializer),
            BatchOp::CreateTable(op) => op.serialize(serializer),
            BatchOp::DropTable(op) => op.serialize(serializer),
            BatchOp::RenameTable(op) => op.serialize(serializer),
            BatchOp::CreateIndex(op) => op.serialize(serializer),
            BatchOp::DropIndex(op) => op.serialize(serializer),
            BatchOp::RenameIndex(op) => op.serialize(serializer),
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
            BatchOp::SetTableSchema(op) => op.serialize(serializer),
            BatchOp::AddSchemaRule(op) => op.serialize(serializer),
            BatchOp::RemoveSchemaRule(op) => op.serialize(serializer),
            BatchOp::GetTableSchema(op) => op.serialize(serializer),
            BatchOp::DescribeTable(op) => op.serialize(serializer),
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
        // QueryValue deserialises natively from any serde format.
        use shamir_types::types::value::{QueryValue, Value};

        let qv = QueryValue::deserialize(deserializer)?;

        // Collect the set of top-level keys for dispatch. This is cheap:
        // just the key strings, not the values.
        let keys: Vec<String> = match &qv {
            Value::Map(m) => m.keys().cloned().collect(),
            _ => return Err(serde::de::Error::custom("BatchOp must be a map")),
        };
        let has = |k: &str| keys.iter().any(|s| s == k);

        // Re-encode the QueryValue through msgpack so that each typed-op
        // struct can use its own serde::Deserialize impl.  The msgpack
        // encoding is byte-identical to what the wire carries when the
        // caller is already on msgpack, and is a faithful round-trip for
        // test callers because QueryValue's Serialize is format-agnostic.
        let bytes = rmp_serde::to_vec_named(&qv).map_err(serde::de::Error::custom)?;

        /// Decode msgpack bytes into a typed op struct.
        fn qv_to<T: serde::de::DeserializeOwned, E: serde::de::Error>(
            bytes: &[u8],
        ) -> Result<T, E> {
            rmp_serde::from_slice(bytes).map_err(serde::de::Error::custom)
        }

        // Dispatch by unique key
        if has("from") {
            qv_to::<ReadQuery, _>(&bytes).map(BatchOp::Read)
        } else if has("insert_into") {
            qv_to::<InsertOp, _>(&bytes).map(BatchOp::Insert)
        } else if has("update") {
            qv_to::<UpdateOp, _>(&bytes).map(BatchOp::Update)
        } else if has("delete_from") {
            qv_to::<DeleteOp, _>(&bytes).map(BatchOp::Delete)
        } else if has("create_db") {
            qv_to::<CreateDbOp, _>(&bytes).map(BatchOp::CreateDb)
        } else if has("drop_db") {
            qv_to::<DropDbOp, _>(&bytes).map(BatchOp::DropDb)
        } else if has("create_repo") {
            qv_to::<CreateRepoOp, _>(&bytes).map(BatchOp::CreateRepo)
        } else if has("drop_repo") {
            qv_to::<DropRepoOp, _>(&bytes).map(BatchOp::DropRepo)
        } else if has("rename_repo") {
            qv_to::<RenameRepoOp, _>(&bytes).map(BatchOp::RenameRepo)
        } else if has("create_table") {
            qv_to::<CreateTableOp, _>(&bytes).map(BatchOp::CreateTable)
        } else if has("drop_table") {
            qv_to::<DropTableOp, _>(&bytes).map(BatchOp::DropTable)
        } else if has("rename_table") {
            qv_to::<RenameTableOp, _>(&bytes).map(BatchOp::RenameTable)
        } else if has("create_index") {
            qv_to::<CreateIndexOp, _>(&bytes).map(BatchOp::CreateIndex)
        } else if has("drop_index") {
            qv_to::<DropIndexOp, _>(&bytes).map(BatchOp::DropIndex)
        } else if has("rename_index") {
            qv_to::<RenameIndexOp, _>(&bytes).map(BatchOp::RenameIndex)
        } else if has("set_buffer_config") {
            qv_to::<SetBufferConfigOp, _>(&bytes).map(BatchOp::SetBufferConfig)
        } else if has("get_buffer_config") {
            qv_to::<GetBufferConfigOp, _>(&bytes).map(BatchOp::GetBufferConfig)
        } else if has("alter_buffer_config") {
            qv_to::<AlterBufferConfigOp, _>(&bytes).map(BatchOp::AlterBufferConfig)
        } else if has("start_migration") {
            qv_to::<StartMigrationOp, _>(&bytes).map(BatchOp::StartMigration)
        } else if has("commit_migration") {
            qv_to::<CommitMigrationOp, _>(&bytes).map(BatchOp::CommitMigration)
        } else if has("rollback_migration") {
            qv_to::<RollbackMigrationOp, _>(&bytes).map(BatchOp::RollbackMigration)
        } else if has("migration_status") {
            qv_to::<MigrationStatusOp, _>(&bytes).map(BatchOp::MigrationStatus)
        } else if has("create_user") {
            qv_to::<CreateUserOp, _>(&bytes).map(BatchOp::CreateUser)
        } else if has("drop_user") {
            qv_to::<DropUserOp, _>(&bytes).map(BatchOp::DropUser)
        } else if has("create_role") {
            qv_to::<CreateRoleOp, _>(&bytes).map(BatchOp::CreateRole)
        } else if has("drop_role") {
            qv_to::<DropRoleOp, _>(&bytes).map(BatchOp::DropRole)
        } else if has("grant_role") {
            qv_to::<GrantRoleOp, _>(&bytes).map(BatchOp::GrantRole)
        } else if has("revoke_role") {
            qv_to::<RevokeRoleOp, _>(&bytes).map(BatchOp::RevokeRole)
        } else if has("list") {
            qv_to::<ListOp, _>(&bytes).map(BatchOp::List)
        } else if has("chmod") {
            qv_to::<ChmodOp, _>(&bytes).map(BatchOp::Chmod)
        } else if has("chown") {
            qv_to::<ChownOp, _>(&bytes).map(BatchOp::Chown)
        } else if has("chgrp") {
            qv_to::<ChgrpOp, _>(&bytes).map(BatchOp::Chgrp)
        } else if has("create_group") {
            qv_to::<CreateGroupOp, _>(&bytes).map(BatchOp::CreateGroup)
        } else if has("drop_group") {
            qv_to::<DropGroupOp, _>(&bytes).map(BatchOp::DropGroup)
        } else if has("add_group_member") {
            qv_to::<AddGroupMemberOp, _>(&bytes).map(BatchOp::AddGroupMember)
        } else if has("remove_group_member") {
            qv_to::<RemoveGroupMemberOp, _>(&bytes).map(BatchOp::RemoveGroupMember)
        } else if has("access_tree") {
            qv_to::<AccessTreeOp, _>(&bytes).map(BatchOp::AccessTree)
        } else if has("create_function") {
            qv_to::<CreateFunctionOp, _>(&bytes).map(BatchOp::CreateFunction)
        } else if has("drop_function") {
            qv_to::<DropFunctionOp, _>(&bytes).map(BatchOp::DropFunction)
        } else if has("rename_function") {
            qv_to::<RenameFunctionOp, _>(&bytes).map(BatchOp::RenameFunction)
        } else if has("create_validator") {
            qv_to::<CreateValidatorOp, _>(&bytes).map(BatchOp::CreateValidator)
        } else if has("drop_validator") {
            qv_to::<DropValidatorOp, _>(&bytes).map(BatchOp::DropValidator)
        } else if has("rename_validator") {
            qv_to::<RenameValidatorOp, _>(&bytes).map(BatchOp::RenameValidator)
        } else if has("bind_validator") {
            qv_to::<BindValidatorOp, _>(&bytes).map(BatchOp::BindValidator)
        } else if has("unbind_validator") {
            qv_to::<UnbindValidatorOp, _>(&bytes).map(BatchOp::UnbindValidator)
        } else if has("list_validators") {
            qv_to::<ListValidatorsOp, _>(&bytes).map(BatchOp::ListValidators)
        } else if has("set_table_schema") {
            qv_to::<SetTableSchemaOp, _>(&bytes).map(BatchOp::SetTableSchema)
        } else if has("add_schema_rule") {
            qv_to::<AddSchemaRuleOp, _>(&bytes).map(BatchOp::AddSchemaRule)
        } else if has("remove_schema_rule") {
            qv_to::<RemoveSchemaRuleOp, _>(&bytes).map(BatchOp::RemoveSchemaRule)
        } else if has("describe_table") {
            qv_to::<DescribeTableOp, _>(&bytes).map(BatchOp::DescribeTable)
        } else if has("get_table_schema") {
            qv_to::<GetTableSchemaOp, _>(&bytes).map(BatchOp::GetTableSchema)
        } else if has("create_function_folder") {
            qv_to::<CreateFunctionFolderOp, _>(&bytes).map(BatchOp::CreateFunctionFolder)
        } else if has("interner_dump") {
            qv_to::<InternerDumpOp, _>(&bytes).map(BatchOp::InternerDump)
        } else if has("interner_touch") {
            qv_to::<InternerTouchOp, _>(&bytes).map(BatchOp::InternerTouch)
        } else if has("purge_history") {
            qv_to::<PurgeHistoryOp, _>(&bytes).map(BatchOp::PurgeHistory)
        } else if has("set_retention") {
            qv_to::<SetRetentionOp, _>(&bytes).map(BatchOp::SetRetention)
        } else if has("changes_since") {
            qv_to::<ChangesSinceOp, _>(&bytes).map(BatchOp::ChangesSince)
        } else if has("call") {
            qv_to::<CallOp, _>(&bytes).map(BatchOp::Call)
        } else if has("batch") {
            qv_to::<SubBatchOp, _>(&bytes).map(BatchOp::Batch)
        } else if has("subscribe") {
            qv_to::<SubscribeOp, _>(&bytes).map(BatchOp::Subscribe)
        } else if has("unsubscribe") {
            qv_to::<UnsubscribeOp, _>(&bytes).map(BatchOp::Unsubscribe)
        } else if has("set") {
            // "set" checked last because UpdateOp also has a "set" field
            qv_to::<SetOp, _>(&bytes).map(BatchOp::Set)
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
                | BatchOp::RenameRepo(_)
                | BatchOp::CreateTable(_)
                | BatchOp::DropTable(_)
                | BatchOp::RenameTable(_)
                | BatchOp::CreateIndex(_)
                | BatchOp::DropIndex(_)
                | BatchOp::RenameIndex(_)
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
                | BatchOp::SetTableSchema(_)
                | BatchOp::AddSchemaRule(_)
                | BatchOp::RemoveSchemaRule(_)
                | BatchOp::GetTableSchema(_)
                | BatchOp::DescribeTable(_)
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
