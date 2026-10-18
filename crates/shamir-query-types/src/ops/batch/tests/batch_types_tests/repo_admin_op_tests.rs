use shamir_types::mpack;

use crate::admin::{AlterRepoOp, DescribeRepoOp};
use crate::batch::BatchOp;

#[test]
fn alter_repo_batch_op_roundtrips_and_classifies_as_write() {
    let wire = mpack!({
        "alter_repo": "main",
        "durability": { "wal_commit": "synced" }
    });
    let bytes = rmp_serde::to_vec_named(&wire).expect("encode alter_repo batch op");
    let op: BatchOp = rmp_serde::from_slice(&bytes).expect("decode AlterRepoOp through BatchOp");
    assert!(matches!(op, BatchOp::AlterRepo(AlterRepoOp { .. })));
    assert!(op.is_write());
    assert!(op.is_admin());
}

#[test]
fn describe_repo_batch_op_roundtrips_and_classifies_as_read() {
    let wire = mpack!({ "describe_repo": "main" });
    let bytes = rmp_serde::to_vec_named(&wire).expect("encode describe_repo batch op");
    let op: BatchOp = rmp_serde::from_slice(&bytes).expect("decode DescribeRepoOp through BatchOp");
    assert!(matches!(op, BatchOp::DescribeRepo(DescribeRepoOp { .. })));
    assert!(!op.is_write());
    assert!(op.is_admin());
}
