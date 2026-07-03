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
//! `hello` returns an empty repo advertisement — the loop only uses it to
//! seed the max-seen epoch.

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use shamir_db::ShamirDb;
use shamir_query_types::wire::repl::ReplResponse;
use tokio::sync::Mutex;

use super::error::ReplError;
use super::source::ReplSource;

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
        // Return an empty repo advertisement — the loop only needs the
        // epoch here. Advertising the real repos would require listing dbs
        // + repos and is not needed for the single-(db,repo) loop tests.
        Ok(ReplResponse::Hello {
            leader_epoch: self.epoch(),
            repos: Vec::new(),
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

        // Reuse the same journal-read path as the wire handler.
        // Effective limit floor of 1, mirroring `handle_pull`.
        let effective_limit = (limit.max(1)) as usize;
        let mut jr = match self
            .leader
            .read_changelog_from_journal(db, repo, from_version, effective_limit)
            .await
        {
            Some(jr) => jr,
            None => {
                return Ok(ReplResponse::Error {
                    leader_epoch: self.epoch(),
                    code: "unknown_repo".into(),
                    message: format!("repository '{db}/{repo}' not found or journal unavailable"),
                });
            }
        };

        // Optional long-poll: if the first read was empty and the caller
        // supplied a positive wait budget, poll until events land or the
        // budget expires. Mirrors `handle_pull`'s behaviour.
        if jr.events.is_empty() {
            if let Some(ms) = wait_ms {
                if ms > 0 {
                    let deadline =
                        std::time::Instant::now() + std::time::Duration::from_millis(u64::from(ms));
                    while jr.events.is_empty() {
                        let now = std::time::Instant::now();
                        if now >= deadline {
                            break;
                        }
                        let remaining = deadline - now;
                        let step = remaining.min(std::time::Duration::from_millis(50));
                        tokio::time::sleep(step).await;
                        match self
                            .leader
                            .read_changelog_from_journal(db, repo, from_version, effective_limit)
                            .await
                        {
                            Some(fresh) => jr = fresh,
                            None => break,
                        }
                    }
                }
            }
        }

        let events_bytes = rmp_serde::to_vec_named(&jr.events)
            .map_err(|e| ReplError::Transport(format!("encode events: {e}")))?;

        let current_version = self
            .leader
            .current_commit_version(db, repo)
            .await
            .unwrap_or(from_version);

        Ok(ReplResponse::Pull {
            leader_epoch: self.epoch(),
            events: events_bytes,
            gap_at: jr.gap_at,
            current_version,
        })
    }
}
