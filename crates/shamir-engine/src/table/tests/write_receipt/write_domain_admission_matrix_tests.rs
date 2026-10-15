//! Mutation futures are directly polled while admission is held; snapshots of backing stores and RAM must remain unchanged until release.

use std::future::Future;
use std::sync::Arc;

use smallvec::smallvec;

use crate::repo::RepoWriteDomain;
use crate::table::tests::write_receipt::write_domain_shared::{
    make_manager, make_manager_on, store_snapshot,
};
use crate::table::TableManager;
use crate::validator::{ValidatorBinding, WriteOp};
use shamir_types::types::record_id::RecordId;

type StoreSnapshot = Vec<(shamir_storage::types::RecordKey, bytes::Bytes)>;

async fn blocked_until_released<T, Fut, Observe>(
    domain: &Arc<RepoWriteDomain>,
    mgr: &TableManager,
    op: impl FnOnce() -> Fut,
    observe: Observe,
) -> T
where
    Fut: Future<Output = T>,
    Observe: FnOnce(&TableManager),
{
    let before_data: StoreSnapshot = store_snapshot(mgr.data_store().as_ref()).await;
    let before_info: StoreSnapshot = store_snapshot(mgr.info_store().as_ref()).await;
    let before_counter = mgr.counter().get().await.expect("counter snapshot");
    let before_regular = mgr.index_manager_ref().iter_indexes().collect::<Vec<_>>();
    let before_unique = mgr.index_manager_ref().has_unique_indexes();

    let before_index2: Vec<_> = mgr
        .index2_registry()
        .all_descriptors()
        .await
        .iter()
        .map(|desc| (desc.id, desc.name.clone(), desc.paths.clone()))
        .collect();
    let before_bindings: Vec<_> = mgr
        .validator_bindings()
        .iter()
        .map(|binding| {
            (
                binding.validator_id.clone(),
                binding.ops.clone(),
                binding.priority,
            )
        })
        .collect();

    let permit = domain.admit().await.expect("admit");
    let mut mutation = Box::pin(op());
    assert!(
        futures::poll!(mutation.as_mut()).is_pending(),
        "mutation completed while repository admission was held"
    );
    assert_eq!(store_snapshot(mgr.data_store().as_ref()).await, before_data);
    assert_eq!(store_snapshot(mgr.info_store().as_ref()).await, before_info);
    assert_eq!(mgr.counter().get().await.unwrap(), before_counter);
    assert_eq!(
        mgr.index_manager_ref().iter_indexes().collect::<Vec<_>>(),
        before_regular
    );
    assert_eq!(mgr.index_manager_ref().has_unique_indexes(), before_unique);
    let after_index2: Vec<_> = mgr
        .index2_registry()
        .all_descriptors()
        .await
        .iter()
        .map(|desc| (desc.id, desc.name.clone(), desc.paths.clone()))
        .collect();
    assert_eq!(after_index2, before_index2);
    let after_bindings: Vec<_> = mgr
        .validator_bindings()
        .iter()
        .map(|binding| {
            (
                binding.validator_id.clone(),
                binding.ops.clone(),
                binding.priority,
            )
        })
        .collect();
    assert_eq!(after_bindings, before_bindings);
    observe(mgr);

    drop(permit);
    mutation.await
}

fn no_extra_effects(_: &TableManager) {}

#[tokio::test]
async fn create_index_blocks_without_durable_or_ram_effect() {
    let (domain, mgr) = make_manager("adm-create-index").await;
    assert!(!mgr.index_exists("i").await);
    blocked_until_released(
        &domain,
        &mgr,
        || mgr.create_index("i", &["a"]),
        no_extra_effects,
    )
    .await
    .expect("create index after release");
    assert!(mgr.index_exists("i").await);
}

#[tokio::test]
async fn create_unique_index_blocks_without_effect() {
    let (domain, mgr) = make_manager("adm-create-unique").await;
    blocked_until_released(
        &domain,
        &mgr,
        || mgr.create_unique_index("u", &["a"]),
        no_extra_effects,
    )
    .await
    .expect("create unique after release");
    assert!(mgr.unique_index_exists("u").await);
}

#[tokio::test]
async fn drop_index_blocks_without_effect() {
    let (domain, mgr) = make_manager("adm-drop-index").await;
    mgr.create_index("i", &["a"]).await.expect("seed index");
    blocked_until_released(
        &domain,
        &mgr,
        || mgr.drop_index("i", None),
        no_extra_effects,
    )
    .await
    .expect("drop index after release");
    assert!(!mgr.index_exists("i").await);
}

#[tokio::test]
async fn drop_unique_index_blocks_without_effect() {
    let (domain, mgr) = make_manager("adm-drop-unique").await;
    mgr.create_unique_index("u", &["a"])
        .await
        .expect("seed unique");
    blocked_until_released(
        &domain,
        &mgr,
        || mgr.drop_unique_index("u", None),
        no_extra_effects,
    )
    .await
    .expect("drop unique after release");
    assert!(!mgr.unique_index_exists("u").await);
}

#[tokio::test]
async fn sorted_index_families_block_without_effect() {
    let (domain, mgr) = make_manager("adm-sorted").await;
    blocked_until_released(
        &domain,
        &mgr,
        || mgr.create_sorted_index("s", &["a"]),
        no_extra_effects,
    )
    .await
    .expect("create sorted");
    assert!(mgr.sorted_index_exists("s").await);

    blocked_until_released(
        &domain,
        &mgr,
        || mgr.drop_sorted_index("s", None),
        no_extra_effects,
    )
    .await
    .expect("drop sorted");
    assert!(!mgr.sorted_index_exists("s").await);
    blocked_until_released(
        &domain,
        &mgr,
        || mgr.create_sorted_index_with_include("s_inc", &["a"], vec![vec!["b".into()]]),
        no_extra_effects,
    )
    .await
    .expect("create sorted with include");
    assert!(mgr.sorted_index_exists("s_inc").await);
    blocked_until_released(
        &domain,
        &mgr,
        || mgr.drop_sorted_index("s_inc", None),
        no_extra_effects,
    )
    .await
    .expect("drop sorted with include");
    assert!(!mgr.sorted_index_exists("s_inc").await);
    assert!(!mgr.sorted_index_exists("s").await);
}

#[tokio::test]
async fn rename_index_blocks_without_effect() {
    let (domain, mgr) = make_manager("adm-rename-index").await;
    mgr.create_index("old", &["a"]).await.expect("seed index");
    blocked_until_released(
        &domain,
        &mgr,
        || mgr.rename_index("old", "new", None),
        no_extra_effects,
    )
    .await
    .expect("rename index");
    assert!(!mgr.index_exists("old").await);
    assert!(mgr.index_exists("new").await);
}

#[tokio::test]
async fn unique_btree_v2_blocks_without_effect() {
    let (domain, mgr) = make_manager("adm-unique-btree-v2").await;
    let op = shamir_query_types::admin::CreateIndexOp {
        create_index: "uniq_email".into(),
        table: "t".into(),
        fields: vec![vec!["email".into()]],
        unique: true,
        sorted: false,
        repo: "main".into(),
        index_type: None,
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    };
    blocked_until_released(&domain, &mgr, || mgr.create_index_v2(&op), no_extra_effects)
        .await
        .expect("create unique btree");
    assert!(mgr.unique_index_exists("uniq_email").await);
}

#[tokio::test]
async fn create_index_v2_blocks_without_effect() {
    let (domain, mgr) = make_manager("adm-create-index2").await;
    let op = shamir_query_types::admin::CreateIndexOp {
        create_index: "lower_name".into(),
        table: "t".into(),
        fields: vec![vec!["name".into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("functional".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: Some("lower".into()),
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    };
    blocked_until_released(&domain, &mgr, || mgr.create_index_v2(&op), no_extra_effects)
        .await
        .expect("create index v2");
    assert!(mgr.index2_exists("lower_name").await);
}

#[tokio::test]
async fn drop_index2_blocks_without_effect() {
    let (domain, mgr) = make_manager("adm-drop-index2").await;
    let op = shamir_query_types::admin::CreateIndexOp {
        create_index: "lower_name".into(),
        table: "t".into(),
        fields: vec![vec!["name".into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("functional".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: Some("lower".into()),
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    };
    mgr.create_index_v2(&op).await.expect("seed index2");
    blocked_until_released(
        &domain,
        &mgr,
        || mgr.drop_index2("lower_name", None),
        no_extra_effects,
    )
    .await
    .expect("drop index2");
    assert!(!mgr.index2_exists("lower_name").await);
}

#[tokio::test]
async fn add_and_remove_validator_bindings_block_without_effect() {
    let (domain, mgr) = make_manager("adm-validator").await;
    let id = RecordId::system("validator");
    let binding = ValidatorBinding {
        validator_id: id.clone(),
        ops: smallvec![WriteOp::Insert],
        priority: 1000,
    };
    blocked_until_released(
        &domain,
        &mgr,
        || mgr.add_validator_binding(binding),
        no_extra_effects,
    )
    .await
    .expect("add binding");
    assert_eq!(mgr.validator_bindings().len(), 1);
    blocked_until_released(
        &domain,
        &mgr,
        || mgr.remove_validator_binding(&id),
        no_extra_effects,
    )
    .await
    .expect("remove binding");
    assert!(mgr.validator_bindings().is_empty());
}

#[tokio::test]
async fn doctor_repair_blocks_without_effect() {
    let (domain, mgr) = make_manager("adm-repair").await;
    blocked_until_released(&domain, &mgr, || mgr.repair(), no_extra_effects)
        .await
        .expect("repair after release");
}

#[tokio::test]
async fn buffer_config_mutations_block_without_effect() {
    let (domain, mgr) = make_manager("adm-buffer-config").await;
    let config = shamir_storage::storage_membuffer::MemBufferConfig {
        max_entries: 222,
        ..Default::default()
    };
    blocked_until_released(
        &domain,
        &mgr,
        || mgr.set_buffer_config(&config),
        no_extra_effects,
    )
    .await
    .expect("set buffer config");
    let altered_config = mgr.alter_buffer_config(|config| {
        config.max_entries = 321;
    });
    let result = blocked_until_released(&domain, &mgr, || altered_config, no_extra_effects)
        .await
        .expect("alter buffer config");
    assert_eq!(result.max_entries, 321);
}

#[tokio::test]
async fn repository_replication_helpers_block_without_effect() {
    let domain = RepoWriteDomain::new("replication-domain");
    let src = make_manager_on(&domain, "replication-source").await;
    let dst = make_manager_on(&domain, "replication-destination").await;

    blocked_until_released(
        &domain,
        &dst,
        || dst.replicate_interner_from(&src),
        no_extra_effects,
    )
    .await
    .expect("replicate interner");
    blocked_until_released(
        &domain,
        &dst,
        || dst.replicate_index2_descriptors_from(&src),
        no_extra_effects,
    )
    .await
    .expect("replicate index2 descriptors");
    blocked_until_released(
        &domain,
        &dst,
        || dst.seed_log_from_data_store(),
        no_extra_effects,
    )
    .await
    .expect("seed log");
    blocked_until_released(
        &domain,
        &dst,
        || dst.bulk_populate_index2(),
        no_extra_effects,
    )
    .await
    .expect("bulk populate index2");
}

#[tokio::test]
async fn migration_freeze_and_leader_write_semantics() {
    let (domain, mgr) = make_manager("adm-freeze").await;
    blocked_until_released(
        &domain,
        &mgr,
        || mgr.begin_migration_freeze(),
        no_extra_effects,
    )
    .await
    .expect("freeze after release");

    let permit = domain.admit().await.expect("hold permit");
    let record = shamir_types::types::value::InnerValue::Str("k".into());
    mgr.insert(&record)
        .await
        .expect("leader write must remain ungated");
    drop(permit);
}

#[tokio::test]
async fn different_repository_domain_is_not_blocked() {
    let domain_a = RepoWriteDomain::new("repo-a");
    let permit_a = domain_a.admit().await.expect("admit a");
    let domain_b = RepoWriteDomain::new("repo-b");
    let mgr_b = make_manager_on(&domain_b, "table-b").await;
    mgr_b
        .create_index("i", &["a"])
        .await
        .expect("domain b mutation");
    drop(permit_a);
    assert!(mgr_b.index_exists("i").await);
}

#[tokio::test]
async fn free_mutation_completes_without_a_held_permit() {
    let (_domain, mgr) = make_manager("adm-free").await;
    mgr.create_index("i", &["a"]).await.expect("free mutation");
    assert!(mgr.index_exists("i").await);
}
