use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::migration::shadow_log::READ_FROM_PAGE_CAP;
use crate::migration::{
    MigrationCoordinator, MigrationPhase, MigrationShadowLog, MigrationState, ShadowOp,
};

fn fixture() -> (
    MigrationCoordinator,
    Arc<MigrationShadowLog>,
    Arc<dyn Store>,
    Arc<dyn Store>,
) {
    let info = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let src = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let dst = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let shadow = Arc::new(MigrationShadowLog::new("cutover".into(), info.clone()));
    let state = MigrationState::new(
        "cutover".into(),
        "rows".into(),
        "src".into(),
        "dst".into(),
        "in_memory".into(),
        None,
    );
    (
        MigrationCoordinator::new(state, shadow.clone(), src, dst.clone(), None),
        shadow,
        info,
        dst,
    )
}

// Red before #1219: only the first page survives, then the log is purged.
#[tokio::test]
async fn final_drain_more_than_three_pages_matches_sequential_model() {
    let (coord, shadow, _, dst) = fixture();
    coord.run_snapshot().await.unwrap();
    coord.mark_cutover_ready().await.unwrap();
    let ids = [RecordId([1; 16]), RecordId([2; 16]), RecordId([3; 16])];
    let mut model = BTreeMap::new();
    let count = READ_FROM_PAGE_CAP * 3 + 17;
    let mut ops = Vec::with_capacity(count);
    for i in 0..count {
        let id = ids[i % ids.len()];
        if i % 5 == 0 {
            model.remove(&id);
            ops.push(ShadowOp::Delete { record_id: id });
        } else {
            let value = i.to_le_bytes().to_vec();
            model.insert(id, value.clone());
            ops.push(ShadowOp::Put {
                record_id: id,
                value,
            });
        }
    }
    shadow.append_batch(ops).await.unwrap();
    assert_eq!(coord.final_drain_and_commit().await.unwrap(), count as u64);
    assert_eq!(coord.state().await.last_lsn_applied, count as u64);
    assert_eq!(coord.phase().await, MigrationPhase::Committed);
    for id in ids {
        let value = dst.get(RecordKey::from(id.as_bytes().to_vec())).await;
        match model.get(&id) {
            Some(expected) => assert_eq!(value.unwrap().as_ref(), expected.as_slice()),
            None => assert!(matches!(value, Err(DbError::NotFound(_)))),
        }
    }
    assert!(shadow.read_from(1).await.unwrap().is_empty());
}

// Red before #1219: mark_cutover_ready accepts a nonzero residual lag.
#[tokio::test]
async fn residual_lag_rejects_cutover_and_draining_remains_retryable() {
    let (coord, shadow, _, _) = fixture();
    coord.run_snapshot().await.unwrap();
    shadow
        .append(ShadowOp::Put {
            record_id: RecordId([1; 16]),
            value: vec![7],
        })
        .await
        .unwrap();
    let result = coord.drain_until_caught_up(1).await.unwrap();
    assert_eq!(result.residual_lag, 1);
    let error = coord.mark_cutover_ready().await.unwrap_err();
    assert!(matches!(error, DbError::Conflict(_)), "{error:?}");
    assert_eq!(coord.phase().await, MigrationPhase::Draining);
    assert_eq!(
        coord.drain_until_caught_up(0).await.unwrap().total_applied,
        1
    );
    coord.mark_cutover_ready().await.unwrap();
    assert_eq!(coord.phase().await, MigrationPhase::CutoverReady);
}

#[tokio::test]
async fn committed_state_recovers_without_changing_legacy_state_layout() {
    let (coord, shadow, info, dst) = fixture();
    coord.run_snapshot().await.unwrap();
    coord.mark_cutover_ready().await.unwrap();
    coord.final_drain_and_commit().await.unwrap();
    coord.complete_finalisation_step(1).await.unwrap();
    drop(coord);
    drop(shadow);
    let state = MigrationCoordinator::load_state(&info, "cutover")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.phase, MigrationPhase::Committed);
    let shadow = Arc::new(
        MigrationShadowLog::recover("cutover".into(), info)
            .await
            .unwrap(),
    );
    let coord = MigrationCoordinator::new(state, shadow, Arc::new(InMemoryStore::new()), dst, None);
    assert_eq!(coord.final_drain_and_commit().await.unwrap(), 0);
    assert_eq!(coord.finalisation_progress().await.unwrap(), 1);
    coord.complete_finalisation_step(2).await.unwrap();
    coord.complete_finalisation_step(1).await.unwrap();
    assert_eq!(coord.finalisation_progress().await.unwrap(), 2);
}

#[tokio::test]
async fn cutover_ready_recovers_after_purge_before_committed_state_write() {
    let (coord, shadow, info, dst) = fixture();
    coord.run_snapshot().await.unwrap();
    coord.mark_cutover_ready().await.unwrap();
    let id = RecordId([4; 16]);
    shadow
        .append(ShadowOp::Put {
            record_id: id,
            value: vec![9],
        })
        .await
        .unwrap();
    coord.final_drain_and_commit().await.unwrap();
    // Reconstruct the persisted state at the exact crash boundary: raw data
    // flushed, cursor persisted, shadow purged, Committed not yet persisted.
    let mut state = coord.state().await;
    drop(coord);
    state.phase = MigrationPhase::CutoverReady;
    let coord = MigrationCoordinator::new(
        state,
        shadow,
        Arc::new(InMemoryStore::new()),
        dst.clone(),
        None,
    );
    coord.persist_state().await.unwrap();
    drop(coord);
    let state = MigrationCoordinator::load_state(&info, "cutover")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.last_lsn_applied, 1);
    let shadow = Arc::new(
        MigrationShadowLog::recover("cutover".into(), info)
            .await
            .unwrap(),
    );
    assert_eq!(shadow.current_lsn(), 0);
    let coord = MigrationCoordinator::new(
        state,
        shadow,
        Arc::new(InMemoryStore::new()),
        dst.clone(),
        None,
    );
    assert_eq!(coord.final_drain_and_commit().await.unwrap(), 0);
    assert_eq!(coord.phase().await, MigrationPhase::Committed);
    assert_eq!(
        dst.get(RecordKey::from(id.as_bytes().to_vec()))
            .await
            .unwrap()
            .as_ref(),
        &[9],
    );
}
