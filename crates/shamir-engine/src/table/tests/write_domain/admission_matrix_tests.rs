//! S5c-X — admission matrix: EVERY public TABLE-layer mutation family must
//! block while another task holds the repository's `RepoWriteDomain`
//! admission, and must complete once the permit is released.
//!
//! Determinism: single-threaded tokio test runtime + bounded
//! `yield_now` spins (same convention as `f95_ddl_admission_tests.rs`'s
//! `wait_flag`) — no sleeps. A blocked task can never finish, so a bounded
//! spin followed by `!task.is_finished()` is a sound "still waiting on
//! admission" proof; the negative-control test proves the spin budget is
//! generous enough that a free mutation always finishes inside it.
//!
//! Every test here FAILS on pre-S5c-X code: the mutation families did not
//! consult any repo domain, so the "blocked" assertion trips immediately
//! (the task completes while the permit is held).

use std::future::Future;
use std::sync::Arc;

use smallvec::smallvec;

use crate::repo::RepoWriteDomain;
use crate::table::tests::write_domain::shared::{make_manager, spin, wait, SPIN_BUDGET};
use crate::validator::{ValidatorBinding, WriteOp};
use shamir_types::types::record_id::RecordId;

/// Hold a permit, spawn `op`, prove it CANNOT finish, release, prove it
/// does — and hand the output to `after` for post-completion assertions.
async fn blocked_until_released<T, F, Fut>(
    domain: &Arc<RepoWriteDomain>,
    op: F,
    after: impl FnOnce(T),
) where
    F: FnOnce() -> Fut,
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let permit = domain.admit().await.expect("admit");
    let task = tokio::spawn(op());
    spin(SPIN_BUDGET).await;
    assert!(
        !task.is_finished(),
        "mutation completed while repo write admission was held — \
         the S5c-X admit-once gate is missing on this family"
    );
    drop(permit);
    let out = wait(task).await;
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
    blocked_until_released(
        &domain,
        move || async move { m.add_validator_binding(binding).await },
        |res| res.expect("add_validator_binding after release"),
    )
    .await;
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
