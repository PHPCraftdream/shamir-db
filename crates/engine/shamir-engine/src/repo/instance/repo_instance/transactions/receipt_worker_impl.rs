use std::sync::Arc;

use crate::repo::RepoInstance;
use crate::tx::cold_reconcile::lifecycle::{InstallError, JobSubmitError};
use crate::tx::follower_durability::receipt::resume::worker::receipt_handler;
use crate::tx::follower_durability::receipt::{ReceiptApplyJob, ReceiptLease};
use crate::tx::{ApplyOutcome, ApplyPolicy, ReplicatedEvent};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;

impl RepoInstance {
    /// Lazy accessor for the repository-owned receipt worker lease — same
    /// `OnceCell` pattern as `tx_gate()`: the worker is installed (the only
    /// `tokio::spawn` of the receipt path, inside `install_worker`) on the
    /// FIRST submission and lives until shutdown or repo drop.
    pub(crate) async fn receipt_worker_installed(&self) -> DbResult<Arc<ReceiptLease>> {
        let lease = self
            .receipt_worker
            .get_or_try_init(|| async {
                let gate = self.tx_gate().await?;
                let lifecycle = self.worker_lifecycle.as_ref().ok_or_else(|| {
                    DbError::Internal(format!(
                        "receipt worker for repo {} has no foreground lifecycle",
                        self.name()
                    ))
                })?;
                let lease = ReceiptLease::new(gate, lifecycle.lifecycle().status());
                lifecycle
                    .lifecycle()
                    .install_worker(lease.clone(), receipt_handler)
                    .map_err(|error| match error {
                        InstallError::AlreadyInstalled => {
                            DbError::Internal("receipt worker already installed".into())
                        }
                        InstallError::ClosedOrForbidden => DbError::Internal(format!(
                            "receipt worker for repo {} cannot be installed: \
                             lifecycle closed or spawn forbidden",
                            self.name()
                        )),
                    })?;
                Ok::<Arc<ReceiptLease>, DbError>(Arc::new(lease))
            })
            .await?;
        Ok(Arc::clone(lease))
    }

    /// Apply entry: submit one bounded job to the lifecycle and await the
    /// oneshot reply. Only the REPLY is awaited — dropping this future at
    /// the await point drops the receiver, and the worker KEEPS RUNNING the
    /// job with its own repo clone. The typed submit errors keep the old
    /// branch's wording.
    pub(crate) async fn submit_receipt_job(
        &self,
        info: Arc<dyn Store>,
        events: Vec<ReplicatedEvent>,
        bookmark: u64,
        policy: ApplyPolicy,
    ) -> DbResult<ApplyOutcome> {
        // Lazy install on the first submission (no-op afterwards).
        self.receipt_worker_installed().await?;
        let lifecycle = self.worker_lifecycle.as_ref().ok_or_else(|| {
            DbError::Internal(format!("receipt worker for repo {} shut down", self.name()))
        })?;
        let payload = ReceiptApplyJob {
            repo: self.clone(),
            info,
            events,
            bookmark,
            policy,
        };
        let reply = lifecycle
            .lifecycle()
            .submit(payload)
            .map_err(|error| match error {
                JobSubmitError::Closing => {
                    DbError::Internal(format!("receipt worker for repo {} shut down", self.name()))
                }
                JobSubmitError::NotInitialized => {
                    DbError::Internal("receipt worker shut down".into())
                }
                JobSubmitError::Full => DbError::Internal(
                    "receipt worker queue full (capacity 4); retry the pull".into(),
                ),
            })?;
        // The worker owns the job: a dropped caller only drops `reply`.
        reply
            .await
            .map_err(|_| DbError::Internal("receipt worker dropped the reply".into()))?
    }

    /// Idle count of parked guards, for tests and diagnostics. `None` when
    /// the worker was never installed.
    #[cfg(all(test, feature = "fjall"))]
    pub(crate) async fn receipt_worker_parked_guards(&self) -> Option<usize> {
        self.receipt_worker.get().map(|lease| lease.parked_count())
    }

    /// Deterministic test wait: no receipt job is in flight on this repo's
    /// worker.
    #[cfg(all(test, feature = "fjall"))]
    #[expect(
        clippy::unwrap_used,
        reason = "guard: test helper requires an installed receipt worker"
    )]
    pub(crate) async fn receipt_worker_wait_idle(&self) {
        self.receipt_worker_installed()
            .await
            .unwrap()
            .wait_idle()
            .await;
    }
}
