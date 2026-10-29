//! Complete wire-discriminator coverage for batch dispatch.

use serde_json::{json, Value};

use super::{op_kind, OpKind, ALL};
use crate::batch::BatchOp;

// Wire-format round-trip test: a query builder would bypass the wire shapes under test.
fn fixture(kind: OpKind, key: &str) -> Value {
    let mut fields = match kind {
        OpKind::Read
        | OpKind::CreateDb
        | OpKind::DropDb
        | OpKind::CreateRepo
        | OpKind::DropRepo
        | OpKind::AlterRepo
        | OpKind::CreateTable
        | OpKind::DropTable
        | OpKind::GetBufferConfig
        | OpKind::CommitMigration
        | OpKind::RollbackMigration
        | OpKind::MigrationStatus
        | OpKind::DropUser
        | OpKind::CreateGroup
        | OpKind::DropGroup
        | OpKind::CreateFunction
        | OpKind::DropFunction
        | OpKind::CreateValidator
        | OpKind::DropValidator
        | OpKind::GetTableSchema
        | OpKind::DescribeTable
        | OpKind::DescribeRepo
        | OpKind::InternerDump
        | OpKind::DropReplicationProfile
        | OpKind::DropPublication
        | OpKind::DropSubscription
        | OpKind::Call
        | OpKind::AccessTree
        | OpKind::ListPublications
        | OpKind::ListSubscriptions
        | OpKind::ReplicationStatus
        | OpKind::Subscribe
        | OpKind::Unsubscribe
        | OpKind::ChangesSince
        | OpKind::List
        | OpKind::Batch => json!({}),
        OpKind::Insert => json!({
            "values": []
        }),
        OpKind::Update => json!({
            "set": {}
        }),
        OpKind::Delete => json!({
            "where": {
                "op": "eq",
                "field": ["name"],
                "value": "x"
            }
        }),
        OpKind::Set => json!({
            "key": "key",
            "value": {}
        }),
        OpKind::RenameRepo
        | OpKind::RenameDb
        | OpKind::RenameTable
        | OpKind::RenameFunction
        | OpKind::RenameValidator
        | OpKind::RenameGroup => json!({
            "to": "renamed"
        }),
        OpKind::CreateIndex => json!({
            "table": "users",
            "fields": [["name"]]
        }),
        OpKind::DropIndex => json!({
            "table": "users"
        }),
        OpKind::RenameIndex => json!({
            "table": "users",
            "to": "renamed"
        }),
        OpKind::SetBufferConfig => json!({
            "config": {
                "max_bytes": 100,
                "max_entries": 10,
                "flush_interval_ms": 10,
                "flush_batch_size": 10
            }
        }),
        OpKind::AlterBufferConfig => json!({
            "patch": {}
        }),
        OpKind::StartMigration => json!({
            "dst_repo": "destination",
            "dst_engine": "memory"
        }),
        OpKind::CreateUser => json!({
            "password": "password"
        }),
        OpKind::GrantRole | OpKind::RevokeRole => json!({
            "user": "alice"
        }),
        OpKind::Chmod => json!({
            "mode": 448
        }),
        OpKind::Chown => json!({
            "owner": 1
        }),
        OpKind::Chgrp => json!({
            "group": 1
        }),
        OpKind::AddGroupMember | OpKind::RemoveGroupMember => json!({
            "user": 1
        }),
        OpKind::BindValidator => json!({
            "db": "db",
            "table": "users",
            "ops": ["insert"],
            "priority": 1
        }),
        OpKind::UnbindValidator => json!({
            "db": "db",
            "table": "users"
        }),
        OpKind::ListValidators => json!({
            "db": "db"
        }),
        OpKind::SetTableSchema => json!({
            "schema": []
        }),
        OpKind::AddSchemaRule => json!({
            "rule": {
                "path": ["name"],
                "type": "string"
            }
        }),
        OpKind::RemoveSchemaRule => json!({
            "path": ["name"]
        }),
        OpKind::RenameFunctionFolder => json!({
            "to": ["renamed"]
        }),
        OpKind::CreateFunctionFolder => json!({}),
        OpKind::InternerTouch => json!({
            "names": ["name"]
        }),
        OpKind::PurgeHistory => json!({
            "scope": {
                "older_than_age": {
                    "age_secs": 10
                }
            }
        }),
        OpKind::SetRetention => json!({
            "retention": {
                "max_count": 1
            }
        }),
        OpKind::CreateReplicationProfile => json!({
            "streams": []
        }),
        OpKind::CreatePublication => json!({
            "scopes": []
        }),
        OpKind::CreateSubscription => json!({
            "upstream": "upstream",
            "publication": "publication",
            "profile": "profile"
        }),
        OpKind::AlterSubscription => json!({
            "action": {
                "set_profile": "profile"
            }
        }),
        OpKind::ForEach => json!({
            "over": [],
            "bind_row": "row"
        }),
    };

    let marker = match kind {
        OpKind::Chmod | OpKind::Chown | OpKind::Chgrp => json!({
            "database": "db"
        }),
        OpKind::DropGroup
        | OpKind::RenameGroup
        | OpKind::AddGroupMember
        | OpKind::RemoveGroupMember => json!({
            "name": "group"
        }),
        OpKind::CreateFunctionFolder | OpKind::RenameFunctionFolder => json!(["folder"]),
        OpKind::AccessTree
        | OpKind::ListPublications
        | OpKind::ListSubscriptions
        | OpKind::ReplicationStatus => json!(true),
        OpKind::ChangesSince | OpKind::Unsubscribe => json!(1),
        OpKind::Subscribe => json!([]),
        OpKind::List => json!("databases"),
        OpKind::Batch | OpKind::ForEach => json!({
            "id": 1,
            "queries": {}
        }),
        OpKind::Read
        | OpKind::Insert
        | OpKind::Update
        | OpKind::Delete
        | OpKind::Set
        | OpKind::CreateDb
        | OpKind::DropDb
        | OpKind::CreateRepo
        | OpKind::DropRepo
        | OpKind::RenameRepo
        | OpKind::AlterRepo
        | OpKind::RenameDb
        | OpKind::CreateTable
        | OpKind::DropTable
        | OpKind::RenameTable
        | OpKind::CreateIndex
        | OpKind::DropIndex
        | OpKind::RenameIndex
        | OpKind::SetBufferConfig
        | OpKind::GetBufferConfig
        | OpKind::AlterBufferConfig
        | OpKind::StartMigration
        | OpKind::CommitMigration
        | OpKind::RollbackMigration
        | OpKind::MigrationStatus
        | OpKind::CreateUser
        | OpKind::DropUser
        | OpKind::GrantRole
        | OpKind::RevokeRole
        | OpKind::CreateGroup
        | OpKind::CreateFunction
        | OpKind::DropFunction
        | OpKind::RenameFunction
        | OpKind::CreateValidator
        | OpKind::DropValidator
        | OpKind::RenameValidator
        | OpKind::BindValidator
        | OpKind::UnbindValidator
        | OpKind::ListValidators
        | OpKind::SetTableSchema
        | OpKind::AddSchemaRule
        | OpKind::RemoveSchemaRule
        | OpKind::GetTableSchema
        | OpKind::DescribeTable
        | OpKind::DescribeRepo
        | OpKind::InternerDump
        | OpKind::InternerTouch
        | OpKind::PurgeHistory
        | OpKind::SetRetention
        | OpKind::CreateReplicationProfile
        | OpKind::DropReplicationProfile
        | OpKind::CreatePublication
        | OpKind::DropPublication
        | OpKind::CreateSubscription
        | OpKind::DropSubscription
        | OpKind::AlterSubscription
        | OpKind::Call => json!("target"),
    };
    fields
        .as_object_mut()
        .unwrap()
        .insert(key.to_owned(), marker);
    fields
}

fn variant_kind(op: &BatchOp) -> OpKind {
    match op {
        BatchOp::Read(_) => OpKind::Read,
        BatchOp::Insert(_) => OpKind::Insert,
        BatchOp::Update(_) => OpKind::Update,
        BatchOp::Delete(_) => OpKind::Delete,
        BatchOp::CreateDb(_) => OpKind::CreateDb,
        BatchOp::DropDb(_) => OpKind::DropDb,
        BatchOp::CreateRepo(_) => OpKind::CreateRepo,
        BatchOp::DropRepo(_) => OpKind::DropRepo,
        BatchOp::RenameRepo(_) => OpKind::RenameRepo,
        BatchOp::AlterRepo(_) => OpKind::AlterRepo,
        BatchOp::RenameDb(_) => OpKind::RenameDb,
        BatchOp::CreateTable(_) => OpKind::CreateTable,
        BatchOp::DropTable(_) => OpKind::DropTable,
        BatchOp::RenameTable(_) => OpKind::RenameTable,
        BatchOp::CreateIndex(_) => OpKind::CreateIndex,
        BatchOp::DropIndex(_) => OpKind::DropIndex,
        BatchOp::RenameIndex(_) => OpKind::RenameIndex,
        BatchOp::SetBufferConfig(_) => OpKind::SetBufferConfig,
        BatchOp::GetBufferConfig(_) => OpKind::GetBufferConfig,
        BatchOp::AlterBufferConfig(_) => OpKind::AlterBufferConfig,
        BatchOp::StartMigration(_) => OpKind::StartMigration,
        BatchOp::CommitMigration(_) => OpKind::CommitMigration,
        BatchOp::RollbackMigration(_) => OpKind::RollbackMigration,
        BatchOp::MigrationStatus(_) => OpKind::MigrationStatus,
        BatchOp::CreateUser(_) => OpKind::CreateUser,
        BatchOp::DropUser(_) => OpKind::DropUser,
        BatchOp::GrantRole(_) => OpKind::GrantRole,
        BatchOp::RevokeRole(_) => OpKind::RevokeRole,
        BatchOp::List(_) => OpKind::List,
        BatchOp::Chmod(_) => OpKind::Chmod,
        BatchOp::Chown(_) => OpKind::Chown,
        BatchOp::Chgrp(_) => OpKind::Chgrp,
        BatchOp::CreateGroup(_) => OpKind::CreateGroup,
        BatchOp::DropGroup(_) => OpKind::DropGroup,
        BatchOp::RenameGroup(_) => OpKind::RenameGroup,
        BatchOp::AddGroupMember(_) => OpKind::AddGroupMember,
        BatchOp::RemoveGroupMember(_) => OpKind::RemoveGroupMember,
        BatchOp::AccessTree(_) => OpKind::AccessTree,
        BatchOp::CreateFunction(_) => OpKind::CreateFunction,
        BatchOp::DropFunction(_) => OpKind::DropFunction,
        BatchOp::RenameFunction(_) => OpKind::RenameFunction,
        BatchOp::CreateValidator(_) => OpKind::CreateValidator,
        BatchOp::DropValidator(_) => OpKind::DropValidator,
        BatchOp::RenameValidator(_) => OpKind::RenameValidator,
        BatchOp::BindValidator(_) => OpKind::BindValidator,
        BatchOp::UnbindValidator(_) => OpKind::UnbindValidator,
        BatchOp::ListValidators(_) => OpKind::ListValidators,
        BatchOp::SetTableSchema(_) => OpKind::SetTableSchema,
        BatchOp::AddSchemaRule(_) => OpKind::AddSchemaRule,
        BatchOp::RemoveSchemaRule(_) => OpKind::RemoveSchemaRule,
        BatchOp::GetTableSchema(_) => OpKind::GetTableSchema,
        BatchOp::DescribeTable(_) => OpKind::DescribeTable,
        BatchOp::DescribeRepo(_) => OpKind::DescribeRepo,
        BatchOp::CreateFunctionFolder(_) => OpKind::CreateFunctionFolder,
        BatchOp::RenameFunctionFolder(_) => OpKind::RenameFunctionFolder,
        BatchOp::InternerDump(_) => OpKind::InternerDump,
        BatchOp::InternerTouch(_) => OpKind::InternerTouch,
        BatchOp::PurgeHistory(_) => OpKind::PurgeHistory,
        BatchOp::SetRetention(_) => OpKind::SetRetention,
        BatchOp::ChangesSince(_) => OpKind::ChangesSince,
        BatchOp::CreateReplicationProfile(_) => OpKind::CreateReplicationProfile,
        BatchOp::DropReplicationProfile(_) => OpKind::DropReplicationProfile,
        BatchOp::CreatePublication(_) => OpKind::CreatePublication,
        BatchOp::DropPublication(_) => OpKind::DropPublication,
        BatchOp::CreateSubscription(_) => OpKind::CreateSubscription,
        BatchOp::DropSubscription(_) => OpKind::DropSubscription,
        BatchOp::AlterSubscription(_) => OpKind::AlterSubscription,
        BatchOp::ListPublications(_) => OpKind::ListPublications,
        BatchOp::ListSubscriptions(_) => OpKind::ListSubscriptions,
        BatchOp::ReplicationStatus(_) => OpKind::ReplicationStatus,
        BatchOp::Call(_) => OpKind::Call,
        BatchOp::ForEach(_) => OpKind::ForEach,
        BatchOp::Batch(_) => OpKind::Batch,
        BatchOp::Subscribe(_) => OpKind::Subscribe,
        BatchOp::Unsubscribe(_) => OpKind::Unsubscribe,
        BatchOp::Set(_) => OpKind::Set,
    }
}

#[test]
fn every_discriminator_decodes_and_round_trips_its_variant() {
    for &(key, kind) in ALL {
        assert_eq!(op_kind(key), Some(kind), "lookup for {key}");
        let wire = fixture(kind, key);
        let bytes = rmp_serde::to_vec_named(&wire).unwrap();
        let op: BatchOp =
            rmp_serde::from_slice(&bytes).unwrap_or_else(|error| panic!("{key}: {error}"));
        assert_eq!(variant_kind(&op), kind, "wire discriminator {key}");
        let encoded = rmp_serde::to_vec_named(&op).unwrap();
        let decoded: BatchOp = rmp_serde::from_slice(&encoded)
            .unwrap_or_else(|error| panic!("round-trip {key}: {error}"));
        assert_eq!(variant_kind(&decoded), kind, "round-trip variant {key}");
        assert_eq!(decoded, op, "round-trip payload {key}");
    }
}
