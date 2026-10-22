use std::time::Duration;

use futures::StreamExt;

use shamir_storage::error::{DbError, DbResult};

use crate::repo::RepoInstance;

/// Offline caller owns admission: no handle may be shared with writers.
/// Refuses allocation holes before any clean-horizon request.
pub async fn prepare_seed(repo: &RepoInstance) -> DbResult<u64> {
    let mut names = repo.list_table_names();
    names.sort();
    for name in &names {
        let table = repo.get_table(name).await?;
        if table.mvcc_store().is_none() {
            return Err(DbError::Storage("seed requires MVCC".into()));
        }
    }
    repo.quiesce_commits(Duration::from_secs(30)).await?;
    let gate = repo.tx_gate().await?;
    let version = repo.consistent_version().await?;
    if version != gate.version_allocation_high_water_mark() {
        return Err(DbError::Storage("unsettled seed versions".into()));
    }
    if repo
        .certified_gap_interval()
        .await?
        .is_some_and(|(_, through)| version < through)
    {
        return Err(DbError::Storage("seed version below certified gap".into()));
    }
    repo.drainer().drain_all(repo).await?;
    repo.flush_all_history().await?;
    // snapshot_stream skips unrecognised history keys; seed export must instead
    // reject them. Timestamp entries are the documented 0x00 + u64 namespace.
    for name in &names {
        let table = repo.get_table(name).await?;
        let mvcc = table
            .mvcc_store()
            .ok_or_else(|| DbError::Storage("seed requires MVCC".into()))?;
        let mut history = mvcc.history_store().iter_stream(1);
        while let Some(batch) = history.next().await {
            for (key, value) in batch? {
                if key.as_ref().len() == 9 && key.as_ref()[0] == 0 {
                    if value.len() != 8 {
                        return Err(DbError::Storage("corrupt history timestamp".into()));
                    }
                } else {
                    let (_, row_version) = shamir_tx::decode_version_key(key.as_ref())
                        .ok_or_else(|| DbError::Storage("corrupt seed history key".into()))?;
                    if row_version == 0 || row_version > version {
                        return Err(DbError::Storage(
                            "history outside settled seed horizon".into(),
                        ));
                    }
                }
                if value.len() > 16 * 1024 * 1024 {
                    return Err(DbError::Storage("seed history row exceeds bounds".into()));
                }
            }
        }
    }
    repo.flush_buffers().await?;
    for name in &names {
        let table = repo.get_table(name).await?;
        table.counter().persist().await?;
        table.info_store().flush().await?;
    }
    repo.flush_seed_metadata().await?;
    if repo.consistent_version().await? != version {
        return Err(DbError::Storage("seed version changed during flush".into()));
    }
    Ok(version)
}
