//! Clean-shutdown method cluster for [`RepoInstance`]:
//! `shutdown_clean`, `worker_reopen_required`, `flush_buffers`.

use crate::tx::commit::core::timed_stage;
use shamir_storage::error::{DbError, DbResult};
use std::sync::Arc;

use crate::tx::apply_replicated::follower_durability::receipt::{ApplyOutcome, ReceiptApplyJob};
use crate::tx::cold_reconcile::lifecycle::{RepoWorkerLifecycle, WorkerTerminal};

use super::super::super::RepoInstance;

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
                match Box::pin(timed_stage(
                    &self.name,
                    "lifecycle_drain",
                    lifecycle.shutdown_until(deadline),
                ))
                .await
                {
                    Ok(WorkerTerminal::Completed) => {}
                    Ok(WorkerTerminal::Failed(msg)) => {
                        return Err(DbError::Storage(format!("worker failed: {msg}")));
                    }
                    Err(_) => return Err(DbError::Storage("worker shutdown timed out".into())),
                }
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout_at(
                deadline,
                timed_stage(
                    &self.name,
                    "quiesce_commits",
                    self.quiesce_commits(remaining),
                ),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => {
                    return Err(DbError::Internal(format!(
                        "commit quiescence timed out after {remaining:?} for repo '{}'",
                        self.name
                    )));
                }
            }
            match tokio::time::timeout_at(
                deadline,
                timed_stage(&self.name, "flush_buffers", self.flush_buffers()),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => {
                    return Err(DbError::Storage(
                        "clean shutdown timed out during flush".into(),
                    ));
                }
            }
            match tokio::time::timeout_at(
                deadline,
                timed_stage(
                    &self.name,
                    "finish_clean_shutdown",
                    self.finish_clean_shutdown(deadline),
                ),
            )
            .await
            {
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
    fn worker_lifecycle(
        &self,
    ) -> Option<&Arc<RepoWorkerLifecycle<ReceiptApplyJob, DbResult<ApplyOutcome>>>> {
        self.core
            .worker_lifecycle
            .as_ref()
            .map(|foreground| foreground.lifecycle())
    }

    /// Flush this repo's in-memory buffers to their durable backing.
    ///
    /// Drains the `__tx__` store and every table's data + info stores, then
    /// issues ONE backend persist (`Repo::persist_once`). The body lives on
    /// `RepoCore::flush_buffers`.
    pub async fn flush_buffers(&self) -> DbResult<()> {
        self.core.flush_buffers().await
    }
}
