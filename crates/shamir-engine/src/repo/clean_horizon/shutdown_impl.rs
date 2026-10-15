//! Clean-shutdown method cluster for [`RepoInstance`]:
//! `shutdown_clean`, `worker_reopen_required`, `flush_buffers`.

use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Repo;
use std::sync::Arc;

use crate::tx::cold_reconcile::lifecycle::{RepoUnitLifecycle, WorkerTerminal};

use super::RepoInstance;

impl RepoInstance {
    /// Caller MUST stop commit/apply admission before calling this method.
    /// One absolute deadline covers lifecycle drain, commit quiescence,
    /// buffer flush and clean-horizon checkpoint. Any error quarantines an
    /// installed lifecycle; a never-installed lifecycle completes immediately.
    pub async fn shutdown_clean(&self, budget: std::time::Duration) -> DbResult<()> {
        Box::pin(self.shutdown_clean_inner(budget)).await
    }

    async fn shutdown_clean_inner(&self, budget: std::time::Duration) -> DbResult<()> {
        let deadline = tokio::time::Instant::now() + budget;
        let lifecycle = self.worker_lifecycle();
        if let Some(lifecycle) = lifecycle {
            lifecycle.close();
        }

        let result = tokio::time::timeout_at(deadline, async {
            if let Some(lifecycle) = lifecycle {
                match Box::pin(lifecycle.shutdown_until(deadline)).await {
                    Ok(WorkerTerminal::Completed) => {}
                    Ok(WorkerTerminal::Failed(msg)) => {
                        return Err(DbError::Storage(format!("worker failed: {msg}")));
                    }
                    Err(_) => return Err(DbError::Storage("worker shutdown timed out".into())),
                }
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout_at(deadline, self.quiesce_commits(remaining)).await {
                Ok(result) => result?,
                Err(_) => {
                    return Err(DbError::Internal(format!(
                        "commit quiescence timed out after {remaining:?} for repo '{}'",
                        self.name
                    )))
                }
            }
            match tokio::time::timeout_at(deadline, self.flush_buffers()).await {
                Ok(result) => result?,
                Err(_) => {
                    return Err(DbError::Storage(
                        "clean shutdown timed out during flush".into(),
                    ))
                }
            }
            match tokio::time::timeout_at(deadline, self.finish_clean_shutdown(deadline)).await {
                Ok(result) => result,
                Err(_) => Err(DbError::Storage("clean shutdown timed out".into())),
            }
        })
        .await;

        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(err)) => {
                if let Some(lifecycle) = lifecycle {
                    lifecycle.quarantine("clean shutdown failed".to_string());
                }
                Err(err)
            }
            Err(_) => {
                if let Some(lifecycle) = lifecycle {
                    lifecycle.quarantine("clean shutdown timed out".to_string());
                }
                Err(DbError::Storage("clean shutdown timed out".into()))
            }
        }
    }

    /// Sticky `reopen_required` flag; never-owned foreground instances are safe.
    pub fn worker_reopen_required(&self) -> bool {
        self.worker_lifecycle()
            .map(|lifecycle| lifecycle.reopen_required())
            .unwrap_or(false)
    }

    /// Shared lifecycle of foreground handles, if present. Background clones
    /// intentionally have no foreground lifecycle owner.
    fn worker_lifecycle(&self) -> Option<&Arc<RepoUnitLifecycle>> {
        self.worker_lifecycle
            .as_ref()
            .map(|foreground| foreground.lifecycle())
    }

    /// Flush this repo's in-memory buffers to their durable backing.
    ///
    /// Drains the `__tx__` store and every table's data + info stores, then
    /// issues ONE backend persist (`Repo::persist_once`) — the old per-store
    /// `flush()` loop fsynced fjall's single per-Database journal once per
    /// store (~N identical full-database fsyncs per shutdown).
    /// In-memory stores are no-ops. Best-effort: individual errors are
    /// logged and skipped; returns the first error encountered (if any)
    /// after attempting all stores.
    pub async fn flush_buffers(&self) -> DbResult<()> {
        let mut first_err: Option<DbError> = None;

        // Drain the inflight WAL tail into `history` before flushing durable stores.
        // Post-cutover the ack-path writes only the in-memory overlay; the value
        // is durable in `history` only after the drainer replays the WAL entry.
        // Graceful shutdown drains first so the committed-but-not-yet-drained
        // tail lands durably (and WAL markers truncate). Without a spawned
        // drainer (no commit), this is a cheap no-op. A drain error is returned
        // after subsequent stores are still flushed; recovery on next open can
        // then converge from WAL.
        if let Err(e) = self.drainer().drain_all(self).await {
            log::warn!("flush_buffers: drain_all {}: {}", self.name, e);
            if first_err.is_none() {
                first_err = Some(e);
            }
        }

        match self.tx_info_store().await {
            Ok(store) => {
                if let Err(e) = store.flush_drain().await {
                    log::warn!("flush_buffers: tx_info_store {}: {}", self.name, e);
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }

        for table_name in self.list_table_names() {
            let table = match self.get_table(&table_name).await {
                Ok(t) => t,
                Err(e) => {
                    log::warn!(
                        "flush_buffers: get_table {}/{}: {}",
                        self.name,
                        table_name,
                        e
                    );
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                    continue;
                }
            };
            if let Err(e) = table.data_store().flush_drain().await {
                log::warn!(
                    "flush_buffers: data_store {}/{}: {}",
                    self.name,
                    table_name,
                    e
                );
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
            if let Err(e) = table.info_store().flush_drain().await {
                log::warn!(
                    "flush_buffers: info_store {}/{}: {}",
                    self.name,
                    table_name,
                    e
                );
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }

        // Stage I: persist the single per-repo interner ONCE on graceful
        // shutdown. Previously persistence ran per table; the shared manager
        // now owns the repo-wide namespace, so one persist covers every table.
        // WAL entries whose deltas cover these ids can be truncated on next boot.
        match self.repo_interner().await {
            Ok(repo_interner) => {
                if let Err(e) = repo_interner.persist().await {
                    log::warn!("flush_buffers: repo interner persist {}: {}", self.name, e);
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }

        // ONE durability barrier for every store drained above. The old
        // per-store `flush()` loop fsynced fjall's single per-Database
        // journal once per store (~N identical full-database fsyncs per
        // shutdown — the Windows flush-stall multiplier). `flush_drain`
        // leaves every store's data in the backend's write path; this single
        // `persist(SyncAll)` makes ALL of it durable, byte-for-byte the same
        // end state as the loop. Non-fjall backends: default no-op — their
        // per-store `flush()` semantics are unchanged.
        if let Err(e) = self.repo.persist_once().await {
            log::warn!("flush_buffers: persist_once {}: {}", self.name, e);
            if first_err.is_none() {
                first_err = Some(e);
            }
        }

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
