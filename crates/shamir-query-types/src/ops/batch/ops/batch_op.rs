//! [`BatchOp`] — the dispatch enum for all supported batch operations.

use serde::de::{Deserializer, Error as DeError, MapAccess, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use shamir_types::access::{Action, ResourcePath};
use shamir_types::types::common::TMap;
use shamir_types::types::value::{QueryValue, Value};

use crate::admin::{
    AccessTreeOp, AddGroupMemberOp, AddSchemaRuleOp, AlterBufferConfigOp, AlterRepoOp,
    AlterSubscriptionOp, BindValidatorOp, ChangesSinceOp, ChgrpOp, ChmodOp, ChownOp,
    CommitMigrationOp, CreateDbOp, CreateFunctionFolderOp, CreateFunctionOp, CreateGroupOp,
    CreateIndexOp, CreatePublicationOp, CreateReplicationProfileOp, CreateRepoOp,
    CreateSubscriptionOp, CreateTableOp, CreateValidatorOp, DescribeRepoOp, DescribeTableOp,
    DropDbOp, DropFunctionOp, DropGroupOp, DropIndexOp, DropPublicationOp,
    DropReplicationProfileOp, DropRepoOp, DropSubscriptionOp, DropTableOp, DropValidatorOp,
    GetBufferConfigOp, GetTableSchemaOp, InternerDumpOp, InternerTouchOp, ListOp,
    ListPublicationsOp, ListSubscriptionsOp, ListValidatorsOp, MigrationStatusOp, PurgeHistoryOp,
    RemoveGroupMemberOp, RemoveSchemaRuleOp, RenameDbOp, RenameFunctionFolderOp, RenameFunctionOp,
    RenameGroupOp, RenameIndexOp, RenameRepoOp, RenameTableOp, RenameValidatorOp,
    ReplicationStatusOp, RollbackMigrationOp, SetBufferConfigOp, SetRetentionOp, SetTableSchemaOp,
    StartMigrationOp, UnbindValidatorOp,
};
use crate::auth::{CreateUserOp, DropUserOp, GrantRoleOp, RevokeRoleOp};
use crate::call::CallOp;
use crate::read::ReadQuery;
use crate::subscribe::{SubscribeOp, UnsubscribeOp};
use crate::write::{DeleteOp, InsertOp, SetOp, UpdateOp};

use super::op_kind::{op_kind, OpKind};
use super::query_value_deserializer::QueryValueDeserializer;

use crate::batch::for_each_op::ForEachOp;
use crate::batch::req::options::assert_nested_batch_caps;
use crate::batch::sub_batch_op::SubBatchOp;
use crate::batch::tree_visitor::BatchTreeVisitor;

/// Batch operation - can be a read or a write operation.
///
/// Untrusted request bytes must pass `validate_request_alias_caps` before typed
/// decoding: nested batch aliases may collapse while buffering into QueryValue.
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
    /// Alter a repository's per-repo settings (durability override patch).
    AlterRepo(AlterRepoOp),
    RenameDb(RenameDbOp),
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
    GrantRole(GrantRoleOp),
    RevokeRole(RevokeRoleOp),

    // Access-control DDL (S3)
    Chmod(ChmodOp),
    Chown(ChownOp),
    Chgrp(ChgrpOp),
    CreateGroup(CreateGroupOp),
    DropGroup(DropGroupOp),
    RenameGroup(RenameGroupOp),
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

    /// Describe a repository — engine, path, per-repo durability policy.
    DescribeRepo(DescribeRepoOp),

    // Function folder DDL
    CreateFunctionFolder(CreateFunctionFolderOp),
    RenameFunctionFolder(RenameFunctionFolderOp),

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

    /// Data-dependent loop — recursive execution of a nested batch, once
    /// per element of `over` (Epic04, #653).
    ForEach(ForEachOp),

    /// Subscribe to table change events.
    Subscribe(SubscribeOp),

    /// Cancel an active subscription.
    Unsubscribe(UnsubscribeOp),

    // Replication DDL — publication/subscription model (REPLICATION.md §5.5).
    // These are pure wire/serde DTOs: classification lives here, execution is
    // a separate concern (R1-loop integration). Mutating ops (create/drop/
    // alter) are write-classified; the list/status ops are read-only.
    CreateReplicationProfile(CreateReplicationProfileOp),
    DropReplicationProfile(DropReplicationProfileOp),
    CreatePublication(CreatePublicationOp),
    DropPublication(DropPublicationOp),
    CreateSubscription(CreateSubscriptionOp),
    DropSubscription(DropSubscriptionOp),
    AlterSubscription(AlterSubscriptionOp),
    /// List all publications defined on this node (read-only).
    ListPublications(ListPublicationsOp),
    /// List all subscriptions defined on this node (read-only).
    ListSubscriptions(ListSubscriptionsOp),
    /// Inspect runtime replication status (read-only).
    ReplicationStatus(ReplicationStatusOp),
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
            BatchOp::AlterRepo(op) => op.serialize(serializer),
            BatchOp::RenameDb(op) => op.serialize(serializer),
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
            BatchOp::GrantRole(op) => op.serialize(serializer),
            BatchOp::RevokeRole(op) => op.serialize(serializer),
            BatchOp::Chmod(op) => op.serialize(serializer),
            BatchOp::Chown(op) => op.serialize(serializer),
            BatchOp::Chgrp(op) => op.serialize(serializer),
            BatchOp::CreateGroup(op) => op.serialize(serializer),
            BatchOp::DropGroup(op) => op.serialize(serializer),
            BatchOp::RenameGroup(op) => op.serialize(serializer),
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
            BatchOp::DescribeRepo(op) => op.serialize(serializer),
            BatchOp::CreateFunctionFolder(op) => op.serialize(serializer),
            BatchOp::RenameFunctionFolder(op) => op.serialize(serializer),
            BatchOp::InternerDump(op) => op.serialize(serializer),
            BatchOp::InternerTouch(op) => op.serialize(serializer),
            BatchOp::PurgeHistory(op) => op.serialize(serializer),
            BatchOp::SetRetention(op) => op.serialize(serializer),
            BatchOp::ChangesSince(op) => op.serialize(serializer),
            BatchOp::Call(op) => op.serialize(serializer),
            BatchOp::Batch(op) => op.serialize(serializer),
            BatchOp::ForEach(op) => op.serialize(serializer),
            BatchOp::Subscribe(op) => op.serialize(serializer),
            BatchOp::Unsubscribe(op) => op.serialize(serializer),
            BatchOp::CreateReplicationProfile(op) => op.serialize(serializer),
            BatchOp::DropReplicationProfile(op) => op.serialize(serializer),
            BatchOp::CreatePublication(op) => op.serialize(serializer),
            BatchOp::DropPublication(op) => op.serialize(serializer),
            BatchOp::CreateSubscription(op) => op.serialize(serializer),
            BatchOp::DropSubscription(op) => op.serialize(serializer),
            BatchOp::AlterSubscription(op) => op.serialize(serializer),
            BatchOp::ListPublications(op) => op.serialize(serializer),
            BatchOp::ListSubscriptions(op) => op.serialize(serializer),
            BatchOp::ReplicationStatus(op) => op.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for BatchOp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OpVisitor;
        impl<'de> Visitor<'de> for OpVisitor {
            type Value = BatchOp;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map carrying exactly one batch operation")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<BatchOp, A::Error> {
                let mut qv_map = TMap::default();
                while let Some((key, value)) = map.next_entry::<String, QueryValue>()? {
                    qv_map.insert(key, value);
                }
                let qv = QueryValue::Map(qv_map);
                assert_nested_batch_caps(&qv).map_err(DeError::custom)?;
                let Value::Map(m) = &qv else {
                    return Err(DeError::custom("BatchOp must be a map"));
                };
                let mut selected = None;
                for kind in m.keys().filter_map(|key| op_kind(key)) {
                    selected = Some(match (selected, kind) {
                        (None, kind) => kind,
                        (Some(OpKind::Update), OpKind::Set)
                        | (Some(OpKind::Set), OpKind::Update) => OpKind::Update,
                        _ => return Err(DeError::custom("ambiguous BatchOp discriminators")),
                    });
                }
                let kind = selected.ok_or_else(|| DeError::custom("Unknown operation type"))?;
                let value = QueryValueDeserializer(qv);
                match kind {
                    OpKind::Read => ReadQuery::deserialize(value).map(BatchOp::Read),
                    OpKind::Insert => InsertOp::deserialize(value).map(BatchOp::Insert),
                    OpKind::Update => UpdateOp::deserialize(value).map(BatchOp::Update),
                    OpKind::Delete => DeleteOp::deserialize(value).map(BatchOp::Delete),
                    OpKind::CreateDb => CreateDbOp::deserialize(value).map(BatchOp::CreateDb),
                    OpKind::DropDb => DropDbOp::deserialize(value).map(BatchOp::DropDb),
                    OpKind::CreateRepo => CreateRepoOp::deserialize(value).map(BatchOp::CreateRepo),
                    OpKind::DropRepo => DropRepoOp::deserialize(value).map(BatchOp::DropRepo),
                    OpKind::RenameRepo => RenameRepoOp::deserialize(value).map(BatchOp::RenameRepo),
                    OpKind::AlterRepo => AlterRepoOp::deserialize(value).map(BatchOp::AlterRepo),
                    OpKind::RenameDb => RenameDbOp::deserialize(value).map(BatchOp::RenameDb),
                    OpKind::CreateTable => {
                        CreateTableOp::deserialize(value).map(BatchOp::CreateTable)
                    }
                    OpKind::DropTable => DropTableOp::deserialize(value).map(BatchOp::DropTable),
                    OpKind::RenameTable => {
                        RenameTableOp::deserialize(value).map(BatchOp::RenameTable)
                    }
                    OpKind::CreateIndex => {
                        CreateIndexOp::deserialize(value).map(BatchOp::CreateIndex)
                    }
                    OpKind::DropIndex => DropIndexOp::deserialize(value).map(BatchOp::DropIndex),
                    OpKind::RenameIndex => {
                        RenameIndexOp::deserialize(value).map(BatchOp::RenameIndex)
                    }
                    OpKind::SetBufferConfig => {
                        SetBufferConfigOp::deserialize(value).map(BatchOp::SetBufferConfig)
                    }
                    OpKind::GetBufferConfig => {
                        GetBufferConfigOp::deserialize(value).map(BatchOp::GetBufferConfig)
                    }
                    OpKind::AlterBufferConfig => {
                        AlterBufferConfigOp::deserialize(value).map(BatchOp::AlterBufferConfig)
                    }
                    OpKind::StartMigration => {
                        StartMigrationOp::deserialize(value).map(BatchOp::StartMigration)
                    }
                    OpKind::CommitMigration => {
                        CommitMigrationOp::deserialize(value).map(BatchOp::CommitMigration)
                    }
                    OpKind::RollbackMigration => {
                        RollbackMigrationOp::deserialize(value).map(BatchOp::RollbackMigration)
                    }
                    OpKind::MigrationStatus => {
                        MigrationStatusOp::deserialize(value).map(BatchOp::MigrationStatus)
                    }
                    OpKind::CreateUser => CreateUserOp::deserialize(value).map(BatchOp::CreateUser),
                    OpKind::DropUser => DropUserOp::deserialize(value).map(BatchOp::DropUser),
                    OpKind::GrantRole => GrantRoleOp::deserialize(value).map(BatchOp::GrantRole),
                    OpKind::RevokeRole => RevokeRoleOp::deserialize(value).map(BatchOp::RevokeRole),
                    OpKind::List => ListOp::deserialize(value).map(BatchOp::List),
                    OpKind::Chmod => ChmodOp::deserialize(value).map(BatchOp::Chmod),
                    OpKind::Chown => ChownOp::deserialize(value).map(BatchOp::Chown),
                    OpKind::Chgrp => ChgrpOp::deserialize(value).map(BatchOp::Chgrp),
                    OpKind::CreateGroup => {
                        CreateGroupOp::deserialize(value).map(BatchOp::CreateGroup)
                    }
                    OpKind::DropGroup => DropGroupOp::deserialize(value).map(BatchOp::DropGroup),
                    OpKind::RenameGroup => {
                        RenameGroupOp::deserialize(value).map(BatchOp::RenameGroup)
                    }
                    OpKind::AddGroupMember => {
                        AddGroupMemberOp::deserialize(value).map(BatchOp::AddGroupMember)
                    }
                    OpKind::RemoveGroupMember => {
                        RemoveGroupMemberOp::deserialize(value).map(BatchOp::RemoveGroupMember)
                    }
                    OpKind::AccessTree => AccessTreeOp::deserialize(value).map(BatchOp::AccessTree),
                    OpKind::CreateFunction => {
                        CreateFunctionOp::deserialize(value).map(BatchOp::CreateFunction)
                    }
                    OpKind::DropFunction => {
                        DropFunctionOp::deserialize(value).map(BatchOp::DropFunction)
                    }
                    OpKind::RenameFunction => {
                        RenameFunctionOp::deserialize(value).map(BatchOp::RenameFunction)
                    }
                    OpKind::CreateValidator => {
                        CreateValidatorOp::deserialize(value).map(BatchOp::CreateValidator)
                    }
                    OpKind::DropValidator => {
                        DropValidatorOp::deserialize(value).map(BatchOp::DropValidator)
                    }
                    OpKind::RenameValidator => {
                        RenameValidatorOp::deserialize(value).map(BatchOp::RenameValidator)
                    }
                    OpKind::BindValidator => {
                        BindValidatorOp::deserialize(value).map(BatchOp::BindValidator)
                    }
                    OpKind::UnbindValidator => {
                        UnbindValidatorOp::deserialize(value).map(BatchOp::UnbindValidator)
                    }
                    OpKind::ListValidators => {
                        ListValidatorsOp::deserialize(value).map(BatchOp::ListValidators)
                    }
                    OpKind::SetTableSchema => {
                        SetTableSchemaOp::deserialize(value).map(BatchOp::SetTableSchema)
                    }
                    OpKind::AddSchemaRule => {
                        AddSchemaRuleOp::deserialize(value).map(BatchOp::AddSchemaRule)
                    }
                    OpKind::RemoveSchemaRule => {
                        RemoveSchemaRuleOp::deserialize(value).map(BatchOp::RemoveSchemaRule)
                    }
                    OpKind::GetTableSchema => {
                        GetTableSchemaOp::deserialize(value).map(BatchOp::GetTableSchema)
                    }
                    OpKind::DescribeTable => {
                        DescribeTableOp::deserialize(value).map(BatchOp::DescribeTable)
                    }
                    OpKind::DescribeRepo => {
                        DescribeRepoOp::deserialize(value).map(BatchOp::DescribeRepo)
                    }
                    OpKind::CreateFunctionFolder => CreateFunctionFolderOp::deserialize(value)
                        .map(BatchOp::CreateFunctionFolder),
                    OpKind::RenameFunctionFolder => RenameFunctionFolderOp::deserialize(value)
                        .map(BatchOp::RenameFunctionFolder),
                    OpKind::InternerDump => {
                        InternerDumpOp::deserialize(value).map(BatchOp::InternerDump)
                    }
                    OpKind::InternerTouch => {
                        InternerTouchOp::deserialize(value).map(BatchOp::InternerTouch)
                    }
                    OpKind::PurgeHistory => {
                        PurgeHistoryOp::deserialize(value).map(BatchOp::PurgeHistory)
                    }
                    OpKind::SetRetention => {
                        SetRetentionOp::deserialize(value).map(BatchOp::SetRetention)
                    }
                    OpKind::ChangesSince => {
                        ChangesSinceOp::deserialize(value).map(BatchOp::ChangesSince)
                    }
                    OpKind::CreateReplicationProfile => {
                        CreateReplicationProfileOp::deserialize(value)
                            .map(BatchOp::CreateReplicationProfile)
                    }
                    OpKind::DropReplicationProfile => DropReplicationProfileOp::deserialize(value)
                        .map(BatchOp::DropReplicationProfile),
                    OpKind::CreatePublication => {
                        CreatePublicationOp::deserialize(value).map(BatchOp::CreatePublication)
                    }
                    OpKind::DropPublication => {
                        DropPublicationOp::deserialize(value).map(BatchOp::DropPublication)
                    }
                    OpKind::CreateSubscription => {
                        CreateSubscriptionOp::deserialize(value).map(BatchOp::CreateSubscription)
                    }
                    OpKind::DropSubscription => {
                        DropSubscriptionOp::deserialize(value).map(BatchOp::DropSubscription)
                    }
                    OpKind::AlterSubscription => {
                        AlterSubscriptionOp::deserialize(value).map(BatchOp::AlterSubscription)
                    }
                    OpKind::ListPublications => {
                        ListPublicationsOp::deserialize(value).map(BatchOp::ListPublications)
                    }
                    OpKind::ListSubscriptions => {
                        ListSubscriptionsOp::deserialize(value).map(BatchOp::ListSubscriptions)
                    }
                    OpKind::ReplicationStatus => {
                        ReplicationStatusOp::deserialize(value).map(BatchOp::ReplicationStatus)
                    }
                    OpKind::Call => CallOp::deserialize(value).map(BatchOp::Call),
                    OpKind::ForEach => ForEachOp::deserialize(value).map(BatchOp::ForEach),
                    OpKind::Batch => SubBatchOp::deserialize(value).map(BatchOp::Batch),
                    OpKind::Subscribe => SubscribeOp::deserialize(value).map(BatchOp::Subscribe),
                    OpKind::Unsubscribe => {
                        UnsubscribeOp::deserialize(value).map(BatchOp::Unsubscribe)
                    }
                    OpKind::Set => SetOp::deserialize(value).map(BatchOp::Set),
                }
                .map_err(DeError::custom)
            }
        }

        deserializer.deserialize_map(OpVisitor)
    }
}

impl BatchOp {
    /// Returns the `(Action, ResourcePath)` this op must be authorized
    /// against, for ops with a `table_ref()`. `None` for admin/DDL ops
    /// (authorized separately in `execute_admin`) and read-only/
    /// introspection ops with no target table.
    ///
    /// This is the single source of truth for the per-op DML authorization
    /// mapping that was previously duplicated (byte-for-byte) in
    /// `shamir-db`'s `execute_as` and `tx_execute_as` — both now call this
    /// method instead of re-deriving the `Action` inline. Like
    /// [`is_write`](Self::is_write), this is an **exhaustive `match` with
    /// no wildcard arm**: adding a new `BatchOp` variant that carries a
    /// `table_ref()` will fail to compile here until the author explicitly
    /// classifies its `Action` — the old duplicated code's `_ =>
    /// Action::Write` wildcard silently swallowed that decision instead.
    ///
    /// The per-variant `Action` choice mirrors `is_write`'s existing
    /// read/write classification: `Read` → `Action::Read`; `Insert` →
    /// `Action::Create`; `Set`/`Update` → `Action::Write`; `Delete` →
    /// `Action::Delete`. Every other variant either has no `table_ref()`
    /// (admin/DDL, `Batch`, `Subscribe`, `Unsubscribe`) and is unreachable
    /// here, or is listed explicitly below for completeness even though
    /// `table_ref()` already returns `None` for it — this keeps the two
    /// matches mechanically in sync so a future change to `table_ref()`
    /// that starts returning `Some` for one of them is forced to pick an
    /// `Action` here too.
    pub fn required_access(&self, db: &str) -> Option<(Action, ResourcePath)> {
        let action = match self {
            // ----- data ops: the classification duplicated pre-fix -------
            BatchOp::Read(_) => Action::Read,
            BatchOp::Insert(_) => Action::Create,
            BatchOp::Set(_) | BatchOp::Update(_) => Action::Write,
            BatchOp::Delete(_) => Action::Delete,

            // ----- everything else has no table_ref(): unreachable below,
            // but listed explicitly (no wildcard) so a new BatchOp variant
            // forces a conscious choice here too. -------------------------
            BatchOp::CreateDb(_)
            | BatchOp::DropDb(_)
            | BatchOp::CreateRepo(_)
            | BatchOp::DropRepo(_)
            | BatchOp::RenameRepo(_)
            | BatchOp::AlterRepo(_)
            | BatchOp::RenameDb(_)
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
            | BatchOp::GrantRole(_)
            | BatchOp::RevokeRole(_)
            | BatchOp::Chmod(_)
            | BatchOp::Chown(_)
            | BatchOp::Chgrp(_)
            | BatchOp::CreateGroup(_)
            | BatchOp::DropGroup(_)
            | BatchOp::RenameGroup(_)
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
            | BatchOp::DescribeRepo(_)
            | BatchOp::CreateFunctionFolder(_)
            | BatchOp::RenameFunctionFolder(_)
            | BatchOp::InternerDump(_)
            | BatchOp::InternerTouch(_)
            | BatchOp::PurgeHistory(_)
            | BatchOp::SetRetention(_)
            | BatchOp::ChangesSince(_)
            | BatchOp::Call(_)
            | BatchOp::Batch(_)
            | BatchOp::ForEach(_)
            | BatchOp::Subscribe(_)
            | BatchOp::Unsubscribe(_)
            | BatchOp::CreateReplicationProfile(_)
            | BatchOp::DropReplicationProfile(_)
            | BatchOp::CreatePublication(_)
            | BatchOp::DropPublication(_)
            | BatchOp::CreateSubscription(_)
            | BatchOp::DropSubscription(_)
            | BatchOp::AlterSubscription(_)
            | BatchOp::ListPublications(_)
            | BatchOp::ListSubscriptions(_)
            | BatchOp::ReplicationStatus(_) => return None,
        };
        let tref = self.table_ref()?;
        Some((
            action,
            ResourcePath::Table {
                db: db.to_string(),
                store: tref.repo.clone(),
                table: tref.table.clone(),
            },
        ))
    }

    /// Returns the table reference for data operations, None for admin ops.
    pub fn table_ref(&self) -> Option<&crate::TableRef> {
        match self {
            BatchOp::Read(q) => Some(&q.from),
            BatchOp::Insert(i) => Some(&i.insert_into),
            BatchOp::Update(u) => Some(&u.update),
            BatchOp::Set(s) => Some(&s.set),
            BatchOp::Delete(d) => Some(&d.delete_from),
            BatchOp::Batch(_)
            | BatchOp::ForEach(_)
            | BatchOp::Subscribe(_)
            | BatchOp::Unsubscribe(_) => None,
            _ => None,
        }
    }

    /// Returns true if this leaf or any descendant leaf is an admin (DDL) op.
    pub fn is_admin(&self) -> bool {
        if let Some(queries) = BatchTreeVisitor::body_queries(self) {
            return BatchTreeVisitor::new(queries).any(|visit| visit.entry.op.is_admin_leaf());
        }
        self.is_admin_leaf()
    }

    fn is_admin_leaf(&self) -> bool {
        matches!(
            self,
            BatchOp::CreateDb(_)
                | BatchOp::DropDb(_)
                | BatchOp::CreateRepo(_)
                | BatchOp::DropRepo(_)
                | BatchOp::RenameRepo(_)
                | BatchOp::AlterRepo(_)
                | BatchOp::RenameDb(_)
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
                | BatchOp::GrantRole(_)
                | BatchOp::RevokeRole(_)
                | BatchOp::Chmod(_)
                | BatchOp::Chown(_)
                | BatchOp::Chgrp(_)
                | BatchOp::CreateGroup(_)
                | BatchOp::DropGroup(_)
                | BatchOp::RenameGroup(_)
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
                | BatchOp::DescribeRepo(_)
                | BatchOp::CreateFunctionFolder(_)
                | BatchOp::RenameFunctionFolder(_)
                | BatchOp::InternerDump(_)
                | BatchOp::InternerTouch(_)
                | BatchOp::PurgeHistory(_)
                | BatchOp::SetRetention(_)
                | BatchOp::ChangesSince(_)
                | BatchOp::CreateReplicationProfile(_)
                | BatchOp::DropReplicationProfile(_)
                | BatchOp::CreatePublication(_)
                | BatchOp::DropPublication(_)
                | BatchOp::CreateSubscription(_)
                | BatchOp::DropSubscription(_)
                | BatchOp::AlterSubscription(_)
                | BatchOp::ListPublications(_)
                | BatchOp::ListSubscriptions(_)
                | BatchOp::ReplicationStatus(_)
        )
    }

    /// Returns true if this operation mutates data or persistent server state.
    ///
    /// This is the gate used by read-only follower nodes (REPLICATION §4.3)
    /// to reject writes: any op that is *not* a pure read/introspection is
    /// classified as a write. `Call` (WASM stored-procedure invocation) is
    /// conservatively treated as a write, since a host function may mutate
    /// state.
    ///
    /// Unlike [`is_admin`](Self::is_admin) (which uses `matches!`), this is
    /// an **exhaustive `match` with no wildcard arm** — adding a new
    /// `BatchOp` variant will break compilation here and force the author
    /// to explicitly classify it as read or write.
    pub fn is_write(&self) -> bool {
        if let Some(queries) = BatchTreeVisitor::body_queries(self) {
            return BatchTreeVisitor::new(queries).any(|visit| visit.entry.op.is_write_leaf());
        }
        self.is_write_leaf()
    }

    pub(crate) fn is_write_leaf(&self) -> bool {
        match self {
            // ----- pure reads / introspection (write = false) -------------
            BatchOp::Read(_) => false,
            BatchOp::List(_) => false,
            BatchOp::GetBufferConfig(_) => false,
            BatchOp::MigrationStatus(_) => false,
            BatchOp::AccessTree(_) => false,
            BatchOp::ListValidators(_) => false,
            BatchOp::GetTableSchema(_) => false,
            BatchOp::DescribeTable(_) => false,
            BatchOp::DescribeRepo(_) => false,
            BatchOp::InternerDump(_) => false,
            BatchOp::ChangesSince(_) => false,
            BatchOp::Subscribe(_) => false,
            BatchOp::Unsubscribe(_) => false,
            BatchOp::ListPublications(_) => false,
            BatchOp::ListSubscriptions(_) => false,
            BatchOp::ReplicationStatus(_) => false,

            // ----- data mutations ----------------------------------------
            BatchOp::Insert(_) => true,
            BatchOp::Update(_) => true,
            BatchOp::Set(_) => true,
            BatchOp::Delete(_) => true,

            // ----- DDL (catalog / schema / index) ------------------------
            BatchOp::CreateDb(_) => true,
            BatchOp::DropDb(_) => true,
            BatchOp::CreateRepo(_) => true,
            BatchOp::DropRepo(_) => true,
            BatchOp::RenameRepo(_) => true,
            BatchOp::AlterRepo(_) => true,
            BatchOp::RenameDb(_) => true,
            BatchOp::CreateTable(_) => true,
            BatchOp::DropTable(_) => true,
            BatchOp::RenameTable(_) => true,
            BatchOp::CreateIndex(_) => true,
            BatchOp::DropIndex(_) => true,
            BatchOp::RenameIndex(_) => true,
            BatchOp::SetBufferConfig(_) => true,
            BatchOp::AlterBufferConfig(_) => true,

            // ----- migrations (engine swap) ------------------------------
            BatchOp::StartMigration(_) => true,
            BatchOp::CommitMigration(_) => true,
            BatchOp::RollbackMigration(_) => true,

            // ----- auth mutations ----------------------------------------
            BatchOp::CreateUser(_) => true,
            BatchOp::DropUser(_) => true,
            BatchOp::GrantRole(_) => true,
            BatchOp::RevokeRole(_) => true,

            // ----- access-control DDL ------------------------------------
            BatchOp::Chmod(_) => true,
            BatchOp::Chown(_) => true,
            BatchOp::Chgrp(_) => true,
            BatchOp::CreateGroup(_) => true,
            BatchOp::DropGroup(_) => true,
            BatchOp::RenameGroup(_) => true,
            BatchOp::AddGroupMember(_) => true,
            BatchOp::RemoveGroupMember(_) => true,

            // ----- function / validator DDL ------------------------------
            BatchOp::CreateFunction(_) => true,
            BatchOp::DropFunction(_) => true,
            BatchOp::RenameFunction(_) => true,
            BatchOp::CreateValidator(_) => true,
            BatchOp::DropValidator(_) => true,
            BatchOp::RenameValidator(_) => true,
            BatchOp::BindValidator(_) => true,
            BatchOp::UnbindValidator(_) => true,

            // ----- declarative schema DDL --------------------------------
            BatchOp::SetTableSchema(_) => true,
            BatchOp::AddSchemaRule(_) => true,
            BatchOp::RemoveSchemaRule(_) => true,

            // ----- function-folder DDL -----------------------------------
            BatchOp::CreateFunctionFolder(_) => true,
            BatchOp::RenameFunctionFolder(_) => true,

            // ----- interner / temporal mutations -------------------------
            BatchOp::InternerTouch(_) => true,
            BatchOp::PurgeHistory(_) => true,
            BatchOp::SetRetention(_) => true,

            // ----- replication DDL (REPLICATION.md §5.5) ------------------
            // Mutating ops: create/drop/alter — they change the system-store
            // catalog (profiles / publications / subscriptions) and thus the
            // persistent cluster topology.
            BatchOp::CreateReplicationProfile(_) => true,
            BatchOp::DropReplicationProfile(_) => true,
            BatchOp::CreatePublication(_) => true,
            BatchOp::DropPublication(_) => true,
            BatchOp::CreateSubscription(_) => true,
            BatchOp::DropSubscription(_) => true,
            BatchOp::AlterSubscription(_) => true,

            // ----- stored procedures -------------------------------------
            // Conservative: a WASM host function may mutate state.
            BatchOp::Call(_) => true,

            // Containers have no intrinsic policy; the iterative visitor
            // classifies the FULL static body, even for zero iterations.
            BatchOp::Batch(_) | BatchOp::ForEach(_) => false,
        }
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
