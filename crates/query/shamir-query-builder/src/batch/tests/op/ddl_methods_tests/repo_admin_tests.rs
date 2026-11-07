use shamir_query_types::batch::BatchOp;

use crate::batch::Batch;
use crate::ddl;

#[test]
fn repo_admin_batch_methods_build_typed_ops() {
    let mut batch = Batch::new();
    batch.alter_repo("alter", ddl::alter_repo("main"));
    batch.describe_repo("describe", ddl::describe_repo("main"));

    let request = batch.build();
    assert!(matches!(request.queries["alter"].op, BatchOp::AlterRepo(_)));
    assert!(matches!(
        request.queries["describe"].op,
        BatchOp::DescribeRepo(_)
    ));
}
