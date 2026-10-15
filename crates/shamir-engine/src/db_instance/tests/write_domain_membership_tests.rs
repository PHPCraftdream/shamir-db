//! Table-membership mutations are gated by the repository write-admission domain.

use crate::db_instance::db_instance::DbInstance;
use crate::repo::{repo_token, BoxRepo, BoxRepoFactory, RepoConfig, RepoInstance, RepoWriteDomain};
use crate::table::TableConfig;
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll};

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::{WalDurability, WalEntryV2, WalOpV2};

use crate::table::table_manager::table_token_for;

#[tokio::test(flavor = "current_thread")]
async fn recovery_waits_for_write_domain_before_replaying_wal() {
    let repo = RepoInstance::new(
        "test".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("t")],
    );
    let table = repo.get_table("t").await.unwrap();
    let wal = repo.repo_wal().await.unwrap();
    let info = repo.tx_info_store().await.unwrap();
    crate::meta::recovery_marker::save_last_committed(&info, 0)
        .await
        .unwrap();

    let mut rid_bytes = [0u8; 16];
    rid_bytes[15] = 73;
    let rid = RecordId(rid_bytes);
    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        repo_token(repo.name()),
        vec![WalOpV2::Put {
            table_id_interned: table_token_for("t"),
            rid,
            body: InnerValue::Str("recovery-proof".into()).to_bytes().unwrap(),
        }],
    )
    .with_commit_version(7);
    wal.begin_grouped(&entry, WalDurability::Buffered)
        .await
        .unwrap();
    let inflight_before = wal.recover().await.unwrap();
    assert_eq!(inflight_before.len(), 1);

    let marker_key = shamir_storage::types::RecordKey::from_slice(
        crate::meta::MetaKey::LastCommittedVersion
            .as_record_id()
            .to_bytes()
            .as_ref(),
    );
    let marker_before = info
        .get(marker_key.clone())
        .await
        .expect("persisted recovery marker");
    let hold = repo.write_domain().admit().await.unwrap();
    let mut recovery = Box::pin(repo.recover_v2_inflight());
    let waker = futures::task::noop_waker_ref();
    let mut cx = Context::from_waker(waker);
    assert!(matches!(recovery.as_mut().poll(&mut cx), Poll::Pending));
    assert!(table.get(rid).await.is_err());
    assert_eq!(info.get(marker_key.clone()).await.unwrap(), marker_before);
    assert_eq!(wal.recover().await.unwrap(), inflight_before);

    drop(hold);
    assert_eq!(recovery.await.unwrap(), 1);
    assert_eq!(
        crate::meta::recovery_marker::load_last_committed(&info)
            .await
            .unwrap(),
        Some(7)
    );
    assert_eq!(wal.recover().await.unwrap(), inflight_before);
    assert!(matches!(table.get(rid).await.unwrap(), InnerValue::Str(s) if s == "recovery-proof"));
    assert_eq!(repo.recover_v2_inflight().await.unwrap(), 1);
    assert_eq!(wal.recover().await.unwrap(), inflight_before);
}

fn assert_pending<F: Future>(mut future: std::pin::Pin<&mut F>) {
    let waker = futures::task::noop_waker_ref();
    let mut context = Context::from_waker(waker);
    assert!(
        matches!(future.as_mut().poll(&mut context), Poll::Pending),
        "mutation completed while admission was blocked"
    );
}

async fn db_with_repo() -> DbInstance {
    let config =
        RepoConfig::new("r", BoxRepoFactory::in_memory()).add_table(TableConfig::new("seeded"));
    DbInstance::with_repos(vec![config]).await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn create_table_blocked_while_permit_held() {
    let db = db_with_repo().await;
    let repo = db.get_repo("r").unwrap();
    let permit = repo.write_domain().admit().await.unwrap();
    let mut mutation = Box::pin(db.create_table("r", "t2"));
    assert_pending(mutation.as_mut());
    assert!(!db.has_table("r", "t2"));
    assert!(!repo.has_table("t2"));
    drop(permit);
    mutation.await.unwrap();
    assert!(db.has_table("r", "t2"));
    assert!(repo.has_table("t2"));
}

#[tokio::test(flavor = "current_thread")]
async fn drop_table_blocked_while_permit_held() {
    let db = db_with_repo().await;
    db.create_table("r", "t2").await.unwrap();
    let repo = db.get_repo("r").unwrap();
    let permit = repo.write_domain().admit().await.unwrap();
    let mut mutation = Box::pin(db.drop_table("r", "t2"));
    assert_pending(mutation.as_mut());
    assert!(db.has_table("r", "t2"));
    drop(permit);
    assert!(mutation.await.unwrap());
    assert!(!db.has_table("r", "t2"));
}

#[tokio::test(flavor = "current_thread")]
async fn membership_rejected_while_reservation_outstanding() {
    let db = db_with_repo().await;
    let repo = db.get_repo("r").unwrap();
    let reservation = repo
        .write_domain()
        .try_place_reservation()
        .expect("reservation slot free");
    let err = db.create_table("r", "t2").await.unwrap_err();
    match &err {
        shamir_storage::error::DbError::Internal(msg) => assert!(
            msg.contains("reservation"),
            "expected Reserved rejection, got: {msg}"
        ),
        other => panic!("expected Internal(Reserved), got: {other:?}"),
    }
    assert!(!db.has_table("r", "t2"));
    drop(reservation);
    db.create_table("r", "t2").await.unwrap();
    assert!(db.has_table("r", "t2"));
}

#[tokio::test]
async fn admit_pair_orders_and_rejects_same() {
    let a = RepoWriteDomain::new("a");
    let b = RepoWriteDomain::new("b");
    let (pa, pb) = RepoWriteDomain::admit_pair(&a, &b).await.unwrap();
    assert_eq!(pa.domain_token(), a.token());
    assert_eq!(pb.domain_token(), b.token());
    drop((pa, pb));
    let (pb, pa) = RepoWriteDomain::admit_pair(&b, &a).await.unwrap();
    assert_eq!(pb.domain_token(), b.token());
    assert_eq!(pa.domain_token(), a.token());
    drop((pb, pa));
    assert!(
        matches!(RepoWriteDomain::admit_pair(&a, &a).await, Err(crate::repo::RepoWriteError::SameRepository { repo }) if repo == "a")
    );

    let hold_a = a.admit().await.unwrap();
    let hold_b = b.admit().await.unwrap();
    let (a1, b1, a2, b2) = (
        std::sync::Arc::clone(&a),
        std::sync::Arc::clone(&b),
        std::sync::Arc::clone(&a),
        std::sync::Arc::clone(&b),
    );
    let mut t1 = Box::pin(async move {
        let (pa, pb) = RepoWriteDomain::admit_pair(&a1, &b1).await.unwrap();
        assert_eq!(pa.domain_token(), a1.token());
        assert_eq!(pb.domain_token(), b1.token());
        drop((pa, pb));
    });
    let mut t2 = Box::pin(async move {
        let (pb, pa) = RepoWriteDomain::admit_pair(&b2, &a2).await.unwrap();
        assert_eq!(pb.domain_token(), b2.token());
        assert_eq!(pa.domain_token(), a2.token());
        drop((pb, pa));
    });
    assert_pending(t1.as_mut());
    assert_pending(t2.as_mut());
    drop((hold_a, hold_b));
    t1.await;
    t2.await;
}
