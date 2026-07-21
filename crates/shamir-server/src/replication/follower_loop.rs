//! R1-c — `run_follower_loop`: the background follower engine
//! (REPLICATION §4/§5.2/§5.3/§5.6).
//!
//! Responsibilities, in order per iteration:
//!   1. [`hello`](super::source::ReplSource::hello) once at start to seed the
//!      max-seen `leader_epoch` (VR-style fencing, §5.2).
//!   2. Loop (until cancelled):
//!      - read the durable `replication_bookmark` for `(db, repo)` on the
//!        follower;
//!      - `pull(db, repo, bookmark+1, limit, wait_ms=poll_wait_ms)`;
//!      - **epoch-fence (§5.2):** if the reply's epoch is strictly lower
//!        than the max seen, terminate with [`ReplError::StaleLeaderEpoch`];
//!        otherwise record the new max;
//!      - **gap-stop (§5.3 / R1 simplification):** if the reply carries
//!        `gap_at: Some(g)` with `g > from_version`, the follower is
//!        permanently missing `[from_version, g)` — log at `warn!`/`error!`
//!        and terminate with [`ReplError::JournalGap`] WITHOUT applying any
//!        events in that same reply (the supervisor marks the subscription
//!        `resync_required`; full automated snapshot reseed is R2);
//!      - decode the events payload (`Vec<ChangelogEvent>`);
//!      - for each event in order: `apply_replicated(event, bookmark)` →
//!        on `Applied`, `advance_replication_bookmark(event.commit_version)`
//!        (the LEADER version, not the follower-local one); on `Skipped`,
//!        no-op (idempotent re-delivery);
//!      - if no events were returned, the leader already long-polled
//!        `wait_ms`, so a small backoff is optional but harmless.
//!
//! **§5.6 invariant:** this is a background task. It holds NO follower locks
//! and never blocks the follower's own commits. Transient transport errors
//! are logged + retried with backoff; only [`ReplError::StaleLeaderEpoch`]
//! terminates the loop.

use std::sync::Arc;

use shamir_db::engine::{tx::ApplyOutcome, ChangelogEvent};
use shamir_db::ShamirDb;
use shamir_query_types::wire::repl::ReplResponse;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::error::ReplError;
use super::source::{leader_epoch_of, ReplSource};

/// Default pull batch size (REPLICATION §6.1 `pull_limit`). Each pull returns
/// up to this many events; a larger batch amortises the per-pull overhead,
/// a smaller one bounds the work per iteration.
const DEFAULT_PULL_LIMIT: u32 = 1000;

/// Small backoff inserted after an iteration that returned no events, even
/// though the leader already long-polled `wait_ms`. Keeps the loop polite
/// without relying solely on the leader's wait budget.
const IDLE_BACKOFF_MS: u64 = 50;

/// Initial backoff for transient transport errors. Doubled on each
/// consecutive failure, capped at `MAX_BACKOFF_MS`. Reset to this on the
/// first success after a failure streak.
const INITIAL_BACKOFF_MS: u64 = 200;

/// Cap for the transient-error exponential backoff. Matches the
/// `reconnect_backoff_ms.max` tunable in REPLICATION §6.1.
const MAX_BACKOFF_MS: u64 = 30_000;

/// Configuration for [`run_follower_loop`]: per-repo identity + tunables,
/// grouped so the loop entry point stays under the argument-count lint and
/// callers can build the config once and reuse it across restarts.
#[derive(Debug, Clone)]
pub struct FollowerLoopConfig {
    /// Stable identity advertised in `ReplHello`.
    pub node_id: String,
    /// Target database on both leader and follower (must match).
    pub db: String,
    /// Target repo on both leader and follower (must match).
    pub repo: String,
    /// Long-poll budget forwarded to each `pull` (ms). The leader blocks
    /// up to this long waiting for new events before returning an empty
    /// batch (REPLICATION §5.1).
    pub poll_wait_ms: u32,
    /// Test/bound hook: if `Some(n)`, the loop exits cleanly after `n`
    /// iterations. `None` = run forever (until `cancel` fires). Production
    /// callers pass `None`; tests pass a small cap to avoid relying on
    /// cancellation timing.
    pub max_iterations: Option<usize>,
}

impl FollowerLoopConfig {
    /// Build a config for one `(db, repo)` follower with the given poll
    /// budget and NO iteration cap (run until cancelled).
    pub fn new(node_id: impl Into<String>, db: impl Into<String>, repo: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            db: db.into(),
            repo: repo.into(),
            poll_wait_ms: 5000,
            max_iterations: None,
        }
    }

    /// Override the long-poll budget (ms).
    #[must_use]
    pub fn with_poll_wait_ms(mut self, ms: u32) -> Self {
        self.poll_wait_ms = ms;
        self
    }

    /// Set the iteration cap (mainly for tests).
    #[must_use]
    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = Some(n);
        self
    }
}

/// Run the follower replication pull-loop for one `(db, repo)` against
/// `source`, applying events to `db` (the follower's local `ShamirDb`).
///
/// The loop runs until:
///   * `cancel` is cancelled (graceful shutdown) → returns `Ok(())`;
///   * a [`ReplError::StaleLeaderEpoch`] is observed → returns the error
///     (the loop MUST stop; a stale leader is a fencing violation);
///   * a [`ReplError::JournalGap`] is observed → returns the error (the loop
///     MUST stop; the follower is permanently missing the gapped range and
///     must not silently resume past it — see the module docs);
///   * `cfg.max_iterations` is reached, if `Some` (used by tests to bound
///     the loop without relying on cancellation).
///
/// `cfg.poll_wait_ms` is forwarded to each `pull` as the long-poll budget;
/// the leader blocks up to that many milliseconds waiting for new events
/// before returning an empty batch (§5.1).
///
/// **§5.6:** the loop owns NO follower locks and never participates in the
/// follower's commit path. It is safe to `tokio::spawn` this and let it run
/// for the lifetime of the follower.
pub async fn run_follower_loop(
    db: Arc<ShamirDb>,
    source: Arc<dyn ReplSource>,
    cfg: FollowerLoopConfig,
    cancel: CancellationToken,
) -> Result<(), ReplError> {
    let FollowerLoopConfig {
        node_id,
        db: db_name,
        repo,
        poll_wait_ms,
        max_iterations,
    } = cfg;
    // 1. hello → seed max-seen epoch (§5.2).
    let mut max_seen_epoch = match source.hello(&node_id).await {
        Ok(resp) => leader_epoch_of(&resp),
        // A hello that already trips fencing is fatal — propagate.
        Err(e @ ReplError::StaleLeaderEpoch { .. }) => return Err(e),
        Err(e) => {
            // A transient hello failure: we cannot seed the epoch, so treat
            // it as a backoff-worthy transport error and let the loop's
            // backoff machinery handle the retry. We seed the epoch at 0
            // (the minimum) so the first real pull reply still fences on
            // regression correctly.
            warn!(
                node_id = %node_id,
                error = %e,
                "follower_loop: initial hello failed; will retry with backoff"
            );
            0
        }
    };
    info!(
        node_id = %node_id,
        db = %db_name,
        repo = %repo,
        leader_epoch = max_seen_epoch,
        "follower_loop: started"
    );

    let mut backoff_ms = INITIAL_BACKOFF_MS;
    let mut iter = 0usize;

    loop {
        // Cancellation check — cheap and race-free (CancellationToken).
        if cancel.is_cancelled() {
            info!(node_id = %node_id, "follower_loop: cancelled, exiting");
            return Ok(());
        }
        if let Some(cap) = max_iterations {
            if iter >= cap {
                debug!(
                    node_id = %node_id,
                    iterations = iter,
                    "follower_loop: reached max_iterations, exiting"
                );
                return Ok(());
            }
        }
        iter += 1;

        // 2. read the durable bookmark on the follower (R1-b).
        let repo_instance = match db.get_db(&db_name).and_then(|d| d.get_repo(&repo)) {
            Some(r) => r,
            None => {
                // Follower repo doesn't exist yet — backoff and retry; an
                // admin DDL may create it later.
                let e = ReplError::UnknownFollowerRepo {
                    db: db_name.clone(),
                    repo: repo.clone(),
                };
                warn!(node_id = %node_id, error = %e, "follower_loop: backing off");
                sleep_backoff(&cancel, &mut backoff_ms).await;
                continue;
            }
        };
        let bookmark = match repo_instance.replication_bookmark().await {
            Ok(v) => v,
            Err(e) => {
                let re = ReplError::Bookmark(e.to_string());
                warn!(node_id = %node_id, error = %re, "follower_loop: bookmark read failed");
                sleep_backoff(&cancel, &mut backoff_ms).await;
                continue;
            }
        };

        // 3. pull — always from the durable bookmark; a journal gap is now
        // terminal (see step 6), so there is no cursor-shift state to track
        // across iterations.
        let from_version = bookmark + 1;
        let resp = match source
            .pull(
                &db_name,
                &repo,
                from_version,
                DEFAULT_PULL_LIMIT,
                Some(poll_wait_ms),
            )
            .await
        {
            Ok(r) => r,
            Err(e @ ReplError::StaleLeaderEpoch { .. }) => return Err(e),
            Err(e) => {
                warn!(node_id = %node_id, error = %e, "follower_loop: pull failed, backing off");
                sleep_backoff(&cancel, &mut backoff_ms).await;
                continue;
            }
        };

        // 4. epoch-fence (§5.2) — check BEFORE touching the payload.
        let resp_epoch = leader_epoch_of(&resp);
        if resp_epoch < max_seen_epoch {
            // Stale leader — terminate the loop.
            return Err(ReplError::StaleLeaderEpoch {
                observed: resp_epoch,
                max_seen: max_seen_epoch,
            });
        }
        if resp_epoch > max_seen_epoch {
            debug!(
                node_id = %node_id,
                old = max_seen_epoch,
                new = resp_epoch,
                "follower_loop: leader epoch advanced"
            );
            max_seen_epoch = resp_epoch;
        }

        // Reset backoff on any successful (non-fencing) pull reply.
        backoff_ms = INITIAL_BACKOFF_MS;

        // 5. dispatch on the reply payload.
        let (events_bytes, gap_at) = match resp {
            ReplResponse::Pull { events, gap_at, .. } => (events, gap_at),
            ReplResponse::Error { code, message, .. } => {
                // The leader returned a structured error (e.g. denied_repo,
                // unknown_repo). Treat as transient — log + backoff.
                warn!(
                    node_id = %node_id,
                    code = %code,
                    "follower_loop: leader returned error: {message}"
                );
                sleep_backoff(&cancel, &mut backoff_ms).await;
                continue;
            }
            ReplResponse::Hello { .. } => {
                // Unexpected reply variant for a pull — skip this iteration.
                warn!(
                    node_id = %node_id,
                    "follower_loop: expected Pull, got Hello; skipping iteration"
                );
                continue;
            }
        };

        // 6. gap-stop (§5.3 / R1 simplification). A gap means the follower
        // is permanently missing `[from_version, g)` — this is now a
        // terminal condition, NOT a silent skip. Check and return BEFORE
        // decoding/applying any events that may have accompanied this same
        // reply (the events, if any, must not be applied).
        if let Some(g) = gap_at {
            if g > from_version {
                warn!(
                    node_id = %node_id,
                    gap_at = g,
                    from = from_version,
                    "follower_loop: leader reports journal gap; STOPPING the loop \
                     (not skipping) — data in [from_version, gap_at) is permanently \
                     missing; the supervisor will mark this subscription \
                     resync_required (full automated snapshot reseed is R2)"
                );
                return Err(ReplError::JournalGap {
                    gap_at: g,
                    from_version,
                });
            }
            // `g <= from_version` — the gap is behind us, ignore and proceed.
        }

        // 7. decode + apply each event in order.
        let events: Vec<ChangelogEvent> = match rmp_serde::from_slice(&events_bytes) {
            Ok(v) => v,
            Err(e) => {
                let re = ReplError::Decode(e.to_string());
                warn!(node_id = %node_id, error = %re, "follower_loop: decode failed, backing off");
                sleep_backoff(&cancel, &mut backoff_ms).await;
                continue;
            }
        };

        if events.is_empty() {
            // The leader already long-polled `poll_wait_ms`; a tiny extra
            // backoff keeps us polite but is not required.
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                _ = tokio::time::sleep(std::time::Duration::from_millis(IDLE_BACKOFF_MS)) => {}
            }
            continue;
        }

        for ev in &events {
            match repo_instance.apply_replicated(ev, bookmark).await {
                Ok(ApplyOutcome::Applied { .. }) => {
                    // Advance the durable bookmark to the LEADER commit
                    // version (not the follower-local version). This is the
                    // idempotency key for re-delivery.
                    if let Err(e) = repo_instance
                        .advance_replication_bookmark(ev.commit_version)
                        .await
                    {
                        // Bookmark persist failed — log and stop advancing
                        // this batch. The next iteration re-reads the old
                        // bookmark and re-applies (idempotently) the events
                        // whose bookmark didn't advance. Do NOT advance the
                        // in-memory `bookmark` cursor past the failure.
                        warn!(
                            node_id = %node_id,
                            leader_version = ev.commit_version,
                            error = %e,
                            "follower_loop: advance_replication_bookmark failed; \
                             will retry on next iteration (idempotent)"
                        );
                        break;
                    }
                }
                Ok(ApplyOutcome::Skipped) => {
                    // Idempotent re-delivery — already applied. No bookmark
                    // change needed (the event's commit_version is <= the
                    // current bookmark by construction).
                    debug!(
                        node_id = %node_id,
                        leader_version = ev.commit_version,
                        "follower_loop: event skipped (already applied)"
                    );
                }
                Err(e) => {
                    let re = ReplError::Apply {
                        leader_version: ev.commit_version,
                        source: e,
                    };
                    warn!(node_id = %node_id, error = %re, "follower_loop: apply failed, backing off");
                    sleep_backoff(&cancel, &mut backoff_ms).await;
                    // Break out of the event loop; the next iteration
                    // re-reads the bookmark (which did NOT advance past
                    // this event) and retries the same event idempotently.
                    break;
                }
            }
        }
    }
}

/// Exponential backoff sleep that is cancellation-aware. Doubles
/// `backoff_ms` on each call (capped at `MAX_BACKOFF_MS`); leaves it
/// untouched on cancellation (the loop is about to return anyway).
async fn sleep_backoff(cancel: &CancellationToken, backoff_ms: &mut u64) {
    let this = *backoff_ms;
    tokio::select! {
        _ = cancel.cancelled() => {}
        _ = tokio::time::sleep(std::time::Duration::from_millis(this)) => {
            *backoff_ms = (*backoff_ms * 2).min(MAX_BACKOFF_MS);
        }
    }
}
