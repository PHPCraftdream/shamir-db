use crate::ddl;
use shamir_query_types::batch::BatchOp;

#[test]
fn describe_repo_builder_builds_admin_op() {
    let op = ddl::describe_repo("archive").build();
    assert!(matches!(op, BatchOp::DescribeRepo(ref payload) if payload.describe_repo == "archive"));
}

#[test]
fn describe_repo_builder_attaches_to_batch() {
    let mut batch = crate::batch::Batch::new();
    batch.describe_repo("repo", ddl::describe_repo("archive"));
    let request = batch.build();
    assert!(request.queries.values().any(
        |entry| matches!(&entry.op, BatchOp::DescribeRepo(op) if op.describe_repo == "archive")
    ));
}
