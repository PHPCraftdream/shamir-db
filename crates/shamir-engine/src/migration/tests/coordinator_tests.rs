use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;
use std::sync::Arc;

use crate::migration::coordinator::{MigrationCoordinator, MigrationPhase, MigrationState};
use crate::migration::shadow_log::{MigrationShadowLog, ShadowOp};

fn make_stores() -> (Arc<dyn Store>, Arc<dyn Store>, Arc<dyn Store>) {
    let info = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let src = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let dst = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    (info, src, dst)
}

async fn seed_src(store: &Arc<dyn Store>, n: usize) -> Vec<RecordKey> {
    let mut keys = Vec::new();
    for i in 0..n {
        let k = store.insert(Bytes::from(format!("val_{i}"))).await.unwrap();
        keys.push(k);
    }
    keys
}

#[tokio::test]
async fn full_migration_lifecycle() {
    let (info, src, dst) = make_stores();
    let _keys = seed_src(&src, 10).await;

    let shadow = Arc::new(MigrationShadowLog::new("mig1".into(), info));
    let state = MigrationState::new(
        "mig1".into(),
        "users".into(),
        "main".into(),
        "cold".into(),
        "fjall".into(),
        None,
    );
    let coord = MigrationCoordinator::new(state, shadow.clone(), src.clone(), dst.clone(), None);

    assert_eq!(coord.phase().await, MigrationPhase::ShadowStarted);

    let copied = coord.run_snapshot().await.unwrap();

    // Simulate a write that arrives after snapshot cut
    shadow
        .append(ShadowOp::Put {
            record_id: RecordId::new(),
            value: b"concurrent_write".to_vec(),
        })
        .await
        .unwrap();
    assert_eq!(copied, 10);
    assert_eq!(coord.phase().await, MigrationPhase::Draining);

    let drained = coord.drain_until_caught_up(0).await.unwrap();
    assert_eq!(drained, 1);

    coord.mark_cutover_ready().await.unwrap();
    assert_eq!(coord.phase().await, MigrationPhase::CutoverReady);

    // One more write during cutover prep
    shadow
        .append(ShadowOp::Put {
            record_id: RecordId::new(),
            value: b"late_write".to_vec(),
        })
        .await
        .unwrap();

    let final_drained = coord.final_drain_and_commit().await.unwrap();
    assert_eq!(final_drained, 1);
    assert_eq!(coord.phase().await, MigrationPhase::Committed);

    let (src_count, dst_count) = coord.verify_record_count().await.unwrap();
    assert_eq!(src_count, 10);
    // dst has 10 snapshot + 2 concurrent writes
    assert_eq!(dst_count, 12);
}

#[tokio::test]
async fn rollback_before_commit() {
    let (info, src, dst) = make_stores();
    seed_src(&src, 5).await;

    let shadow = Arc::new(MigrationShadowLog::new("mig2".into(), info));
    let state = MigrationState::new(
        "mig2".into(),
        "t".into(),
        "main".into(),
        "cold".into(),
        "fjall".into(),
        None,
    );
    let coord = MigrationCoordinator::new(state, shadow.clone(), src, dst, None);

    coord.run_snapshot().await.unwrap();
    coord.rollback().await.unwrap();
    assert_eq!(coord.phase().await, MigrationPhase::RolledBack);
}

#[tokio::test]
async fn cannot_rollback_after_commit() {
    let (info, src, dst) = make_stores();
    seed_src(&src, 3).await;

    let shadow = Arc::new(MigrationShadowLog::new("mig3".into(), info));
    let state = MigrationState::new(
        "mig3".into(),
        "t".into(),
        "main".into(),
        "cold".into(),
        "fjall".into(),
        None,
    );
    let coord = MigrationCoordinator::new(state, shadow, src, dst, None);

    coord.run_snapshot().await.unwrap();
    coord.mark_cutover_ready().await.unwrap();
    coord.final_drain_and_commit().await.unwrap();

    let err = coord.rollback().await.unwrap_err();
    assert!(err.to_string().contains("committed"));
}

#[tokio::test]
async fn phase_transitions_enforced() {
    let (info, src, dst) = make_stores();

    let shadow = Arc::new(MigrationShadowLog::new("mig4".into(), info));
    let state = MigrationState::new(
        "mig4".into(),
        "t".into(),
        "main".into(),
        "cold".into(),
        "fjall".into(),
        None,
    );
    let coord = MigrationCoordinator::new(state, shadow, src, dst, None);

    // Can't drain before snapshot
    assert!(coord.drain_shadow_log().await.is_err());
    // Can't mark cutover_ready before draining
    assert!(coord.mark_cutover_ready().await.is_err());
    // Can't final_drain before cutover_ready
    assert!(coord.final_drain_and_commit().await.is_err());
}

#[tokio::test]
async fn deletes_propagated_to_dst() {
    let (info, src, dst) = make_stores();
    let keys = seed_src(&src, 5).await;

    let shadow = Arc::new(MigrationShadowLog::new("mig5".into(), info));
    let state = MigrationState::new(
        "mig5".into(),
        "t".into(),
        "main".into(),
        "cold".into(),
        "fjall".into(),
        None,
    );
    let coord = MigrationCoordinator::new(state, shadow.clone(), src.clone(), dst.clone(), None);

    coord.run_snapshot().await.unwrap();

    // Delete record from src (shadow captures it)
    let mut arr = [0u8; 16];
    arr.copy_from_slice(&keys[0].as_ref()[..16]);
    let rid = RecordId(arr);
    shadow
        .append(ShadowOp::Delete { record_id: rid })
        .await
        .unwrap();

    coord.drain_until_caught_up(0).await.unwrap();
    coord.mark_cutover_ready().await.unwrap();
    coord.final_drain_and_commit().await.unwrap();

    let (_, dst_count) = coord.verify_record_count().await.unwrap();
    assert_eq!(dst_count, 4);
}
