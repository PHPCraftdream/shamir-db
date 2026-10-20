//! R1-c — in-process [`ReplSource`] for tests.
//!
//! Wraps the leader's `Arc<ShamirDb>` and builds [`ReplResponse`]s directly
//! from the durable journal (`read_changelog_from_journal` +
//! `current_commit_version`), bypassing the wire + SCRAM + authorisation
//! stack entirely. This is what the engine / loop tests use: it exercises
//! the real journal-read path and the real follower apply path, without the
//! network.
//!
//! The leader epoch is a configurable field (default `1`); tests that need
//! epoch-fencing regressions mutate it via [`InProcessReplSource::set_epoch`].
//!
//! `hello` advertises durable identity and certified intervals for every repo.

use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::ShamirDb;
use shamir_query_types::wire::repl::{ReplRepoInfo, ReplResponse};
use tokio::sync::Mutex;
#[cfg(test)]
use tokio::sync::Notify;

#[cfg(test)]
pub(crate) struct PullRetryHook {
    pub(crate) entered: Notify,
    pub(crate) release: Notify,
}

#[cfg(test)]
tokio::task_local! {
    pub(crate) static PULL_RETRY_HOOK: Arc<PullRetryHook>;
}

#[cfg(test)]
async fn pause_before_retry_page() {
    if let Ok(hook) = PULL_RETRY_HOOK.try_with(Arc::clone) {
        hook.entered.notify_one();
        hook.release.notified().await;
    }
}

use super::source::ReplSource;
use crate::replication::error::ReplError;
pub(crate) const MAX_PULL_EVENTS: u32 = 1024;

/// In-process [`ReplSource`] backed directly by a leader `Arc<ShamirDb>`.
///
/// `pull` reuses `ShamirDb::read_changelog_from_journal` (the same code the
/// wire-side `handle_pull` uses), encodes the events with `rmp_serde`, and
/// stamps the configured `leader_epoch` on every reply. `hello` returns the
/// configured epoch so the loop can seed its max-seen tracker.
///
/// The internal `epoch` is an `AtomicU64` so a test thread can mutate it
/// (to simulate a regressing leader) without taking a lock that the loop
/// task would contend on.
pub struct InProcessReplSource {
    leader: ShamirDb,
    epoch: AtomicU64,
    /// Serialises `pull` calls — the underlying journal read takes `&self`
    /// but we want deterministic ordering in tests with multiple concurrent
    /// pullers (none today, but cheap insurance).
    _pull_lock: Mutex<()>,
}

impl InProcessReplSource {
    /// Wrap a leader `ShamirDb` with `leader_epoch = 1`.
    pub fn new(leader: ShamirDb) -> Self {
        Self::with_epoch(leader, 1)
    }

    /// Wrap a leader `ShamirDb` with a custom starting `leader_epoch`.
    pub fn with_epoch(leader: ShamirDb, epoch: u64) -> Self {
        Self {
            leader,
            epoch: AtomicU64::new(epoch),
            _pull_lock: Mutex::new(()),
        }
    }

    /// Current configured leader epoch (read atomically).
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    /// Set the leader epoch (used by tests to simulate a regression or a
    /// bump).
    pub fn set_epoch(&self, epoch: u64) {
        self.epoch.store(epoch, Ordering::Release);
    }
}

#[async_trait]
impl ReplSource for InProcessReplSource {
    async fn hello(&self, _node_id: &str) -> Result<ReplResponse, ReplError> {
        let mut repos = Vec::new();
        for db in self.leader.list_dbs() {
            let Some(database) = self.leader.get_db(&db) else {
                continue;
            };
            for repo in database.list_repos() {
                let Some(instance) = database.get_repo(&repo) else {
                    continue;
                };
                if self
                    .leader
                    .has_resync_required_subscription_covering(&db, &repo)
                    .await
                    .map_err(|error| ReplError::Transport(error.to_string()))?
                {
                    return Ok(ReplResponse::Error {
                        leader_epoch: self.epoch(),
                        code: "downstream_resync_required".into(),
                        message: format!(
                            "repository '{db}/{repo}' is quarantined (resync_required)"
                        ),
                    });
                }
                let identity = instance
                    .repo_identity()
                    .await
                    .map_err(|e| ReplError::Transport(e.to_string()))?;
                let gap = instance
                    .certified_gap_interval()
                    .await
                    .map_err(|e| ReplError::Transport(e.to_string()))?;
                repos.push(ReplRepoInfo {
                    schema_digest: Some(
                        super::source::seed::fence::live_schema(&self.leader, &db, &repo).await?,
                    ),
                    settled_version: Some(
                        instance
                            .consistent_version()
                            .await
                            .map_err(|e| ReplError::Transport(e.to_string()))?,
                    ),
                    publications: super::source::seed::fence::publications(&self.leader).await?,
                    db: db.clone(),
                    repo,
                    current_version: instance
                        .current_commit_version()
                        .await
                        .map_err(|e| ReplError::Transport(e.to_string()))?,
                    journal_floor: 0,
                    repo_lineage_id: Some(identity.lineage_id),
                    history_epoch: Some(identity.history_epoch),
                    gap_first: gap.map(|(first, _)| first),
                    gap_through: gap.map(|(_, through)| through),
                });
            }
        }
        Ok(ReplResponse::Hello {
            leader_epoch: self.epoch(),
            repos,
        })
    }

    async fn pull(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: u32,
        wait_ms: Option<u32>,
    ) -> Result<ReplResponse, ReplError> {
        let _guard = self._pull_lock.lock().await;

        let Some(instance) = self.leader.get_db(db).and_then(|db| db.get_repo(repo)) else {
            return Ok(ReplResponse::Error {
                leader_epoch: self.epoch(),
                code: "unknown_repo".into(),
                message: format!("repository '{db}/{repo}' not found"),
            });
        };
        // S5c-X PR8b: refuse to serve a quarantined repo — the same
        // serve-entry refusal the wire handler performs via
        // `has_resync_required_subscription_covering`.
        if let Some(reason) = self.leader.read_quarantine_reason(db, repo) {
            return Ok(ReplResponse::Error {
                leader_epoch: self.epoch(),
                code: "downstream_resync_required".into(),
                message: format!(
                    "repository '{db}/{repo}' is quarantined (resync_required): {reason}"
                ),
            });
        }
        // Reuse the same journal-read path as the wire handler.
        // Effective limit floor of 1, mirroring `handle_pull`.
        let effective_limit = limit.clamp(1, MAX_PULL_EVENTS) as usize;
        let mut jr = match self
            .leader
            .read_changelog_for_replication(db, repo, from_version, effective_limit)
            .await
        {
            Ok(Some(jr)) => jr,
            Ok(None) => {
                return Ok(ReplResponse::Error {
                    leader_epoch: self.epoch(),
                    code: "unknown_repo".into(),
                    message: format!("repository '{db}/{repo}' not found"),
                });
            }
            Err(error) => {
                return Ok(ReplResponse::Error {
                    leader_epoch: self.epoch(),
                    code: if matches!(error, shamir_db::DbError::ResyncRequired { .. }) {
                        "downstream_resync_required"
                    } else {
                        "replication_read_error"
                    }
                    .into(),
                    message: error.to_string(),
                });
            }
        };

        // Optional long-poll: if the first read was empty and the caller
        // supplied a positive wait budget, poll until events land or the
        // budget expires. Mirrors `handle_pull`'s behaviour.
        if jr.0.is_empty() {
            if let Some(ms) = wait_ms.map(|ms| ms.min(30_000)) {
                if ms > 0 {
                    let deadline =
                        std::time::Instant::now() + std::time::Duration::from_millis(u64::from(ms));
                    while jr.0.is_empty() {
                        let now = std::time::Instant::now();
                        if now >= deadline {
                            break;
                        }
                        let remaining = deadline - now;
                        let step = remaining.min(std::time::Duration::from_millis(50));
                        tokio::time::sleep(step).await;
                        #[cfg(test)]
                        pause_before_retry_page().await;
                        match self
                            .leader
                            .read_changelog_for_replication(db, repo, from_version, effective_limit)
                            .await
                        {
                            Ok(Some(fresh)) => jr = fresh,
                            Ok(None) => {
                                return Ok(ReplResponse::Error {
                                    leader_epoch: self.epoch(),
                                    code: "unknown_repo".into(),
                                    message: format!("repository '{db}/{repo}' not found"),
                                });
                            }
                            Err(error) => {
                                return Ok(ReplResponse::Error {
                                    leader_epoch: self.epoch(),
                                    code: if matches!(
                                        error,
                                        shamir_db::DbError::ResyncRequired { .. }
                                    ) {
                                        "downstream_resync_required"
                                    } else {
                                        "replication_read_error"
                                    }
                                    .into(),
                                    message: error.to_string(),
                                });
                            }
                        }
                        if let Some(reason) = self.leader.read_quarantine_reason(db, repo) {
                            return Ok(ReplResponse::Error {
                                leader_epoch: self.epoch(),
                                code: "downstream_resync_required".into(),
                                message: format!(
                                    "repository '{db}/{repo}' is quarantined (resync_required): {reason}"
                                ),
                            });
                        }
                    }
                }
            }
        }

        let events_bytes = rmp_serde::to_vec_named(&jr.0)
            .map_err(|e| ReplError::Transport(format!("encode events: {e}")))?;

        let current_version = self
            .leader
            .current_commit_version(db, repo)
            .await
            .unwrap_or(from_version);

        let metadata = async {
            let identity = instance.repo_identity().await?;
            let gap = instance.certified_gap_interval().await?;
            Ok::<_, shamir_db::DbError>((identity, gap))
        }
        .await;
        let (identity, gap) = match metadata {
            Ok(metadata) => metadata,
            Err(error) => {
                return Ok(ReplResponse::Error {
                    leader_epoch: self.epoch(),
                    code: "replication_read_error".into(),
                    message: error.to_string(),
                });
            }
        };

        let schema_digest = super::source::seed::fence::live_schema(&self.leader, db, repo).await?;
        if let Some(reason) = self.leader.read_quarantine_reason(db, repo) {
            return Ok(ReplResponse::Error {
                leader_epoch: self.epoch(),
                code: "downstream_resync_required".into(),
                message: format!(
                    "repository '{db}/{repo}' is quarantined (resync_required): {reason}"
                ),
            });
        }

        Ok(ReplResponse::Pull {
            leader_epoch: self.epoch(),
            schema_digest: Some(schema_digest),
            events: events_bytes,
            repo_lineage_id: Some(identity.lineage_id),
            history_epoch: Some(identity.history_epoch),
            gap_first: gap.map(|(first, _)| first),
            gap_through: gap.map(|(_, through)| through),
            gap_at: jr.1,
            current_version,
        })
    }
}
