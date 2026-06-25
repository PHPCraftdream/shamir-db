//! MessagePack round-trip tests for BatchOp and ListOp variants.
//!
//! Each test builds a `BatchOp` via the query builder, serialises it with
//! `rmp_serde::to_vec_named`, deserialises with `rmp_serde::from_slice`, and
//! asserts round-trip equality.  Where the original tests verified presence /
//! absence of serialised fields (e.g. `if_not_exists`, `cascade`), the
//! assertions now inspect the deserialized struct fields directly.

use shamir_db::query::batch::BatchOp;
use shamir_query_builder::batch::IntoBatchOp;
use shamir_query_builder::ddl;

// ═══════════════════════════════════════════════════════════════════════
// 10. BatchOp msgpack round-trip for new variants
// ═══════════════════════════════════════════════════════════════════════

fn msgpack_roundtrip(op: BatchOp) -> BatchOp {
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    rmp_serde::from_slice::<BatchOp>(&bytes).unwrap()
}

#[test]
fn create_function_serde_roundtrip() {
    let op = ddl::create_function("my_fn")
        .wasm("AAAA")
        .replace()
        .into_batch_op();
    assert!(op.is_admin());
    assert!(op.table_ref().is_none());
    let op2 = msgpack_roundtrip(op.clone());
    assert_eq!(op, op2);
}

#[test]
fn create_validator_serde_roundtrip() {
    let op = ddl::create_validator("v_age").wasm("BBBB").into_batch_op();
    assert!(op.is_admin());
    let op2 = msgpack_roundtrip(op.clone());
    assert_eq!(op, op2);
}

#[test]
fn bind_validator_serde_roundtrip() {
    let op = ddl::bind_validator("v_age", "users")
        .db("testdb")
        .repo("main")
        .ops([
            shamir_query_builder::ddl::WriteOp::Insert,
            shamir_query_builder::ddl::WriteOp::Update,
        ])
        .priority(1500)
        .into_batch_op();
    assert!(op.is_admin());
    let op2 = msgpack_roundtrip(op.clone());
    assert_eq!(op, op2);
}

#[test]
fn create_function_folder_serde_roundtrip() {
    let op = ddl::create_function_folder(["reports", "daily"]);
    assert!(op.is_admin());
    let op2 = msgpack_roundtrip(op.clone());
    assert_eq!(op, op2);
}

#[test]
fn drop_function_serde_roundtrip() {
    let op = ddl::drop_function("my_fn").into_batch_op();
    assert!(op.is_admin());
    let op2 = msgpack_roundtrip(op.clone());
    assert_eq!(op, op2);
}

#[test]
fn rename_validator_serde_roundtrip() {
    let op = ddl::rename_validator("v_old", "v_new");
    assert!(op.is_admin());
    let op2 = msgpack_roundtrip(op.clone());
    assert_eq!(op, op2);
}

// =====================================================================
// Phase 1b: msgpack round-trip — if_not_exists / cascade
// =====================================================================

#[test]
fn serde_create_table_if_not_exists_round_trip() {
    // With flag set
    let op_with = ddl::create_table("orders")
        .repo("main")
        .if_not_exists()
        .into_batch_op();
    let op_with2 = msgpack_roundtrip(op_with.clone());
    assert_eq!(op_with, op_with2);
    // The if_not_exists flag must be preserved on the decoded struct.
    if let BatchOp::CreateTable(inner) = &op_with2 {
        assert!(
            inner.if_not_exists,
            "if_not_exists must be true after roundtrip"
        );
    } else {
        panic!("expected CreateTable variant");
    }

    // With flag absent (default false) — must NOT be set after roundtrip.
    let op_without = ddl::create_table("orders").repo("main").into_batch_op();
    let op_without2 = msgpack_roundtrip(op_without.clone());
    assert_eq!(op_without, op_without2);
    if let BatchOp::CreateTable(inner) = &op_without2 {
        assert!(
            !inner.if_not_exists,
            "if_not_exists must be false when not set"
        );
    } else {
        panic!("expected CreateTable variant");
    }
}

#[test]
fn serde_drop_db_cascade_round_trip() {
    // With cascade
    let op_with = ddl::drop_db("testdb").cascade().into_batch_op();
    let op_with2 = msgpack_roundtrip(op_with.clone());
    assert_eq!(op_with, op_with2);
    if let BatchOp::DropDb(inner) = &op_with2 {
        assert!(inner.cascade, "cascade must be true after roundtrip");
    } else {
        panic!("expected DropDb variant");
    }

    // Without cascade — must NOT be set after roundtrip.
    let op_without = ddl::drop_db("testdb").into_batch_op();
    let op_without2 = msgpack_roundtrip(op_without.clone());
    assert_eq!(op_without, op_without2);
    if let BatchOp::DropDb(inner) = &op_without2 {
        assert!(!inner.cascade, "cascade must be false when not set");
    } else {
        panic!("expected DropDb variant");
    }
}

#[test]
fn serde_drop_repo_cascade_round_trip() {
    let op = ddl::drop_repo("archive").cascade().into_batch_op();
    let op2 = msgpack_roundtrip(op.clone());
    assert_eq!(op, op2);
    if let BatchOp::DropRepo(inner) = &op2 {
        assert!(inner.cascade, "cascade must be preserved");
    } else {
        panic!("expected DropRepo variant");
    }
}

#[test]
fn serde_create_db_if_not_exists_round_trip() {
    let op = ddl::create_db("mydb").if_not_exists().into_batch_op();
    let op2 = msgpack_roundtrip(op.clone());
    assert_eq!(op, op2);
    if let BatchOp::CreateDb(inner) = &op2 {
        assert!(inner.if_not_exists, "if_not_exists must be preserved");
    } else {
        panic!("expected CreateDb variant");
    }
}

// =====================================================================
// Msgpack round-trip for new ListOp variants
// =====================================================================

#[test]
fn serde_list_functions_round_trip() {
    let op = ddl::list_functions().into_batch_op();
    assert!(op.is_admin());
    let op2 = msgpack_roundtrip(op.clone());
    assert_eq!(op, op2);
}

#[test]
fn serde_list_functions_with_folder_round_trip() {
    let op = ddl::list_functions().folder("math").into_batch_op();
    assert!(op.is_admin());
    let op2 = msgpack_roundtrip(op.clone());
    assert_eq!(op, op2);
    // The folder must be preserved in the decoded struct.
    if let BatchOp::List(shamir_db::query::admin::ListOp::Functions { folder }) = &op2 {
        assert_eq!(folder.as_deref(), Some("math"), "folder must be preserved");
    } else {
        panic!("expected List::Functions variant");
    }
}

#[test]
fn serde_list_validators_round_trip() {
    let op = ddl::list_all_validators();
    assert!(op.is_admin());
    let op2 = msgpack_roundtrip(op.clone());
    assert_eq!(op, op2);
}

#[test]
fn serde_list_function_folders_round_trip() {
    let op = ddl::list_function_folders().into_batch_op();
    assert!(op.is_admin());
    let op2 = msgpack_roundtrip(op.clone());
    assert_eq!(op, op2);
}

#[test]
fn serde_list_function_folders_with_parent_round_trip() {
    let op = ddl::list_function_folders()
        .parent("reports")
        .into_batch_op();
    assert!(op.is_admin());
    let op2 = msgpack_roundtrip(op.clone());
    assert_eq!(op, op2);
    // The parent must be preserved in the decoded struct.
    if let BatchOp::List(shamir_db::query::admin::ListOp::FunctionFolders { parent }) = &op2 {
        assert_eq!(
            parent.as_deref(),
            Some("reports"),
            "parent must be preserved"
        );
    } else {
        panic!("expected List::FunctionFolders variant");
    }
}
