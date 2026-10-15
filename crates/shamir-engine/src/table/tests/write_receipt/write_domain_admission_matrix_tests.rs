//! S5c-X — admission matrix: EVERY public TABLE-layer mutation family must
//! block while another task holds the repository's `RepoWriteDomain`
//! admission, and must complete once the permit is released.
//!
//! Determinism: mutation futures are polled directly while the permit is
//! held; each must return `Pending` without being spawned or scheduler-driven.
//!
//! Every test here FAILS on pre-S5c-X code: the mutation families did not
//! consult any repo domain, so the "blocked" assertion trips immediately
//! (the task completes while the permit is held).

use std::future::Future;
use std::sync::Arc;

use smallvec::smallvec;

use crate::repo::RepoWriteDomain;
use crate::table::tests::write_receipt::write_domain_shared::{
    make_manager, make_manager_on, spin, wait, SPIN_BUDGET,
};
use crate::validator::{ValidatorBinding, WriteOp};
use shamir_types::types::record_id::RecordId;

/// Hold a permit, spawn `op`, prove it CANNOT finish, release, prove it
/// does — and hand the output to `after` for post-completion assertions.
async fn blocked_until_released<T, R, F, Fut>(
    domain: &Arc<RepoWriteDomain>,
    op: F,
    after: impl FnOnce(T) -> R,
) where
    F: FnOnce() -> Fut,
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let permit = domain.admit().await.expect("admit");
    let mut mutation = Box::pin(op());
    assert!(
        futures::poll!(mutation.as_mut()).is_pending(),
        "mutation completed while repo write admission was held — \
         the S5c-X admit-once gate is missing on this family"
    );
    drop(permit);
    let out = mutation.await;
    after(out);
}

#[tokio::test]
async fn create_index_blocks_while_permit_held_then_applies() {
    let (domain, mgr) = make_manager("adm-create-index").await;
    assert!(!mgr.index_exists("i").await);
    let m = mgr.clone();
    blocked_until_released(
        &domain,
        move || async move { m.create_index("i", &["a"]).await },
        |res| res.expect("create_index after release"),
    )
    .await;
    // RAM/durability: the observable effect lands ONLY after completion.
    assert!(mgr.index_exists("i").await);
}

#[tokio::test]
async fn create_unique_index_blocks_while_permit_held() {
    let (domain, mgr) = make_manager("adm-create-unique").await;
    let m = mgr.clone();
    blocked_until_released(
        &domain,
        move || async move { m.create_unique_index("u", &["a"]).await },
        |res| res.expect("create_unique_index after release"),
    )
    .await;
    assert!(mgr.unique_index_exists("u").await);
}

#[tokio::test]
async fn drop_index_blocks_while_permit_held_then_removes() {
    let (domain, mgr) = make_manager("adm-drop-index").await;
    mgr.create_index("i", &["a"]).await.expect("seed create");
    assert!(mgr.index_exists("i").await);
    let m = mgr.clone();
    blocked_until_released(
        &domain,
        move || async move { m.drop_index("i", None).await },
        |res| res.expect("drop_index after release"),
    )
    .await;
    assert!(
        !mgr.index_exists("i").await,
        "drop must be visible only after release"
    );
}

#[tokio::test]
async fn drop_unique_index_blocks_while_permit_held() {
    let (domain, mgr) = make_manager("adm-drop-unique").await;
    mgr.create_unique_index("u", &["a"])
        .await
        .expect("seed create");
    let m = mgr.clone();
    blocked_until_released(
        &domain,
        move || async move { m.drop_unique_index("u", None).await },
        |res| res.expect("drop_unique_index after release"),
    )
    .await;
    assert!(!mgr.unique_index_exists("u").await);
}

#[tokio::test]
async fn create_sorted_index_blocks_while_permit_held() {
    let (domain, mgr) = make_manager("adm-create-sorted").await;
    let m = mgr.clone();
    blocked_until_released(
        &domain,
        move || async move { m.create_sorted_index("s", &["a"]).await },
        |res| res.expect("create_sorted_index after release"),
    )
    .await;
    assert!(mgr.sorted_index_exists("s").await);
}

#[tokio::test]
async fn create_sorted_index_with_include_blocks_while_permit_held() {
    let (domain, mgr) = make_manager("adm-create-sorted-inc").await;
    let m = mgr.clone();
    blocked_until_released(
        &domain,
        move || async move {
            m.create_sorted_index_with_include("s", &["a"], vec![vec!["b".into()]])
                .await
        },
        |res| res.expect("create_sorted_index_with_include after release"),
    )
    .await;
    assert!(mgr.sorted_index_exists("s").await);
}

#[tokio::test]
async fn drop_sorted_index_blocks_while_permit_held() {
    let (domain, mgr) = make_manager("adm-drop-sorted").await;
    mgr.create_sorted_index("s", &["a"])
        .await
        .expect("seed create");
    let m = mgr.clone();
    blocked_until_released(
        &domain,
        move || async move { m.drop_sorted_index("s", None).await },
        |res| res.expect("drop_sorted_index after release"),
    )
    .await;
    assert!(!mgr.sorted_index_exists("s").await);
}

#[tokio::test]
async fn create_index_v2_blocks_while_permit_held() {
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
    let m = mgr.clone();
    blocked_until_released(
        &domain,
        move || async move { m.create_index_v2(&op).await },
        |res| res.expect("create_index_v2 after release"),
    )
    .await;
    assert!(mgr.index2_exists("lower_name").await);
}

#[tokio::test]
async fn drop_index2_blocks_while_permit_held() {
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
    mgr.create_index_v2(&op).await.expect("seed create");
    assert!(mgr.index2_exists("lower_name").await);
    let m = mgr.clone();
    blocked_until_released(
        &domain,
        move || async move { m.drop_index2("lower_name", None).await },
        |res| res.expect("drop_index2 after release"),
    )
    .await;
    assert!(!mgr.index2_exists("lower_name").await);
}

#[tokio::test]
async fn add_validator_binding_blocks_while_permit_held() {
    let (domain, mgr) = make_manager("adm-add-binding").await;
    let binding = ValidatorBinding {
        validator_id: RecordId::system("v1"),
        ops: smallvec![WriteOp::Insert],
        priority: 1000,
    };
    let m = mgr.clone();
    // Inlined (not via the helper) so the RAM state can be asserted
    // MID-block: while the permit is held the binding must be observable
    // NOWHERE — neither durable nor in `validator_bindings()`.
    let permit = domain.admit().await.expect("admit");
    let task = tokio::spawn(async move { m.add_validator_binding(binding).await });
    spin(SPIN_BUDGET).await;
    assert!(
        !task.is_finished(),
        "add_validator_binding completed while repo write admission was held"
    );
    assert!(
        mgr.validator_bindings().is_empty(),
        "no binding may land in RAM while the flush is blocked on admission"
    );
    drop(permit);
    wait(task)
        .await
        .expect("add_validator_binding after release");
    assert_eq!(mgr.validator_bindings().len(), 1);
}

#[tokio::test]
async fn remove_validator_binding_blocks_while_permit_held() {
    let (domain, mgr) = make_manager("adm-remove-binding").await;
    let id = RecordId::system("v1");
    mgr.add_validator_binding(ValidatorBinding {
        validator_id: id.clone(),
        ops: smallvec![WriteOp::Insert],
        priority: 1000,
    })
    .await
    .expect("seed binding");
    let m = mgr.clone();
    blocked_until_released(
        &domain,
        move || async move { m.remove_validator_binding(&id).await },
        |res| {
            assert!(res.expect("remove_validator_binding after release"));
        },
    )
    .await;
    assert!(mgr.validator_bindings().is_empty());
}

#[tokio::test]
async fn doctor_repair_blocks_while_permit_held() {
    let (domain, mgr) = make_manager("adm-repair").await;
    let m = mgr.clone();
    blocked_until_released(
        &domain,
        move || async move { m.repair().await },
        |res| res.expect("repair after release"),
    )
    .await;
}

/// Negative control: WITHOUT a held permit the same mutations complete
/// inside the spin budget — guards every blocked test above against
/// passing vacuously (a budget far too small for a free mutation would
/// report "blocked" for everything).
#[tokio::test]
async fn negative_control_mutations_complete_without_held_permit() {
    let (_domain, mgr) = make_manager("adm-negative").await;
    let m = mgr.clone();
    let task = tokio::spawn(async move { m.create_index("i", &["a"]).await });
    spin(SPIN_BUDGET).await;
    assert!(
        task.is_finished(),
        "negative control failed: create_index did not finish free — \
         the spin budget is too small for the blocked-* assertions to mean anything"
    );
    task.await.unwrap().expect("create_index ok");
    assert!(mgr.index_exists("i").await);
}

/// Two DIFFERENT repos (private domains) never serialize against each
/// other — a mutation on repo B proceeds while repo A's permit is held.
#[tokio::test]
async fn domains_are_per_repo_not_global() {
    let (domain_a, _mgr_a) = make_manager("repo-a").await;
    let (_domain_b, mgr_b) = make_manager("repo-b").await;
    let _permit_a = domain_a.admit().await.expect("admit a");
    let m = mgr_b.clone();
    let task = tokio::spawn(async move { m.create_index("i", &["a"]).await });
    spin(SPIN_BUDGET).await;
    assert!(
        task.is_finished(),
        "another repo's permit must not block repo B"
    );
    task.await.unwrap().expect("create_index ok");
}

/// Completeness: `rename_index` blocks while the permit is held and applies
/// the name change only after release (seed i, rename i→i2, assert name
/// state before/after). Fails pre-S5c-X: rename never consulted a domain.
#[tokio::test]
async fn rename_index_blocks_while_permit_held_then_renames() {
    let (domain, mgr) = make_manager("adm-rename-index").await;
    mgr.create_index("i", &["a"]).await.expect("seed create");
    assert!(mgr.index_exists("i").await);
    assert!(!mgr.index_exists("i2").await);
    let m = mgr.clone();
    blocked_until_released(
        &domain,
        move || async move { m.rename_index("i", "i2", None).await },
        |res| res.expect("rename_index after release"),
    )
    .await;
    assert!(
        !mgr.index_exists("i").await && mgr.index_exists("i2").await,
        "rename must be visible only after release"
    );
}

/// Completeness: `create_index_v2` on the UNIQUE + BTREE (base_index) path
/// blocks while the permit is held and lands only after release.
#[tokio::test]
async fn create_index_v2_unique_btree_blocks_while_permit_held() {
    let (domain, mgr) = make_manager("adm-create-v2-unique").await;
    let op = shamir_query_types::admin::CreateIndexOp {
        create_index: "uniq_email".into(),
        table: "t".into(),
        fields: vec![vec!["email".into()]],
        unique: true,
        sorted: false,
        repo: "main".into(),
        index_type: None, // btree default
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
    let m = mgr.clone();
    blocked_until_released(
        &domain,
        move || async move { m.create_index_v2(&op).await },
        |res| res.expect("create_index_v2 unique btree after release"),
    )
    .await;
    assert!(mgr.unique_index_exists("uniq_email").await);
}

/// Cross-table DDL-with-writers on ONE shared repo domain: while a permit is
/// held, a plain LEADER WRITE (`insert`) on manager A must COMPLETE (the
/// leader write path gains NO admission — S5c-X invariant), while DDL
/// (`create_index`) on sibling manager B stays BLOCKED. Release → the DDL
/// lands. Fails pre-S5c-X in its blocked half (DDL was ungated); the insert
/// half guards against over-gating (accidentally putting admission on the
/// write path would time out the spin budget with the insert unfinished).
#[tokio::test]
async fn leader_write_completes_while_ddl_blocks_on_shared_domain() {
    let domain = RepoWriteDomain::new("shared-domain");
    let mgr_a = make_manager_on(&domain, "shared-t1").await;
    let mgr_b = make_manager_on(&domain, "shared-t2").await;

    let permit = domain.admit().await.expect("admit");

    // Leader write on A: must complete while the permit is held.
    let a = mgr_a.clone();
    let writer = tokio::spawn(async move {
        a.insert(&shamir_types::types::value::InnerValue::Str(
            "k".to_string(),
        ))
        .await
    });
    spin(SPIN_BUDGET).await;
    assert!(
        writer.is_finished(),
        "leader write must NOT pay repository admission"
    );
    writer.await.unwrap().expect("insert ok");

    // DDL on B (same domain): stays blocked.
    let b = mgr_b.clone();
    let ddl = tokio::spawn(async move { b.create_index("i", &["a"]).await });
    spin(SPIN_BUDGET).await;
    assert!(
        !ddl.is_finished(),
        "DDL must stay blocked while another task holds the shared repo permit"
    );
    assert!(!mgr_b.index_exists("i").await, "no DDL effect mid-block");

    drop(permit);
    ddl.await.unwrap().expect("create_index after release");
    assert!(mgr_b.index_exists("i").await);
}

/// Negative control via the unprotected seam: manager whose domain is B,
/// while domain A's permit is held. The mutation COMPLETES — proving the
/// blocked-* assertions detect gating via `self.write_domain` specifically,
/// not some global lock. This is also how a revert-to-ungated world would
/// manifest: if the admit-once gate were dropped from the mutation families
/// (or the gate read a private per-table domain instead of the shared one),
/// this same "completes while held" behavior would show up in the blocked-*
/// tests and trip their `!task.is_finished()` assertions.
#[tokio::test]
async fn negative_control_other_domain_does_not_block() {
    let (domain_a, _mgr_a) = make_manager("neg-a").await;
    let (_domain_b, mgr_b) = make_manager("neg-b").await;
    let _permit_a = domain_a.admit().await.expect("admit a");
    let m = mgr_b.clone();
    let task = tokio::spawn(async move { m.create_index("i", &["a"]).await });
    spin(SPIN_BUDGET).await;
    assert!(
        task.is_finished(),
        "a mutation through a DIFFERENT domain must not be blocked by A's permit"
    );
    task.await.unwrap().expect("create_index ok");
    assert!(mgr_b.index_exists("i").await);
}
