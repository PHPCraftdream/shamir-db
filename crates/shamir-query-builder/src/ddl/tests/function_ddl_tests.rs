//! Tests for Function DDL, Validator DDL, Function folder DDL, and Batch composition.

use shamir_query_types::WriteOp;
use shamir_types::mpack;

use crate::ddl;

use super::helpers::roundtrip;

// ============================================================================
// Function DDL
// ============================================================================

#[test]
fn create_function_from_source_wire() {
    let op = ddl::create_function("my_fn")
        .source("pub fn main() {}")
        .build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "create_function": "my_fn",
            "source": "pub fn main() {}",
            "replace": false
        })
    );
    assert!(op.is_admin());
}

#[test]
fn create_function_from_wasm_with_replace() {
    let op = ddl::create_function("my_fn")
        .wasm("AQIDBA==")
        .replace()
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["create_function"], "my_fn");
    assert_eq!(j["wasm"], "AQIDBA==");
    assert_eq!(j["replace"], true);
    assert!(j.get("source").is_none());
}

#[test]
fn drop_function_wire() {
    let op = ddl::drop_function("my_fn");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_function": "my_fn"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn rename_function_wire() {
    let op = ddl::rename_function("old_fn", "new_fn");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "rename_function": "old_fn",
            "to": "new_fn"
        })
    );
    assert!(op.is_admin());
}

// ============================================================================
// Validator DDL
// ============================================================================

#[test]
fn create_validator_from_source_wire() {
    let op = ddl::create_validator("v_age")
        .source("pub fn validate() {}")
        .build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "create_validator": "v_age",
            "source": "pub fn validate() {}",
            "replace": false
        })
    );
    assert!(op.is_admin());
}

#[test]
fn create_validator_from_wasm_replace() {
    let op = ddl::create_validator("v_age")
        .wasm("AQIDBA==")
        .replace()
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["create_validator"], "v_age");
    assert_eq!(j["wasm"], "AQIDBA==");
    assert_eq!(j["replace"], true);
}

#[test]
fn drop_validator_wire() {
    let op = ddl::drop_validator("v_age");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_validator": "v_age"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn rename_validator_wire() {
    let op = ddl::rename_validator("v_old", "v_new");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "rename_validator": "v_old",
            "to": "v_new"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn bind_validator_wire() {
    let op = ddl::bind_validator("v_age", "users")
        .db("testdb")
        .repo("main")
        .ops([WriteOp::Insert, WriteOp::Update])
        .priority(1500)
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["bind_validator"], "v_age");
    assert_eq!(j["db"], "testdb");
    assert_eq!(j["repo"], "main");
    assert_eq!(j["table"], "users");
    assert_eq!(j["ops"], mpack!(["insert", "update"]));
    assert_eq!(j["priority"], 1500);
    assert!(op.is_admin());
}

#[test]
fn unbind_validator_wire() {
    let op = ddl::unbind_validator("v_age", "users").db("testdb").build();
    let j = roundtrip(&op);
    assert_eq!(j["unbind_validator"], "v_age");
    assert_eq!(j["db"], "testdb");
    assert_eq!(j["repo"], "main");
    assert_eq!(j["table"], "users");
    assert!(op.is_admin());
}

#[test]
fn list_validators_wire() {
    let op = ddl::list_validators("users")
        .db("testdb")
        .repo("hot")
        .build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "list_validators": "users",
            "db": "testdb",
            "repo": "hot"
        })
    );
    assert!(op.is_admin());
}

// ============================================================================
// Function folder DDL
// ============================================================================

#[test]
fn create_function_folder_wire() {
    let op = ddl::create_function_folder(["reports", "daily"]);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "create_function_folder": ["reports", "daily"]
        })
    );
    assert!(op.is_admin());
}

#[test]
fn create_function_folder_single_segment() {
    let op = ddl::create_function_folder(["utils"]);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "create_function_folder": ["utils"]
        })
    );
}

// ============================================================================
// Batch composition — builders pass through Batch::op()
// ============================================================================

#[test]
fn builder_composes_with_batch() {
    let mut batch = crate::batch::Batch::new();
    batch.id(1);
    batch.op("db", ddl::create_db("mydb"));
    batch.op("repo", ddl::create_repo("main").engine("in_memory"));
    batch.op("tbl", ddl::create_table("users"));
    batch.op(
        "idx",
        ddl::create_index("email_idx", "users")
            .field("email")
            .unique(),
    );
    let req = batch.build();
    assert_eq!(req.queries.len(), 4);
    assert!(req.queries["db"].op.is_admin());
    assert!(req.queries["repo"].op.is_admin());
    assert!(req.queries["tbl"].op.is_admin());
    assert!(req.queries["idx"].op.is_admin());
}
