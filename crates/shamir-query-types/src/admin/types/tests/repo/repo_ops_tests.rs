use shamir_types::mpack;

use crate::admin::{AlterRepoOp, CreateRepoOp, DescribeRepoOp};

#[test]
fn create_repo_rejects_unknown_fields() {
    let value = mpack!({
        "create_repo": "analytics",
        "tablez": ["events"]
    });
    let bytes = rmp_serde::to_vec_named(&value).expect("encode malformed create_repo");
    let error = rmp_serde::from_slice::<CreateRepoOp>(&bytes)
        .expect_err("unknown CreateRepoOp field must be rejected")
        .to_string();
    assert!(
        error.contains("tablez"),
        "unknown field should be named: {error}"
    );
}

#[test]
fn alter_repo_rejects_unknown_fields() {
    let value = mpack!({
        "alter_repo": "main",
        "durabilty": {}
    });
    let bytes = rmp_serde::to_vec_named(&value).expect("encode malformed alter_repo");
    let error = rmp_serde::from_slice::<AlterRepoOp>(&bytes)
        .expect_err("unknown AlterRepoOp field must be rejected")
        .to_string();
    assert!(
        error.contains("durabilty"),
        "unknown field should be named: {error}"
    );
}

#[test]
fn describe_repo_rejects_unknown_fields() {
    let value = mpack!({
        "describe_repo": "main",
        "typo": true
    });
    let bytes = rmp_serde::to_vec_named(&value).expect("encode malformed describe_repo");
    let error = rmp_serde::from_slice::<DescribeRepoOp>(&bytes)
        .expect_err("unknown DescribeRepoOp field must be rejected")
        .to_string();
    assert!(
        error.contains("typo"),
        "unknown field should be named: {error}"
    );
}
