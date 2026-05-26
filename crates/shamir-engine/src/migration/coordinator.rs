use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::shadow_log::{MigrationShadowLog, ShadowOp};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MigrationPhase {
    ShadowStarted,
    Snapshotting,
    Draining,
    CutoverReady,
    Committed,
    RolledBack,
}

impl std::fmt::Display for MigrationPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ShadowStarted => write!(f, "shadow_started"),
            Self::Snapshotting => write!(f, "snapshotting"),
            Self::Draining => write!(f, "draining"),
            Self::CutoverReady => write!(f, "cutover_ready"),
            Self::Committed => write!(f, "committed"),
            Self::RolledBack => write!(f, "rolled_back"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationState {
    pub id: String,
    pub phase: MigrationPhase,
    pub table_name: String,
    pub src_repo: String,
    pub dst_repo: String,
    pub dst_engine: String,
    pub dst_path: Option<String>,
    pub snapshot_lsn: u64,
    pub last_lsn_applied: u64,
    pub started_at_ns: u64,
    pub records_copied: u64,
}

impl MigrationState {
    pub fn new(
        id: String,
        table_name: String,
        src_repo: String,
        dst_repo: String,
        dst_engine: String,
        dst_path: Option<String>,
    ) -> Self {
        let started_at_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self {
            id,
            phase: MigrationPhase::ShadowStarted,
            table_name,
            src_repo,
            dst_repo,
            dst_engine,
            dst_path,
            snapshot_lsn: 0,
            last_lsn_applied: 0,
            started_at_ns,
            records_copied: 0,
        }
    }
}

pub struct MigrationCoordinator {
    state: Mutex<MigrationState>,
    shadow_log: Arc<MigrationShadowLog>,
    src_data: Arc<dyn Store>,
    dst_data: Arc<dyn Store>,
}

impl MigrationCoordinator {
    /// Non-blocking check: does this coordinator target the given
    /// `(src_repo, table_name)`? Skips on lock contention (caller
    /// retries via the normal phase check in start_migration). Used
    /// to detect concurrent migrations on the same table.
    pub fn targets_table(&self, src_repo: &str, table_name: &str) -> bool {
        match self.state.try_lock() {
            Ok(s) => s.table_name == table_name && s.src_repo == src_repo,
            Err(_) => false,
        }
    }
}

impl MigrationCoordinator {
    pub fn new(
        state: MigrationState,
        shadow_log: Arc<MigrationShadowLog>,
        src_data: Arc<dyn Store>,
        dst_data: Arc<dyn Store>,
    ) -> Self {
        Self {
            state: Mutex::new(state),
            shadow_log,
            src_data,
            dst_data,
        }
    }

    pub async fn state(&self) -> MigrationState {
        self.state.lock().await.clone()
    }

    pub async fn phase(&self) -> MigrationPhase {
        self.state.lock().await.phase
    }

    pub async fn migration_id(&self) -> String {
        self.state.lock().await.id.clone()
    }

    pub fn shadow_log(&self) -> &Arc<MigrationShadowLog> {
        &self.shadow_log
    }

    pub async fn run_snapshot(&self) -> DbResult<u64> {
        {
            let mut s = self.state.lock().await;
            if s.phase != MigrationPhase::ShadowStarted {
                return Err(DbError::Internal(format!(
                    "snapshot requires ShadowStarted, got {}",
                    s.phase
                )));
            }
            s.snapshot_lsn = self.shadow_log.current_lsn();
            s.phase = MigrationPhase::Snapshotting;
        }

        let mut stream = self.src_data.iter_stream(256);
        let mut copied = 0u64;
        while let Some(batch) = stream.next().await {
            let records = batch?;
            if records.is_empty() {
                break;
            }
            let items: Vec<(RecordKey, Bytes)> = records.into_iter().map(|(k, v)| (k, v)).collect();
            let count = items.len() as u64;
            self.dst_data.set_many(items).await?;
            copied += count;
        }

        {
            let mut s = self.state.lock().await;
            s.records_copied = copied;
            s.phase = MigrationPhase::Draining;
        }
        Ok(copied)
    }

    pub async fn drain_shadow_log(&self) -> DbResult<u64> {
        let start_lsn = {
            let s = self.state.lock().await;
            if s.phase != MigrationPhase::Draining {
                return Err(DbError::Internal(format!(
                    "drain requires Draining, got {}",
                    s.phase
                )));
            }
            s.last_lsn_applied.max(s.snapshot_lsn) + 1
        };

        let entries = self.shadow_log.read_from(start_lsn).await?;
        let mut applied = 0u64;
        for entry in &entries {
            match &entry.op {
                ShadowOp::Put { record_id, value } => {
                    let key = RecordKey::from(record_id.as_bytes().to_vec());
                    self.dst_data.set(key, Bytes::from(value.clone())).await?;
                }
                ShadowOp::Delete { record_id } => {
                    let key = RecordKey::from(record_id.as_bytes().to_vec());
                    // NotFound is benign — record already absent on dst;
                    // other errors propagate.
                    match self.dst_data.remove(key).await {
                        Ok(_) | Err(DbError::NotFound(_)) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            applied += 1;
        }

        if let Some(last) = entries.last() {
            self.state.lock().await.last_lsn_applied = last.lsn;
        }

        Ok(applied)
    }

    pub async fn shadow_lag(&self) -> u64 {
        let (last_applied, snapshot_lsn) = {
            let s = self.state.lock().await;
            (s.last_lsn_applied, s.snapshot_lsn)
        };
        let current = self.shadow_log.current_lsn();
        current.saturating_sub(last_applied.max(snapshot_lsn))
    }

    pub async fn drain_until_caught_up(&self, max_lag: u64) -> DbResult<u64> {
        let mut total = 0u64;
        loop {
            let applied = self.drain_shadow_log().await?;
            total += applied;
            if self.shadow_lag().await <= max_lag {
                break;
            }
            if applied == 0 {
                break;
            }
        }
        Ok(total)
    }

    pub async fn mark_cutover_ready(&self) -> DbResult<()> {
        let mut s = self.state.lock().await;
        if s.phase != MigrationPhase::Draining {
            return Err(DbError::Internal(format!(
                "cutover_ready requires Draining, got {}",
                s.phase
            )));
        }
        s.phase = MigrationPhase::CutoverReady;
        Ok(())
    }

    pub async fn final_drain_and_commit(&self) -> DbResult<u64> {
        {
            let s = self.state.lock().await;
            if s.phase != MigrationPhase::CutoverReady {
                return Err(DbError::Internal(format!(
                    "final_drain requires CutoverReady, got {}",
                    s.phase
                )));
            }
        }

        let start_lsn = self.state.lock().await.last_lsn_applied + 1;
        let entries = self.shadow_log.read_from(start_lsn).await?;
        let mut applied = 0u64;
        for entry in &entries {
            match &entry.op {
                ShadowOp::Put { record_id, value } => {
                    let key = RecordKey::from(record_id.as_bytes().to_vec());
                    self.dst_data.set(key, Bytes::from(value.clone())).await?;
                }
                ShadowOp::Delete { record_id } => {
                    let key = RecordKey::from(record_id.as_bytes().to_vec());
                    match self.dst_data.remove(key).await {
                        Ok(_) | Err(DbError::NotFound(_)) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            applied += 1;
        }
        if let Some(last) = entries.last() {
            self.state.lock().await.last_lsn_applied = last.lsn;
        }

        self.state.lock().await.phase = MigrationPhase::Committed;
        Ok(applied)
    }

    pub async fn rollback(&self) -> DbResult<()> {
        let phase = self.phase().await;
        if phase == MigrationPhase::Committed {
            return Err(DbError::Internal(
                "cannot rollback a committed migration".into(),
            ));
        }
        self.shadow_log.purge().await?;
        self.state.lock().await.phase = MigrationPhase::RolledBack;
        Ok(())
    }

    pub async fn verify_record_count(&self) -> DbResult<(u64, u64)> {
        let mut src_count = 0u64;
        let mut stream = self.src_data.iter_stream(256);
        while let Some(batch) = stream.next().await {
            src_count += batch?.len() as u64;
        }

        let mut dst_count = 0u64;
        let mut stream = self.dst_data.iter_stream(256);
        while let Some(batch) = stream.next().await {
            dst_count += batch?.len() as u64;
        }

        Ok((src_count, dst_count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_types::types::record_id::RecordId;

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
            "redb".into(),
            None,
        );
        let coord = MigrationCoordinator::new(state, shadow.clone(), src.clone(), dst.clone());

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
            "redb".into(),
            None,
        );
        let coord = MigrationCoordinator::new(state, shadow.clone(), src, dst);

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
            "redb".into(),
            None,
        );
        let coord = MigrationCoordinator::new(state, shadow, src, dst);

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
            "redb".into(),
            None,
        );
        let coord = MigrationCoordinator::new(state, shadow, src, dst);

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
            "redb".into(),
            None,
        );
        let coord = MigrationCoordinator::new(state, shadow.clone(), src.clone(), dst.clone());

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
}
