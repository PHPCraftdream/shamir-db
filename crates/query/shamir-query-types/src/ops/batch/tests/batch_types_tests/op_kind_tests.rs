//! Complete wire-discriminator coverage for batch dispatch.

use serde::Serialize;
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
        | OpKind::DescribeFunction
        | OpKind::ExportFunction
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
        OpKind::AlterFunction => json!({
            "optimize": "now"
        }),
        OpKind::CheckFunction => json!({}),
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
        OpKind::ApplicationSchema => json!({
            "operation": "application_schema_status",
            "payload": {
                "protocol_version": 1,
                "target": {
                    "database_identity": shamir_types::types::record_id::RecordId::from_ts_seq(1_700_000_000_000_000, 7).to_string()
                }
            }
        }),
        OpKind::StartBackup => json!({
            "to": "/backups/snap-1"
        }),
        OpKind::BackupStatus
        | OpKind::CancelBackup
        | OpKind::CancelRequest
        | OpKind::CancelTx
        | OpKind::CancelCursorAdmin => json!({}),
        OpKind::ListActivity => json!({
            "scope": "all",
            "kind": "all"
        }),
        OpKind::ListShapeStats | OpKind::ResetShapeStats => json!({
            "repo": "main"
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
        OpKind::CheckFunction => json!({}),
        OpKind::AccessTree
        | OpKind::ListPublications
        | OpKind::ListSubscriptions
        | OpKind::ReplicationStatus
        | OpKind::StartBackup
        | OpKind::BackupStatus
        | OpKind::CancelBackup
        | OpKind::ListActivity
        | OpKind::ListShapeStats
        | OpKind::ResetShapeStats => json!(true),
        OpKind::ChangesSince
        | OpKind::Unsubscribe
        | OpKind::CancelRequest
        | OpKind::CancelTx
        | OpKind::CancelCursorAdmin => json!(1),
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
        | OpKind::DescribeFunction
        | OpKind::ExportFunction
        | OpKind::AlterFunction
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
        | OpKind::ApplicationSchema
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
        BatchOp::DescribeFunction(_) => OpKind::DescribeFunction,
        BatchOp::ExportFunction(_) => OpKind::ExportFunction,
        BatchOp::CheckFunction(_) => OpKind::CheckFunction,
        BatchOp::AlterFunction(_) => OpKind::AlterFunction,
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
        BatchOp::ApplicationSchema(_) => OpKind::ApplicationSchema,
        BatchOp::StartBackup(_) => OpKind::StartBackup,
        BatchOp::BackupStatus(_) => OpKind::BackupStatus,
        BatchOp::CancelBackup(_) => OpKind::CancelBackup,
        BatchOp::ListActivity(_) => OpKind::ListActivity,
        BatchOp::ListShapeStats(_) => OpKind::ListShapeStats,
        BatchOp::ResetShapeStats(_) => OpKind::ResetShapeStats,
        BatchOp::CancelRequest(_) => OpKind::CancelRequest,
        BatchOp::CancelTx(_) => OpKind::CancelTx,
        BatchOp::CancelCursorAdmin(_) => OpKind::CancelCursorAdmin,
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

/// Malformed-wire cases: the raw map IS the subject under test, so these
/// inputs are hand-built `Value`/serializer maps rather than builder output.
fn decode_malformed<T: Serialize>(wire: &T) -> Result<BatchOp, rmp_serde::decode::Error> {
    let bytes = rmp_serde::to_vec_named(wire).unwrap();
    rmp_serde::from_slice(&bytes)
}

/// The same discriminator key written twice: a `Value` map cannot hold
/// duplicate keys, so the pair is emitted at the serializer level.
struct DuplicateDiscriminator {
    key: &'static str,
    first: Value,
    second: Value,
}

impl Serialize for DuplicateDiscriminator {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map([(self.key, &self.first), (self.key, &self.second)])
    }
}

#[test]
fn cancel_tx_duplicate_discriminator_is_rejected() {
    // mutant: drop the `cancel_tx` arm of the duplicate guard in `batch_op.rs`
    // => the second key overwrites the first and the op decodes.
    let wire = DuplicateDiscriminator {
        key: "cancel_tx",
        first: json!(5),
        second: json!(6),
    };
    let error = decode_malformed(&wire).expect_err("duplicated cancel_tx must not decode");
    assert!(
        error.to_string().contains("duplicate field"),
        "unexpected error: {error}"
    );
}

#[test]
fn list_shape_stats_duplicate_discriminator_is_rejected() {
    // mutant: drop the `list_shape_stats` arm of the duplicate guard in
    // `batch_op.rs` => the second key overwrites the first and the op decodes.
    let wire = DuplicateDiscriminator {
        key: "list_shape_stats",
        first: json!({ "list_shape_stats": true, "repo": "main" }),
        second: json!({ "list_shape_stats": true, "repo": "other" }),
    };
    let error = decode_malformed(&wire).expect_err("duplicated list_shape_stats must not decode");
    assert!(
        error.to_string().contains("duplicate field"),
        "unexpected error: {error}"
    );
}

#[test]
fn reset_shape_stats_duplicate_discriminator_is_rejected() {
    // mutant: drop the `reset_shape_stats` arm of the duplicate guard in
    // `batch_op.rs` => the second key overwrites the first and the op decodes.
    let wire = DuplicateDiscriminator {
        key: "reset_shape_stats",
        first: json!({ "reset_shape_stats": true, "repo": "main" }),
        second: json!({ "reset_shape_stats": true, "repo": "other" }),
    };
    let error = decode_malformed(&wire).expect_err("duplicated reset_shape_stats must not decode");
    assert!(
        error.to_string().contains("duplicate field"),
        "unexpected error: {error}"
    );
}

#[test]
fn cancel_tx_missing_payload_is_rejected() {
    // mutant: accept a map-shaped `cancel_tx` payload (tolerating an absent
    // handle) => `BatchOp::CancelTx` decodes out of an empty payload map.
    decode_malformed(&json!({ "cancel_tx": {} }))
        .expect_err("cancel_tx with an empty payload map must not decode");
}

#[test]
fn cancel_tx_wrong_payload_type_is_rejected() {
    // mutant: coerce the `cancel_tx` payload from a string => a string handle
    // decodes instead of failing the `u64` newtype.
    decode_malformed(&json!({ "cancel_tx": "not-a-number" }))
        .expect_err("cancel_tx with a string handle must not decode");
}

#[test]
fn cancel_tx_extra_field_is_rejected() {
    // mutant: drop `deny_unknown_fields` from `CancelTxWire` => a payload with
    // an extra field decodes.
    decode_malformed(&json!({ "cancel_tx": 5, "extra": 1 }))
        .expect_err("cancel_tx with an extra field must not decode");
}

#[test]
fn cancel_cursor_admin_duplicate_discriminator_is_rejected() {
    // mutant: drop the `cancel_cursor_admin` arm of the duplicate guard in
    // `batch_op.rs` => the second key overwrites the first and the op decodes.
    let wire = DuplicateDiscriminator {
        key: "cancel_cursor_admin",
        first: json!(7),
        second: json!(8),
    };
    let error =
        decode_malformed(&wire).expect_err("duplicated cancel_cursor_admin must not decode");
    assert!(
        error.to_string().contains("duplicate field"),
        "unexpected error: {error}"
    );
}

#[test]
fn cancel_cursor_admin_missing_payload_is_rejected() {
    // mutant: accept a map-shaped `cancel_cursor_admin` payload (tolerating an
    // absent id) => `BatchOp::CancelCursorAdmin` decodes from an empty map.
    decode_malformed(&json!({ "cancel_cursor_admin": {} }))
        .expect_err("cancel_cursor_admin with an empty payload map must not decode");
}

#[test]
fn cancel_cursor_admin_wrong_payload_type_is_rejected() {
    // mutant: coerce the `cancel_cursor_admin` payload from a string => a
    // string id decodes instead of failing the `CursorId` newtype.
    decode_malformed(&json!({ "cancel_cursor_admin": "not-a-number" }))
        .expect_err("cancel_cursor_admin with a string id must not decode");
}

#[test]
fn cancel_cursor_admin_extra_field_is_rejected() {
    // mutant: drop `deny_unknown_fields` from `CancelCursorAdminWire` => a
    // payload with an extra field decodes.
    decode_malformed(&json!({ "cancel_cursor_admin": 5, "extra": 1 }))
        .expect_err("cancel_cursor_admin with an extra field must not decode");
}
