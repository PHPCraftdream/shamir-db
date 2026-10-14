use shamir_query_types::admin::AlterRepoOp;
use shamir_query_types::batch::BatchOp;
use shamir_query_types::durability::DurabilityOverride;

#[test]
fn repo_admin_ops_roundtrip_full_u64_via_batch_op_codec() {
    let override_value = DurabilityOverride {
        wal_retention_max_bytes: Some(std::num::NonZeroU64::new(u64::MAX).unwrap()),
        ..Default::default()
    };
    let alter = BatchOp::AlterRepo(AlterRepoOp {
        alter_repo: "main".into(),
        durability: Some(override_value),
    });
    let bytes = rmp_serde::to_vec_named(&alter).unwrap();
    let decoded: BatchOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(decoded, alter);

    let mut batch = shamir_query_builder::batch::Batch::new();
    batch.create_repo(
        "create",
        shamir_query_builder::ddl::create_repo("main").durability(override_value),
    );
    let request = batch.build();
    let bytes = rmp_serde::to_vec_named(&request).unwrap();
    let decoded: shamir_query_types::batch::BatchRequest = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(decoded, request);
}
