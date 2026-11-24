use std::sync::atomic::{fence, AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_db::engine::meta::repl::bookmark_v3::{self, FollowerBookmarkState};
use shamir_db::{SeedRefusal, SubscriptionBinding};
use shamir_query_types::admin::ReplDirection;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::handle::SubHandle;
use super::machine::SubscriptionSupervisor;
use super::Subscription;
use crate::replication::error::ReplError;
use crate::replication::follower_loop::{
    note_source_created, run_follower_loop, FollowerLoopConfig,
};
use crate::replication::identity_fence;
use crate::replication::source;
use crate::replication::source::seed::terminal;
use source::seed::startup_lease::StartupLeaseGuard;

impl SubscriptionSupervisor {
    /// Start the follower loops for `sub`: one per destination recorded in
    /// the row's DURABLE binding (the enforced write boundary), after
    /// requiring the resolved profile's `pull` streams to cover every bound
    /// destination (keeps the admission/hello machinery honest).
    ///
    /// Returns `Ok(true)` when the loops were started and `Ok(false)` when
    /// the caller's snapshot turned out stale: nothing is marked and nothing
    /// is spawned. The reconcile pass that observes the new row owns any
    /// restart.
    pub(crate) async fn start_subscription(&self, sub: &Subscription) -> Result<bool, ReplError> {
        // Re-read under the control lock; a stale snapshot must not start or
        // mark anything. The new reconcile pass owns any restart.
        // Take the generation lease under the control lock; the start barrier
        // follows after the lock is released.
        let lease_guard = {
            let _control = self.local.lock_replication_control().await;
            let stale = match self.read_subscription_row(&sub.name).await {
                Ok(Some(row)) => {
                    let binding = SubscriptionBinding::from_row(&row)
                        .map_err(|e| ReplError::Start(e.to_string()))?;
                    row.get("state").and_then(|v| v.as_str()) != Some("active")
                        || binding.generation() != sub.binding_generation
                        || binding.destinations().to_vec() != sub.binding_destinations
                }
                Ok(None) => true,
                Err(e) => return Err(ReplError::Start(e)),
            };
            if stale {
                warn!(
                    subscription = %sub.name,
                    "supervisor: subscription row changed since the reconcile snapshot; start dropped"
                );
                return Ok(false);
            }
            // Round-5 A2 wiring: `None` = REFUSED — a valid lease for this
            // name exists somewhere (another supervisor's live follower).
            // Never kill it: defer; this supervisor's reconcile retries
            // later, and the refusal keeps exactly one live follower.
            let Some(lease) = self
                .local
                .take_subscription_lease(&sub.name, sub.binding_generation, &_control)
                .await
            else {
                debug!(
                    subscription = %sub.name,
                    "a valid lease for this subscription already exists; start deferred"
                );
                return Ok(false);
            };
            // Every start failure/cancellation invalidates the acquired lease.
            let lease_guard = StartupLeaseGuard::new(
                Arc::clone(&self.local),
                sub.name.clone(),
                Arc::clone(&lease),
            );
            // Round-4 MECHANISM 1: the start-time durability barrier — the
            // same catalogue flush CREATE/RESUME use. Only a successful
            // flush may start a follower; on failure the row stays
            // live-active and a later reconcile retries the barrier.
            if let Err(e) = self.local.flush_seed_catalogue().await {
                debug!(
                    subscription = %sub.name,
                    error = %e,
                    "supervisor: start barrier flush failed; start deferred (reconcile retries)"
                );
                // The unused lease must not linger valid (round-5 A2), or
                // the retry is refused forever by W1's valid-lease
                // semantics — the guard's Drop (on this return) cleans it.
                return Ok(false);
            }
            lease_guard
        };
        let lease = lease_guard.lease();
        // Interior window 1: the lease may have been invalidated between the
        // lease-take inside the block above and here (a concurrent lifecycle
        // mutation ran its quiesce before we re-check). Never start on an
        // invalid lease.
        if !lease.valid.load(Ordering::Acquire) {
            return Ok(false);
        }
        let streams = self
            .resolve_profile_streams(&sub.profile)
            .await
            .map_err(ReplError::Start)?;
        // The binding is the enforced boundary — targets come from the row,
        // NEVER from the profile. The profile is only checked for coverage.
        let profile_pulls: Vec<(String, String)> = streams
            .iter()
            .filter(|st| matches!(st.direction, ReplDirection::Pull))
            .filter_map(|st| {
                st.scope
                    .repo
                    .as_ref()
                    .map(|repo| (st.scope.db.clone(), repo.clone()))
            })
            .collect();
        for (db, repo) in &sub.binding_destinations {
            if !profile_pulls
                .iter()
                .any(|(p_db, p_repo)| p_db == db && p_repo == repo)
            {
                return Err(ReplError::Start(format!(
                    "profile '{}' does not cover bound destination '{}/{}'",
                    sub.profile, db, repo
                )));
            }
        }
        let pull_targets = sub.binding_destinations.clone();
        let generation = sub.binding_generation;
        for (index, (db, repo)) in pull_targets.iter().enumerate() {
            // Interior-window check: an invalidated lease marks nothing in
            // this iteration and never admits a destination.
            if !lease.valid.load(Ordering::Acquire) {
                return Ok(false);
            }
            source::seed::admission::admit(&self.local, &sub.name, &streams, db, repo).await?;
            let mut owner = None;
            self.registry.iter_sync(|name, handle| {
                if handle
                    .targets
                    .iter()
                    .any(|(target_db, target_repo)| target_db == db && target_repo == repo)
                {
                    owner = Some(name.clone());
                    return false;
                }
                true
            });
            if pull_targets[..index].contains(&(db.clone(), repo.clone())) {
                owner = Some(sub.name.clone());
            }
            if let Some(owner) = owner {
                let error = ReplError::DuplicateOwner {
                    db: db.clone(),
                    repo: repo.clone(),
                    owner,
                };
                // Fenced by the start's generation snapshot: a concurrently replaced
                // row turns the mark into a logged no-op inside `ShamirDb`.
                self.local
                    .mark_subscription_terminal_generation(
                        &sub.name,
                        "duplicate_owner",
                        &error.to_string(),
                        Some(sub.binding_generation),
                    )
                    .await
                    .map_err(|error| ReplError::Start(error.to_string()))?;
                return Err(error);
            }
            if let Some(destination) = self.local.get_db(db).and_then(|db| db.get_repo(repo)) {
                // Validate repo-wide batch support before probing or spawning.
                if let Err(storage_error) =
                    destination.validate_repo_wide_atomic_batch(&format!("{db}/{repo}"))
                {
                    let error = ReplError::AtomicBatchUnsupported {
                        db: db.clone(),
                        repo: repo.clone(),
                    };
                    self.local
                        .mark_subscription_terminal_generation(
                            &sub.name,
                            "resync_required",
                            &format!("{error}: {storage_error}"),
                            Some(sub.binding_generation),
                        )
                        .await
                        .map_err(|error| ReplError::Start(error.to_string()))?;
                    return Err(error);
                }
                // Gate-init failures map by KIND, not by string — see
                // `refuse_gate_init` below (typed refusal → terminal
                // `resync_required`; anything else → retryable `Start`).
                let gate = match destination.tx_gate().await {
                    Ok(gate) => gate,
                    Err(error) => {
                        return Err(self.refuse_gate_init(&sub.name, generation, error).await)
                    }
                };
                if gate.requires_reopen() {
                    let error = ReplError::ReopenRequired {
                        db: db.clone(),
                        repo: repo.clone(),
                        detail: "Resume refused until destination is reopened".into(),
                    };
                    // Fenced by the same generation snapshot as every mark above.
                    self.local
                        .mark_subscription_terminal_generation(
                            &sub.name,
                            "reopen_required",
                            &error.to_string(),
                            Some(sub.binding_generation),
                        )
                        .await
                        .map_err(|error| ReplError::Start(error.to_string()))?;
                    return Err(error);
                }
                let info = destination
                    .tx_info_store()
                    .await
                    .map_err(|error| ReplError::Start(error.to_string()))?;
                // PR6b-3: refuse resume of a repo whose durable follower
                // state this version does not support (legacy bookmark,
                // malformed record, floor-without-bookmark evidence). A
                // refused repo can never be resumed — the operator reseeds.
                match bookmark_v3::classify_follower_bookmark(&info).await {
                    Err(shamir_db::storage::error::DbError::FollowerStateRefused { reason }) => {
                        let error = ReplError::FollowerStateRefused { reason };
                        self.local
                            .mark_subscription_terminal_generation(
                                &sub.name,
                                "resync_required",
                                &error.to_string(),
                                Some(sub.binding_generation),
                            )
                            .await
                            .map_err(|error| ReplError::Start(error.to_string()))?;
                        return Err(error);
                    }
                    Err(e) => return Err(ReplError::Start(e.to_string())),
                    Ok(state) => {
                        // R2-W3: admission-time refusal of Resume for a
                        // corrupt or retired durable receipt pointer — the v3
                        // record decodes fine, so this is NOT the typed
                        // refusal above, but a resume would only re-trip at
                        // the pipeline with zero progress. Refuse BEFORE any
                        // loop is spawned.
                        if let FollowerBookmarkState::Native(v3) = &state {
                            let resolves = bookmark_v3::receipt_pointer_resolves(&info, v3)
                                .await
                                .map_err(|error| ReplError::Start(error.to_string()))?;
                            if !resolves {
                                let error = ReplError::ReopenRequired {
                                    db: db.clone(),
                                    repo: repo.clone(),
                                    detail:
                                        "durable follower record references a corrupt or retired receipt pointer; reopen required"
                                            .into(),
                                };
                                self.local
                                    .mark_subscription_terminal_generation(
                                        &sub.name,
                                        "reopen_required",
                                        &error.to_string(),
                                        Some(sub.binding_generation),
                                    )
                                    .await
                                    .map_err(|error| ReplError::Start(error.to_string()))?;
                                return Err(error);
                            }
                        }
                    }
                }
            }
        }
        // A superseded lease must not claim a source creation that will not
        // be invoked.
        if !lease.valid.load(Ordering::Acquire) {
            return Ok(false);
        }
        let is_reconnect = self
            .source_starts
            .insert_sync(sub.name.clone(), ())
            .is_err();
        for (db, repo) in &pull_targets {
            note_source_created(db, repo, is_reconnect);
        }
        let source = (self.factory)(sub);
        // T5 seam: awaited after admission and before spawn — the startup
        // interior window (round 4).
        #[cfg(test)]
        if let Some(hook) = &self.start_race_hook {
            hook().await;
        }
        // Interior window 2 (T5): re-check after the hook / between
        // admission and spawn; a superseded lease never spawns loops.
        if !lease.valid.load(Ordering::Acquire) {
            return Ok(false);
        }
        let cancel = CancellationToken::new();
        let mut joins = Vec::with_capacity(pull_targets.len());
        let mut dones: Vec<Arc<AtomicBool>> = Vec::with_capacity(pull_targets.len());
        // One live-count reference per target; only the final exit may
        // invalidate the shared lease after every apply guard is gone.
        let live_targets = Arc::new(AtomicUsize::new(pull_targets.len()));
        // Last exit tears down the shared lease.
        lease_guard.defuse();

        for (db, repo) in pull_targets.iter().cloned() {
            let mut cfg = FollowerLoopConfig::new(self.node_id.clone(), db, repo)
                .with_poll_wait_ms(self.poll_wait_ms)
                .with_validate_replicated_payload(self.validate_replicated_payload)
                // PR6b-3 stall visibility: let the loop persist a
                // status_message on this row after repeated no-progress
                // failures (the loop swallows transient errors internally,
                // so the supervisor can never observe them via the loop's
                // return value).
                .with_subscription(sub.name.clone());
            if let Some(n) = self.max_iterations {
                cfg = cfg.with_max_iterations(n);
            }
            // Bind the loop to this subscription's lease.
            cfg.subscription = sub.name.clone();
            cfg.lease = Some(lease.clone());
            #[cfg(test)]
            {
                cfg.apply_park = self.apply_park.clone();
                cfg.exit_hook = self.exit_hook.clone();
            }

            let local = self.local.clone();
            let src = source.clone();
            let tok = cancel.clone();
            let sub_name = sub.name.clone();
            // Generation fences stale terminal callbacks from replacements.
            let generation = sub.binding_generation;
            // Per-loop done state shared with the handle and exit path.
            let done_flag = Arc::new(AtomicBool::new(false));
            let done = Arc::clone(&done_flag);
            let lease_for_exit = Arc::clone(&lease);
            let live = Arc::clone(&live_targets);
            #[cfg(test)]
            let teardown_count = Arc::clone(&self.teardown_count);
            #[cfg(test)]
            let teardown_done = Arc::clone(&self.teardown_done);
            #[cfg(test)]
            let exit_signal = cfg.exit_hook.clone();
            #[cfg(test)]
            let signal_other_exit = exit_signal
                .as_ref()
                .is_some_and(|hook| hook.repo.as_deref() != Some(cfg.repo.as_str()));

            let registry = self.registry.clone();
            // §5.6 — each loop is its own task; never blocks the server.
            let join = tokio::spawn(async move {
                // Retry terminal marking after the loop returns.
                let tok_exit = tok.clone();
                let loop_result = run_follower_loop(local.clone(), src, cfg, tok).await;
                #[cfg(test)]
                if signal_other_exit {
                    if let Some(hook) = &exit_signal {
                        hook.exited.notify_one();
                    }
                }
                match loop_result {
                    Ok(()) => {}
                    Err(e @ ReplError::JournalGap { gap_at, .. })
                    | Err(
                        e @ ReplError::ApplyRejected {
                            leader_version: gap_at,
                            ..
                        },
                    ) => {
                        let from_version = if let ReplError::JournalGap { from_version, .. } = &e {
                            *from_version
                        } else {
                            gap_at
                        };
                        let (local_m, name_m) = (Arc::clone(&local), sub_name.clone());
                        terminal::mark_with_retry(
                            &local,
                            &sub_name,
                            "resync_required",
                            &tok_exit,
                            generation,
                            || {
                                let l = Arc::clone(&local_m);
                                let n = name_m.clone();
                                async move {
                                    l.mark_subscription_resync_required_generation(
                                        &n,
                                        gap_at,
                                        from_version,
                                        generation,
                                    )
                                    .await
                                }
                            },
                        )
                        .await;
                        warn!(
                            subscription = %sub_name,
                            error = %e,
                            "follower loop terminated; subscription marked resync_required"
                        );
                    }
                    Err(
                        e @ (ReplError::IdentityMismatch { .. }
                        | ReplError::Seed(
                            SeedRefusal::WrongIdentity
                            | SeedRefusal::WrongHistoryEpoch
                            | SeedRefusal::MissingIdentity
                            | SeedRefusal::WrongSchema,
                        )),
                    ) => {
                        // S5c-X PR8 chunk 4 — SERVER FENCES: an identity/epoch
                        // fence trip means the leader is not the incarnation
                        // this subscription recorded. QUARANTINE the follower
                        // (state = "resync_required") so the split is visible
                        // and the loop stays stopped, exactly like the
                        // journal-gap path. The fence fires BEFORE decode/
                        // apply, so nothing was applied and the gap metadata
                        // is 0/0 (no version was reached).
                        let skip_quarantine = cfg!(test)
                            && identity_fence::checks::SKIP_FENCE_QUARANTINE_FOR_TEST
                                .load(Ordering::Relaxed);
                        if skip_quarantine {
                            warn!(
                                subscription = %sub_name,
                                error = %e,
                                "identity fence tripped; quarantine disabled by test seam"
                            );
                            // Negative control: the UNPROTECTED pre-quarantine
                            // path (generic terminal mapping), still retried.
                            terminal::persist_terminal_with_retry(
                                &local, &sub_name, &e, generation, &tok_exit,
                            )
                            .await;
                        } else {
                            // Same catalogue state the gap path persists via
                            // `mark_subscription_resync_required_generation`
                            // (state = "resync_required"), but the message
                            // keeps the TYPED fence reason plus the recorded
                            // and observed identities instead of gap text —
                            // there is no gap; nothing was applied.
                            let (local_m, name_m, msg_m) =
                                (Arc::clone(&local), sub_name.clone(), e.to_string());
                            terminal::mark_with_retry(
                                &local,
                                &sub_name,
                                "resync_required (identity fence)",
                                &tok_exit,
                                generation,
                                || {
                                    let l = Arc::clone(&local_m);
                                    let n = name_m.clone();
                                    let msg = msg_m.clone();
                                    async move {
                                        l.mark_subscription_terminal_generation(
                                            &n,
                                            "resync_required",
                                            &msg,
                                            Some(generation),
                                        )
                                        .await
                                    }
                                },
                            )
                            .await;
                            metrics::counter!("shamir_repl_resync_required_total").increment(1);
                            warn!(
                                subscription = %sub_name,
                                error = %e,
                                "identity fence tripped; subscription marked resync_required"
                            );
                        }
                    }
                    Err(e @ ReplError::FollowerStateRefused { .. }) => {
                        // S5c-X PR6b-3 — TERMINAL: the follower repo's durable
                        // state is unsupported (no migration exists). Same
                        // quarantine family as the identity fence, WITHOUT
                        // the identity-fence test seam: always quarantined.
                        // Round 12 F3: loop-originated -> generation-fenced.
                        if let Err(mark_err) = local
                            .mark_subscription_terminal_generation(
                                &sub_name,
                                "resync_required",
                                &e.to_string(),
                                Some(generation),
                            )
                            .await
                        {
                            warn!(
                                subscription = %sub_name,
                                error = %mark_err,
                                "supervisor: failed to persist resync_required after follower-state refusal"
                            );
                        }
                        metrics::counter!("shamir_repl_resync_required_total").increment(1);
                        warn!(
                            subscription = %sub_name,
                            error = %e,
                            "follower state refused; subscription marked resync_required (reseed required)"
                        );
                    }
                    Err(e @ ReplError::AtomicBatchUnsupported { .. }) => {
                        // S5c-X PR6b-3 R7-2(ii) — TERMINAL: the destination
                        // backend cannot host the receipt pipeline (no atomic
                        // batches). Same quarantine family as the
                        // follower-state refusal: persist the typed terminal
                        // status ONCE with the remedy in the message; the loop
                        // never retries it.
                        // Round 12 F3: loop-originated -> generation-fenced.
                        if let Err(mark_err) = local
                            .mark_subscription_terminal_generation(
                                &sub_name,
                                "resync_required",
                                &e.to_string(),
                                Some(generation),
                            )
                            .await
                        {
                            warn!(
                                subscription = %sub_name,
                                error = %mark_err,
                                "supervisor: failed to persist resync_required after atomic-batch refusal"
                            );
                        }
                        metrics::counter!("shamir_repl_resync_required_total").increment(1);
                        warn!(
                            subscription = %sub_name,
                            error = %e,
                            "atomic batches unsupported on destination; subscription marked resync_required (re-create the repo raw)"
                        );
                    }
                    Err(e @ ReplError::ReopenRequired { .. }) => {
                        // Generation-aware: a replaced binding outlives the
                        // stale task's reopen mark. Round-5 B3: bounded,
                        // cancellation-aware retry.
                        let (local_m, name_m, msg_m) =
                            (Arc::clone(&local), sub_name.clone(), e.to_string());
                        terminal::mark_with_retry(
                            &local,
                            &sub_name,
                            "reopen_required",
                            &tok_exit,
                            generation,
                            || {
                                let l = Arc::clone(&local_m);
                                let n = name_m.clone();
                                let msg = msg_m.clone();
                                async move {
                                    l.mark_subscription_terminal_generation(
                                        &n,
                                        "reopen_required",
                                        &msg,
                                        Some(generation),
                                    )
                                    .await
                                }
                            },
                        )
                        .await;
                    }
                    Err(e) => {
                        terminal::persist_terminal_with_retry(
                            &local, &sub_name, &e, generation, &tok_exit,
                        )
                        .await;
                        warn!(subscription = %sub_name, error = %e, "follower loop terminated");
                    }
                }
                // ---- exit housekeeping: shared by every loop return path.
                // The apply guard is dropped before this point; decrement the
                // shared count, and let only the last loop invalidate/remove.
                done.store(true, Ordering::Release);
                if live.fetch_sub(1, Ordering::AcqRel) == 1 {
                    let control = local.lock_replication_control().await;
                    local.invalidate_subscription_lease(&sub_name, &lease_for_exit, &control);
                    drop(control);
                    let lease_check = Arc::clone(&lease_for_exit);
                    let _ =
                        registry.remove_if_sync(&sub_name, |h| Arc::ptr_eq(&h.lease, &lease_check));
                    #[cfg(test)]
                    {
                        teardown_count.fetch_add(1, Ordering::AcqRel);
                        teardown_done.notify_one();
                    }
                }
            });
            joins.push(join);
            dones.push(done_flag);
        }

        let handle = SubHandle {
            cancel,
            joins,
            dones,
            lease,
            profile: sub.profile.clone(),
            generation: sub.binding_generation,
            targets: pull_targets,
        };
        // A racing reconcile may have inserted already; if so, stop the loops
        // we just spawned rather than leak them.
        let fence_cancel = handle.cancel.clone();
        if let Err((_, handle)) = self.registry.insert_sync(sub.name.clone(), handle) {
            handle.stop();
        } else {
            info!(subscription = %sub.name, profile = %sub.profile, "supervisor: started follower loop");
        }
        // Pairs with the fence in `begin_stop`: a handle registered after its
        // registry walk sees the stop flag here and is cancelled at once.
        fence(Ordering::SeqCst);
        if self.stopping.load(Ordering::SeqCst) {
            fence_cancel.cancel();
        }
        Ok(true)
    }
}
