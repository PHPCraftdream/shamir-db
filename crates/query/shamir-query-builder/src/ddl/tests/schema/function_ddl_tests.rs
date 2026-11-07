//! Tests for Function DDL, Validator DDL, Function folder DDL, and Batch composition.

use shamir_query_types::WriteOp;
use shamir_types::mpack;

use shamir_query_types::admin::{
    AlterFunctionOp, CheckFunctionOp, CheckFunctionSpec, DescribeFunctionOp, ExportFunctionOp,
};
use shamir_query_types::batch::BatchOp;

use crate::ddl;
use crate::write::BuilderError;

use crate::ddl::tests::helpers::roundtrip;

// ============================================================================
// Function DDL
// ============================================================================

#[test]
fn create_function_from_source_wire() {
    let op = ddl::create_function("my_fn")
        .source("pub fn main() {}")
        .build()
        .unwrap();
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
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(j["create_function"], "my_fn");
    assert_eq!(j["wasm"], "AQIDBA==");
    assert_eq!(j["replace"], true);
    assert!(j.get("source").is_none());
}

#[test]
fn drop_function_wire() {
    let op = ddl::drop_function("my_fn").hmac("abc").build().unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_function": "my_fn",
            "hmac": "abc"
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
        .build()
        .unwrap();
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
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(j["create_validator"], "v_age");
    assert_eq!(j["wasm"], "AQIDBA==");
    assert_eq!(j["replace"], true);
}

#[test]
fn drop_validator_wire() {
    let op = ddl::drop_validator("v_age").hmac("abc").build().unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_validator": "v_age",
            "hmac": "abc"
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

// ============================================================================
// Function wire ops — describe / export / alter / check
// ============================================================================

#[test]
fn describe_function_produces_op() {
    let op = ddl::describe_function("my_fn");
    assert_eq!(
        op,
        BatchOp::DescribeFunction(DescribeFunctionOp {
            describe_function: "my_fn".to_string(),
        })
    );
    assert!(op.is_admin());
}

#[test]
fn export_function_produces_op() {
    let op = ddl::export_function("my_fn");
    assert_eq!(
        op,
        BatchOp::ExportFunction(ExportFunctionOp {
            export_function: "my_fn".to_string(),
        })
    );
    assert!(op.is_admin());
}

#[test]
fn alter_function_produces_op() {
    let op = ddl::alter_function("my_fn", "never");
    assert_eq!(
        op,
        BatchOp::AlterFunction(AlterFunctionOp {
            alter_function: "my_fn".to_string(),
            optimize: "never".to_string(),
        })
    );
    assert!(op.is_admin());
}

#[test]
fn check_function_from_wasm_produces_op() {
    let op = ddl::check_function()
        .lang("wasm")
        .wasm("AQIDBA==")
        .optimize("auto")
        .build()
        .unwrap();
    assert_eq!(
        op,
        BatchOp::CheckFunction(CheckFunctionOp {
            check_function: CheckFunctionSpec {
                lang: Some("wasm".to_string()),
                wasm: Some("AQIDBA==".to_string()),
                source: None,
                optimize: Some("auto".to_string()),
            },
        })
    );
    assert!(op.is_admin());
}

#[test]
fn check_function_from_source_produces_op() {
    let op = ddl::check_function()
        .source("pub fn main() {}")
        .build()
        .unwrap();
    assert_eq!(
        op,
        BatchOp::CheckFunction(CheckFunctionOp {
            check_function: CheckFunctionSpec {
                lang: None,
                wasm: None,
                source: Some("pub fn main() {}".to_string()),
                optimize: None,
            },
        })
    );
    assert!(op.is_admin());
}

#[test]
fn check_function_rejects_both_wasm_and_source() {
    let err = ddl::check_function()
        .wasm("AQIDBA==")
        .source("pub fn main() {}")
        .build()
        .unwrap_err();
    assert_eq!(err, BuilderError::ConflictingFunctionPayload);
}

#[test]
fn check_function_rejects_missing_payload() {
    let err = ddl::check_function().build().unwrap_err();
    assert_eq!(err, BuilderError::ConflictingFunctionPayload);
}

#[test]
fn create_function_lang_and_optimize_set() {
    let op = ddl::create_function("my_fn")
        .wasm("AQIDBA==")
        .lang("wasm")
        .optimize("never")
        .build()
        .unwrap();
    match op {
        BatchOp::CreateFunction(op) => {
            assert_eq!(op.lang, Some("wasm".to_string()));
            assert_eq!(op.optimize, Some("never".to_string()));
        }
        other => panic!("expected CreateFunction, got {other:?}"),
    }
}

#[test]
fn create_function_lang_and_optimize_default_absent() {
    let op = ddl::create_function("my_fn")
        .wasm("AQIDBA==")
        .build()
        .unwrap();
    match op {
        BatchOp::CreateFunction(op) => {
            assert_eq!(op.lang, None);
            assert_eq!(op.optimize, None);
        }
        other => panic!("expected CreateFunction, got {other:?}"),
    }
}
