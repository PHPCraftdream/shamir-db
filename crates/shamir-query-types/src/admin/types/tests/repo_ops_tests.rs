use shamir_types::mpack;

use crate::admin::CreateRepoOp;

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
