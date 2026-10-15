//! Clean-shutdown method cluster for [`RepoInstance`]:
//! `shutdown_clean`, `worker_reopen_required`, `flush_buffers`.

use shamir_storage::error::{DbError, DbResult};

use super::RepoInstance;

impl RepoInstance {
    /// Caller MUST stop commit/apply admission before calling this method.
    /// One bounded budget covers quiescence, drain/store flush and checkpoint.
    /// cancel-safe: NO — drain/flush may partially finish; J is never requested
    /// after an earlier failure or timeout. Once admitted, checkpoint I/O can
    /// finish independently; a cancelled caller never reports shutdown success.
    pub async fn shutdown_clean(&self, budget: std::time::Duration) -> DbResult<()> {
        let deadline = tokio::time::Instant::now() + budget;
        self.quiesce_commits(budget).await?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        tokio::time::timeout(remaining, async {
            self.worker_lifecycle
                .shutdown(remaining)
                .await
                .map_err(|_| DbError::Storage("worker shutdown timed out".into()))?;
            self.flush_buffers().await?;
            self.finish_clean_shutdown(deadline).await
        })
        .await
        .map_err(|_| DbError::Storage("clean shutdown timed out".into()))?
    }

    /// PR6b-2b: sticky worker-quarantine flag (`reopen_required`).
    pub fn worker_reopen_required(&self) -> bool {
        self.worker_lifecycle.reopen_required()
    }

    /// Flush this repo's in-memory buffers to their durable backing.
    ///
    /// Drains the `__tx__` store and every table's data + info stores.
    /// In-memory stores are no-ops. Best-effort: individual errors are
    /// logged and skipped; returns the first error encountered (if any)
    /// after attempting all stores.
    pub async fn flush_buffers(&self) -> DbResult<()> {
        let mut first_err: Option<DbError> = None;

        // D2 P1d-2b: drain the inflight WAL tail into `history` before flushing
        // the durable stores. Post-cutover the ack-path writes only the
        // in-memory overlay; the value is durable in `history` only after the
        // drainer replays the WAL entry. A graceful flush therefore drains
        // first so the committed-but-not-yet-drained tail lands durably (and
        // the WAL markers truncate). If no drainer was ever spawned (no commit
        // happened) this is a cheap no-op pass. Best-effort: a drain error is
        // logged and recovery on the next open still converges.
        if let Err(e) = self.drainer().drain_all(self).await {
            log::warn!("flush_buffers: drain_all {}: {}", self.name, e);
            if first_err.is_none() {
                first_err = Some(e);
            }
        }

        match self.tx_info_store().await {
            Ok(store) => {
                if let Err(e) = store.flush().await {
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

            if let Err(e) = table.data_store().flush().await {
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
            if let Err(e) = table.info_store().flush().await {
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
        // shutdown. Pre-Stage-I this was a per-table loop; now that every
        // table shares the repo interner, one persist covers all of them
        // (the manager is Arc-shared so any table's `interner()` handle is
        // the same live state). After this, WAL entries whose deltas covered
        // these ids can be safely truncated on next boot.
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

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
