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
//!      - verify/adopt upstream identity FIRST, before leader-epoch fencing;
//!      - **epoch-fence (§5.2):** if the reply's epoch is strictly lower
//!        than the max seen, terminate with [`ReplError::StaleLeaderEpoch`];
//!        otherwise record the new max;
//!      - **gap-stop (§5.3 / R1 simplification):** if the reply carries
//!        `gap_at: Some(g)` with `g > from_version`, the follower is
//!        permanently missing `[from_version, g)` — log at `warn!`/`error!`
//!        and terminate with [`ReplError::JournalGap`] WITHOUT applying any
//!        events in that same reply (the supervisor marks the subscription
//!        `resync_required`; full automated snapshot reseed is R2);
//!      - decode the events payload (`Vec<ReplicatedEvent>`);
//!      - apply the decoded batch through `apply_replicated_batch`: reserve
//!        a flushed local-version upper bound before any history write, then
//!        flush history once before the covering bookmark set+flush;
//!      - no cursor advancement or acknowledgement precedes bookmark flush;
//!      - if no events were returned, the leader already long-polled
//!        `wait_ms`, so a small backoff is optional but harmless.
//!
//! **§5.6 invariant:** this is a background task. It holds NO follower locks
//! and never blocks the follower's own commits. Transient transport errors
//! are logged + retried with backoff. Epoch/gap/rejection and reopen-required
//! errors terminate the loop; a dead destination gate is never retried.

use std::sync::Arc;

use shamir_db::engine::{
    tx::ReplicatedEvent,
    tx::{ApplyOutcome, ApplyPolicy},
};
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
    pub validate_replicated_payload: bool,
    /// S5c-X PR7: the universal leader-identity fence on every Hello/Pull.
    /// `true` in production; tests may disable it through this explicit seam
    /// to exercise the unprotected path (negative control).
    #[cfg(test)]
    pub(crate) identity_fence: bool,
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
            validate_replicated_payload: false,
            #[cfg(test)]
            identity_fence: true,
            max_iterations: None,
        }
    }

    /// Override the long-poll budget (ms).
    #[must_use]
    pub fn with_poll_wait_ms(mut self, ms: u32) -> Self {
        self.poll_wait_ms = ms;
        self
    }

    #[must_use]
    pub fn with_validate_replicated_payload(mut self, validate: bool) -> Self {
        self.validate_replicated_payload = validate;
        self
    }

    /// Disable the universal identity fence (negative-control tests only).
    #[must_use]
    #[cfg(test)]
    pub(crate) fn with_identity_fence(mut self, enabled: bool) -> Self {
        self.identity_fence = enabled;
        self
    }

    /// Set the iteration cap (mainly for tests).
    #[must_use]
    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = Some(n);
        self
    }
}

/// Overrides only identity persistence for full-loop durability regressions.
#[cfg(test)]
pub(crate) struct IdentityStoreOverride {
    pub(crate) info: Arc<dyn shamir_db::storage::types::Store>,
    pub(crate) startup_reflush: bool,
}

#[cfg(test)]
impl std::fmt::Debug for IdentityStoreOverride {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentityStoreOverride")
            .field("startup_reflush", &self.startup_reflush)
            .finish_non_exhaustive()
    }
}

/// Run the complete production loop body with a test-only identity store.
#[cfg(test)]
pub(crate) async fn run_follower_loop_with_identity_store(
    db: Arc<ShamirDb>,
    source: Arc<dyn ReplSource>,
    cfg: FollowerLoopConfig,
    cancel: CancellationToken,
    identity_store: IdentityStoreOverride,
) -> Result<(), ReplError> {
    run_follower_loop_internal(db, source, cfg, cancel, Some(identity_store)).await
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
    run_follower_loop_internal(
        db,
        source,
        cfg,
        cancel,
        #[cfg(test)]
        None,
    )
    .await
}

async fn run_follower_loop_internal(
    db: Arc<ShamirDb>,
    source: Arc<dyn ReplSource>,
    cfg: FollowerLoopConfig,
    cancel: CancellationToken,
    #[cfg(test)] identity_store: Option<IdentityStoreOverride>,
) -> Result<(), ReplError> {
    let FollowerLoopConfig {
        node_id,
        db: db_name,
        repo,
        poll_wait_ms,
        validate_replicated_payload,
        #[cfg(test)]
        identity_fence,
        max_iterations,
    } = cfg;
    #[cfg(not(test))]
    let identity_fence = true;
    let apply_policy = if validate_replicated_payload {
        ApplyPolicy::ValidatePayload
    } else {
        ApplyPolicy::Trusted
    };
    // 1. hello → seed max-seen epoch (§5.2).
    let seed_receipt = super::source::seed::fence::load_receipt(&db, &db_name, &repo).await?;
    #[cfg(test)]
    let startup_reflush = identity_store
        .as_ref()
        .is_none_or(|store| store.startup_reflush);
    #[cfg(not(test))]
    let startup_reflush = true;
    if identity_fence && startup_reflush {
        super::identity_fence::checks::prepare_subscription(
            &db,
            &db_name,
            &repo,
            #[cfg(test)]
            identity_store.as_ref().map(|store| &store.info),
        )
        .await?;
    }
    let mut max_seen_epoch = match source.hello(&node_id).await {
        Ok(resp) => {
            // S5c-X PR7: universal identity fence — verify the recorded
            // (lineage, history_epoch) on EVERY Hello, before any pull.
            if identity_fence {
                #[cfg(test)]
                let override_info = identity_store.as_ref().map(|store| &store.info);
                #[cfg(not(test))]
                let override_info: Option<
                    &Arc<dyn shamir_db::storage::types::Store>,
                > = None;
                if let Some(info) = override_info {
                    super::identity_fence::checks::fence_hello_identity(
                        info,
                        &db_name,
                        &repo,
                        &resp,
                        seed_receipt.as_ref(),
                    )
                    .await?;
                } else {
                    super::identity_fence::checks::fence_hello_recorded(
                        &db,
                        &db_name,
                        &repo,
                        &resp,
                        seed_receipt.as_ref(),
                    )
                    .await?;
                }
            } else {
                super::source::seed::fence::fence_hello(seed_receipt.as_ref(), &resp)?;
            }
            super::source::leader_epoch_of(&resp)
        }
        // A hello that already trips fencing is fatal — propagate.
        Err(e @ ReplError::StaleLeaderEpoch { .. })
        | Err(e @ ReplError::Seed(_))
        | Err(e @ ReplError::IdentityMismatch { .. }) => return Err(e),
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
        let gate = repo_instance
            .tx_gate()
            .await
            .map_err(|error| ReplError::Bookmark(error.to_string()))?;
        if gate.requires_reopen() {
            return Err(ReplError::ReopenRequired {
                db: db_name.clone(),
                repo: repo.clone(),
                detail: "destination gate requires reopen".into(),
            });
        }
        let bookmark = match repo_instance.replication_bookmark().await {
            Ok(v) => v,
            Err(e) => {
                // PR6b-3: a refused durable follower state is TERMINAL — no
                // migration exists, the operator reseeds. Never retry it.
                if let shamir_db::storage::error::DbError::FollowerStateRefused { reason } = &e {
                    return Err(ReplError::FollowerStateRefused { reason: *reason });
                }
                let re = ReplError::Bookmark(e.to_string());
                warn!(node_id = %node_id, error = %re, "follower_loop: bookmark read failed");
                sleep_backoff(&cancel, &mut backoff_ms).await;
                continue;
            }
        };

        // 3. pull — always from the durable bookmark; a journal gap is now
        // terminal (see step 6), so there is no cursor-shift state to track
        // across iterations.
        let from_version = bookmark
            .checked_add(1)
            .ok_or(shamir_db::SeedRefusal::BookmarkOverflow)?;
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
            Err(e @ ReplError::StaleLeaderEpoch { .. })
            | Err(e @ ReplError::ReopenRequired { .. })
            | Err(e @ ReplError::IdentityMismatch { .. })
            | Err(e @ ReplError::Seed(_)) => return Err(e),
            Err(e) => {
                warn!(node_id = %node_id, error = %e, "follower_loop: pull failed, backing off");
                sleep_backoff(&cancel, &mut backoff_ms).await;
                continue;
            }
        };

        // S5c-X PR7: universal identity fence — verify (or adopt at first
        // contact) on EVERY Pull, BEFORE any event of this response is
        // applied. Terminal on mismatch: nothing below runs.
        if identity_fence {
            #[cfg(test)]
            let info = match identity_store.as_ref() {
                Some(store) => store.info.clone(),
                None => repo_instance
                    .tx_info_store()
                    .await
                    .map_err(|error| ReplError::Bookmark(error.to_string()))?,
            };
            #[cfg(not(test))]
            let info = repo_instance
                .tx_info_store()
                .await
                .map_err(|error| ReplError::Bookmark(error.to_string()))?;
            super::identity_fence::checks::fence_pull_identity(&info, &resp, seed_receipt.as_ref())
                .await?;
        } else {
            super::source::seed::fence::fence_pull(seed_receipt.as_ref(), &resp)?;
        }

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
                // S5c-X 2c: the leader refuses to serve a completed_reseed
                // repo to a legacy follower. New enough to understand the
                // refusal → map it to the typed, terminal identity refusal
                // (the supervisor's quarantine arm marks `resync_required`).
                // NOTE the asymmetry: a truly OLD follower that cannot parse
                // this semantic keeps retrying with backoff below — but the
                // leader still refuses to serve it every time (fail closed at
                // the leader); the typed quarantine only applies to followers
                // that understand the refusal code.
                if code == "legacy_follower_refused" {
                    return Err(ReplError::Seed(shamir_db::SeedRefusal::MissingIdentity));
                }
                // Any other structured error (e.g. denied_repo,
                // unknown_repo): treat as transient — log + backoff.
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
        // is permanently missing `[from_version, g]` (`g == from_version` is the
        // first requested version itself) — this is now a
        // terminal condition, NOT a silent skip. Check and return BEFORE
        // decoding/applying any events that may have accompanied this same
        // reply (the events, if any, must not be applied).
        if let Some(g) = gap_at {
            if g >= from_version {
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
            // `g < from_version` — the gap is behind us, ignore and proceed.
        }

        // 7. decode + apply each event in order.
        let events: Vec<ReplicatedEvent> = match rmp_serde::from_slice(&events_bytes) {
            Ok(v) => v,
            Err(e) => {
                let re = ReplError::Decode(e.to_string());
                warn!(node_id = %node_id, error = %re, "follower_loop: decode failed, backing off");
                sleep_backoff(&cancel, &mut backoff_ms).await;
                continue;
            }
        };

        if events.is_empty() {
            if max_iterations.is_some_and(|cap| iter >= cap) {
                return Ok(());
            }
            // The leader already long-polled `poll_wait_ms`; a tiny extra
            // backoff keeps us polite but is not required.
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                _ = tokio::time::sleep(std::time::Duration::from_millis(IDLE_BACKOFF_MS)) => {}
            }
            continue;
        }

        // PR6b-3: the receipt pipeline is the ONLY apply path; admission
        // refuses unsupported durable follower states with a typed error.
        match shamir_db::engine::tx::apply_replicated::apply_replicated_batch(
            &repo_instance,
            &events,
            bookmark,
            apply_policy,
        )
        .await
        {
            Ok(ApplyOutcome::Applied { .. } | ApplyOutcome::Skipped) => {}
            Ok(ApplyOutcome::Rejected { reason }) => {
                return Err(ReplError::ApplyRejected {
                    leader_version: events[0].event.commit_version,
                    reason,
                });
            }
            Err(e) => {
                // PR6b-3: a refused durable follower state is TERMINAL —
                // check BEFORE the requires_reopen probe and never back off.
                if let shamir_db::storage::error::DbError::FollowerStateRefused { reason } = &e {
                    return Err(ReplError::FollowerStateRefused { reason: *reason });
                }
                // PR6b-3 (W5): the receipt pipeline's `RepoRequiresReopen`
                // error kind IS the producer's explicit terminal
                // classification (corrupt receipt pointer, vanished
                // bookmark, double-Uncertain). The engine deliberately keeps
                // the RAM gate flag clean on these failures — the parked
                // guards keep the repo serving — so the loop must trust the
                // error KIND, not only the flag: retrying it in-process
                // would contradict its meaning and the design's
                // machine-readable-terminal requirement.
                if let shamir_db::storage::error::DbError::RepoRequiresReopen(detail) = &e {
                    return Err(ReplError::ReopenRequired {
                        db: db_name.clone(),
                        repo: repo.clone(),
                        detail: detail.clone(),
                    });
                }
                let gate = repo_instance
                    .tx_gate()
                    .await
                    .map_err(|error| ReplError::Bookmark(error.to_string()))?;
                if gate.requires_reopen() {
                    return Err(ReplError::ReopenRequired {
                        db: db_name.clone(),
                        repo: repo.clone(),
                        detail: e.to_string(),
                    });
                }
                warn!(error = %e, "follower_loop: batch apply failed, backing off");
                sleep_backoff(&cancel, &mut backoff_ms).await;
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
