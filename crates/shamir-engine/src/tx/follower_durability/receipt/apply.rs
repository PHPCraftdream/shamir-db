//! Orchestration (S5c-X PR6b-2): route one pull batch through the receipt
//! pipeline — resume an outstanding receipt if any, otherwise chunk the
//! prepared events and run Prepared → Materialized → adopt-counters →
//! checkpoint (bookmark commit + RAM publication in one poll) per chunk.
//!
//! Cancellation: the pipeline runs on the REPOSITORY-OWNED lifecycle worker
//! (`resume::worker`, ONE lifecycle spawn per repo — the merged
//! `ForegroundLifecycle` foundation). The caller submits a bounded job and
//! awaits only the oneshot reply; dropping the caller future drops that
//! await only — the worker retains the job (with its own repo clone) and
//! runs it to completion or to a durable pause point. Bounded: `submit` is
//! `try_send` into the lifecycle's capacity-bounded queue; the write-domain
//! permit inside the pipeline serializes applies (one at a time). Errors
//! while fresh chunk guards are held PARK those guards in the lease
//! (finding 2); retries re-load the durable receipt so the recorded
//! counters are never recomputed from the live cache after Materialized
//! committed (finding 3).

use std::sync::Arc;

use shamir_collections::TFxMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;
use shamir_tx::RepoTxGate;

use super::super::super::{prepare_event, ApplyOutcome, ApplyPolicy, PreparedEvent};
#[cfg(test)]
use super::super::test_seam::{self, Phase};
use super::resume::worker::ReceiptLease;
use super::{checkpoint, publish, resume, stage};
use crate::meta::repl::bookmark_v3::{load_bookmark_v3, BookmarkV3};
use crate::meta::repl::{ReceiptBinding, ReceiptPhase, ReplicationReceipt, MAX_RECEIPT_EVENTS};
use crate::meta::repl_bookmark::{load_fence_record, load_replication_bookmark};
use crate::repo::changelog_store::write_domain::RepoWritePermit;
use crate::repo::RepoInstance;
use crate::tx::ReplicatedEvent;

#[cfg_attr(not(test), allow(dead_code))] // entry: wired by the activation chunk (next stage)
pub(crate) async fn apply_batch_receipt(
    repo: &RepoInstance,
    info: Arc<dyn Store>,
    events: Vec<ReplicatedEvent>,
    bookmark: u64,
    policy: ApplyPolicy,
) -> DbResult<ApplyOutcome> {
    // Repository-owned bounded lifecycle worker (review finding 4, round 2):
    // the entry lazily installs the worker on the FIRST submission, then
    // submits and awaits only the oneshot reply — a cancelled caller drops
    // the receiver, and the worker KEEPS RUNNING the job with its own repo
    // clone. No orphan `tokio::spawn` outside the lifecycle's
    // `install_worker`.
    repo.submit_receipt_job(info, events, bookmark, policy)
        .await
}

pub(crate) async fn run_receipt_pipeline(
    repo: &RepoInstance,
    info: Arc<dyn Store>,
    events: Vec<ReplicatedEvent>,
    bookmark: u64,
    policy: ApplyPolicy,
    lease: &ReceiptLease,
) -> DbResult<ApplyOutcome> {
    let gate = repo.tx_gate().await?;
    if gate.requires_reopen() {
        return Err(DbError::RepoRequiresReopen(repo.name().to_string()));
    }
    // Repository write-admission domain for the receipt path (PR6b-2c): the
    // DOMAIN permit excludes DDL and every other repository mutation for the
    // pipeline's whole run — there is deliberately NO second admission
    // mechanism here. Lock order (per #183): the permit FIRST, the marker
    // write mutex second. When the lease carries a paused reservation (the
    // previous failure parked one), admission goes through
    // `admit_with_reservation`; the reservation is taken OUT of the lease's
    // mutex BEFORE the await and restored on failure, so no std-sync guard
    // is ever held across `.await` and the future stays `Send`.
    let permit = match lease.take_reservation() {
        Some(reservation) => {
            match repo
                .write_domain()
                .admit_with_reservation(&reservation)
                .await
            {
                Ok(permit) => permit,
                Err(error) => {
                    // Restore the pause so the NEXT pull can still resume
                    // through the reservation path.
                    lease.restore_reservation(reservation);
                    return Err(admission_error(repo, error));
                }
            }
        }
        None => match repo.write_domain().admit().await {
            Ok(permit) => permit,
            Err(error) => return Err(admission_error(repo, error)),
        },
    };
    let _marker = repo.marker_write_mutex().lock().await;
    if let Some(outstanding) = resume::lookup(repo, &info).await? {
        // Full identity check (finding 1c) BEFORE any resume work and
        // before loading bookmark v3: fence lineage/epoch, offered
        // coverage, bindings, and byte-identical re-translation.
        resume::identity::verify_identity(repo, &info, &outstanding.receipt, &events).await?;
        let tail = resume::tail_events(&outstanding.receipt, &events);
        let v3 = load_bookmark_v3(&info).await?.ok_or_else(|| {
            DbError::RepoRequiresReopen("replication bookmark vanished during resume".into())
        })?;
        let old = BookmarkV3 {
            fence: v3.fence,
            bookmark: outstanding.receipt.predecessor_bookmark,
            clean_through: v3.clean_through,
            outstanding_receipt: None,
        };
        // Converge both resume phases on the SAME publication/durability row
        // building — but NOT on re-commit (reviewer P1 #6): the Materialized
        // phase is ALREADY durable — its rows, counters and receipt bytes
        // were committed by the phase that wrote it (mirroring the inline
        // fast path in `run_chunk_tail`), so re-running
        // `commit_materialized_work` would be a pure rewrite of durable
        // state. Control flow:
        //   Prepared    -> materialize (compute counters vs live pre-state,
        //                  nothing published yet) -> commit Materialized
        //                  batch -> publication -> durable_tail
        //   Materialized-> publication built STRAIGHT from the STORED
        //                  receipt (decoded downstream events + recorded
        //                  absolute counters) -> durable_tail
        let (receipt, publication) = match outstanding.receipt.phase {
            ReceiptPhase::Prepared => {
                let work =
                    stage::materialize_from_prepared_receipt(repo, &outstanding.receipt).await?;
                let (receipt, _counters) =
                    stage::commit_materialized_work(repo, &permit, &info, &work, &old).await?;
                let publication = publish::publication_from_receipt(repo, &receipt).await?;
                (receipt, publication)
            }
            ReceiptPhase::Materialized => {
                let publication =
                    publish::publication_from_receipt(repo, &outstanding.receipt).await?;
                (outstanding.receipt.clone(), publication)
            }
            ReceiptPhase::Checkpointed => {
                return Err(DbError::RepoRequiresReopen(
                    "bookmark references a Checkpointed receipt".into(),
                ));
            }
        };
        let floor = publication.floor;
        let new_bookmark =
            durable_tail(repo, &permit, &info, &gate, &receipt, publication, &old).await?;
        // The receipt retired cleanly: clear the paused reservation (its
        // RAII drop releases the domain slot, so ordinary `admit()` works
        // again).
        lease.clear_reservation();
        // Publication succeeded: parked guards' versions are ≤ the watermark
        // now — retire (drop) them, their Aborted marks are no-ops.
        lease.retire_parked();
        if !tail.is_empty() {
            let tail: Vec<ReplicatedEvent> = tail.into_iter().cloned().collect();
            apply_fresh(
                repo,
                &permit,
                &info,
                &gate,
                &tail,
                new_bookmark,
                policy,
                lease,
            )
            .await?;
        }
        return Ok(ApplyOutcome::Applied {
            local_version: floor,
        });
    }
    apply_fresh(
        repo, &permit, &info, &gate, &events, bookmark, policy, lease,
    )
    .await
}

/// Map a write-domain admission rejection to the repo's `Internal` error
/// style (see repo_instance.rs's mapping).
fn admission_error(
    repo: &RepoInstance,
    error: crate::repo::changelog_store::write_domain::RepoWriteError,
) -> DbError {
    DbError::Internal(format!(
        "receipt pipeline admission rejected for repo {}: {error}",
        repo.name()
    ))
}

/// The fresh path: translate the whole batch (rejections stay pre-write, like
/// the legacy batch path), then chunk the prepared events greedily under the
/// receipt's combined structural bounds and run the pipeline per chunk.
#[allow(clippy::too_many_arguments)]
async fn apply_fresh(
    repo: &RepoInstance,
    permit: &RepoWritePermit,
    info: &Arc<dyn Store>,
    gate: &Arc<RepoTxGate>,
    events: &[ReplicatedEvent],
    bookmark: u64,
    policy: ApplyPolicy,
    lease: &ReceiptLease,
) -> DbResult<ApplyOutcome> {
    // Design step 1: the DURABLE bookmark is consulted BEFORE any version
    // allocation, so a replayed pull carrying a stale caller watermark can
    // never allocate replacement versions over versions the durable state
    // already owns. (The resume path already consults the receipt before
    // anything else.) The caller-supplied `bookmark` is kept for
    // compatibility, but the durable floor always applies.
    let durable_bookmark = load_replication_bookmark(info).await?;
    let watermark = bookmark.max(durable_bookmark);
    let mut prepared: Vec<PreparedEvent> = Vec::with_capacity(events.len());
    let mut origins: Vec<usize> = Vec::new();
    for (index, event) in events.iter().enumerate() {
        match prepare_event(repo, &event.event, &event.field_bindings, watermark, policy).await? {
            Ok(event) => {
                prepared.push(event);
                origins.push(index);
            }
            Err(ApplyOutcome::Skipped) => {}
            Err(rejected) => return Ok(rejected),
        }
    }
    if prepared.is_empty() {
        return Ok(ApplyOutcome::Skipped);
    }
    // Preparation awaited I/O; re-check admission before allocation.
    if gate.requires_reopen() {
        return Err(DbError::RepoRequiresReopen(repo.name().to_string()));
    }
    let mut guards = Vec::with_capacity(prepared.len());
    for event in &mut prepared {
        let guard = gate.assign_next_version_guarded();
        event.version = Some(guard.version());
        guards.push(guard);
    }
    let floor = gate.version_allocation_high_water_mark();
    let mut old = match load_bookmark_v3(info).await? {
        Some(bookmark) => BookmarkV3 {
            outstanding_receipt: None,
            ..bookmark
        },
        // Activation guarantees this repo is v3-or-absent, so `None` is a
        // FRESH INSTALL: synthesize the v3 view from the legacy records
        // (safe — absent v3 means these are the only stored values).
        None => BookmarkV3 {
            fence: load_fence_record(info).await?,
            bookmark: load_replication_bookmark(info).await?,
            clean_through: None,
            outstanding_receipt: None,
        },
    };

    let mut start = 0usize;
    while start < prepared.len() {
        // Greedy chunking under the COMBINED structural bound: each event
        // costs 3 (mapping + downstream event + timestamp) plus TWICE its
        // op count — the Materialized receipt adds one counter entry per
        // distinct table (≤ ops), and the bound is checked again by
        // `encode_receipt` at that phase.
        let mut combined = 0usize;
        let mut end = start;
        while end < prepared.len() {
            let cost = 3 + 2 * prepared[end]
                .tables
                .iter()
                .map(|table| table.ops.len())
                .sum::<usize>();
            if combined + cost > MAX_RECEIPT_EVENTS && end > start {
                break;
            }
            combined += cost;
            end += 1;
        }
        // Guard ownership across errors (finding 2): from allocation until
        // publication the worker OWNS these guards — on ANY failure they are
        // PARKED in the lease (never dropped), because a dropped guard
        // marks Aborted and advances the watermarks past still-unpublished
        // versions while the repo keeps serving. Storage failures retry the
        // idempotent pipeline inline (the durable receipt is the redo
        // authority); the 3rd failure or any NON-retryable error parks and
        // returns. Parked guards retire on the next successful publication,
        // or at worker shutdown under quarantine (requires_reopen).
        let mut attempt = 0usize;
        let (new_bookmark, chunk_local_end) = loop {
            match try_chunk(
                repo,
                permit,
                info,
                gate,
                events,
                &origins[start..end],
                &prepared[start..end],
                &old,
            )
            .await
            {
                Ok(done) => break done,
                Err(error) => {
                    attempt += 1;
                    if !matches!(error, DbError::Storage(_)) || attempt >= 3 {
                        let parked: Vec<_> = guards.drain(0..end - start).collect();
                        lease.park(parked);
                        // Paused reservation (PR6b-2c): only when a receipt
                        // is OUTSTANDING — the parked guards alone do not
                        // justify rejecting ordinary admission repo-wide. A
                        // failure with NO outstanding receipt (before the
                        // Prepared commit) places NO reservation.
                        if matches!(resume::lookup(repo, info).await, Ok(Some(_))) {
                            if let Some(reservation) = repo.write_domain().try_place_reservation() {
                                lease.place_pause_reservation(reservation);
                            }
                        }
                        return Err(error);
                    }
                    log::warn!(
                        "follower receipt chunk failed (attempt {attempt}/3), \
                         retrying: {error}"
                    );
                }
            }
        };
        // The chunk published (apply_publication ran `mark_durable` inside
        // the checkpoint): any guards parked by EARLIER failed chunks are now
        // ≤ the watermark — retire them; then retire this chunk's own guards
        // (their Aborted marks are no-ops at versions already durable).
        lease.retire_parked();
        guards.drain(0..end - start);
        old = BookmarkV3 {
            fence: old.fence,
            bookmark: new_bookmark,
            clean_through: Some(
                old.clean_through
                    .map_or(chunk_local_end, |j| j.max(chunk_local_end)),
            ),
            outstanding_receipt: None,
        };
        start = end;
    }
    Ok(ApplyOutcome::Applied {
        local_version: floor,
    })
}

/// One retryable attempt of one fresh chunk: build the plan, durably commit
/// the Prepared receipt (idempotent — same receipt bytes — so a fallible
/// flush after the Prepared commit retries instead of dropping the chunk's
/// guards), then the post-Prepared tail.
#[allow(clippy::too_many_arguments)]
async fn try_chunk(
    repo: &RepoInstance,
    permit: &RepoWritePermit,
    info: &Arc<dyn Store>,
    gate: &Arc<RepoTxGate>,
    events: &[ReplicatedEvent],
    origins: &[usize],
    prepared: &[PreparedEvent],
    old: &BookmarkV3,
) -> DbResult<(u64, u64)> {
    let bindings = collect_bindings(events, origins);
    let leader_start = prepared[0].event.commit_version;
    let leader_end = prepared[prepared.len() - 1].event.commit_version;
    let mut plan = stage::build_plan(
        repo,
        prepared,
        bindings,
        old.bookmark,
        leader_start,
        leader_end,
    )
    .await?;
    // Phase-aware retry (finding 3): once this chunk's Materialized receipt
    // is durable, rewriting the Prepared-phase bytes over it would ERASE the
    // recorded absolute counters and force the retry to recompute them from
    // the live cache (where the batch's own history rows read back as
    // pre-existing) — skip the rewrite and let `run_chunk_tail` replay the
    // recorded Materialized receipt. Matching is by CONTENT (`same_batch`),
    // not by `receipt_id`: the id is a hash of phase+counters (see
    // `stage::set_counters`), so a freshly rebuilt Prepared plan can never
    // share it with the durable Materialized receipt even for the identical
    // batch.
    let already_materialized = matches!(
        resume::lookup(repo, info).await?.as_ref(),
        Some(outstanding)
            if outstanding.receipt.phase == ReceiptPhase::Materialized
                && same_batch(&outstanding.receipt, &plan.receipt),
    );
    if !already_materialized {
        stage::commit_prepared(repo, permit, info, &plan, old).await?;
        #[cfg(test)]
        fail_after(repo.name(), Phase::ReceiptPrepared)?;
    }
    run_chunk_tail(repo, permit, info, gate, &mut plan, old).await
}

/// Post-Prepared tail of one fresh chunk: the idempotent materialization,
/// the durable Materialized batch, the publication build, and the
/// durability tail. Returns `(new_bookmark, chunk_local_end)`.
async fn run_chunk_tail(
    repo: &RepoInstance,
    permit: &RepoWritePermit,
    info: &Arc<dyn Store>,
    gate: &Arc<RepoTxGate>,
    plan: &mut stage::BatchPlan,
    old: &BookmarkV3,
) -> DbResult<(u64, u64)> {
    // Phase-aware, counter-idempotent retry (finding 3): on EACH attempt,
    // load the durable outstanding receipt FIRST. If this chunk's
    // Materialized receipt is already durable (content-matched via
    // `same_batch`), the recorded absolute counters are replayed — never
    // recomputed from the live cache/MVCC, where the batch's own history
    // rows read back as pre-existing and the absolute count would be
    // overwritten with a wrong value — and the durable batch is NOT
    // re-staged: the pipeline goes straight to the checkpoint tail. Only
    // when no Materialized receipt is durable yet is `materialize_from_plan`
    // used (counters computed once against the true pre-state).
    let stored = resume::lookup(repo, info).await?;
    if matches!(
        stored.as_ref(),
        Some(outstanding)
            if outstanding.receipt.phase == ReceiptPhase::Materialized
                && same_batch(&outstanding.receipt, &plan.receipt)
    ) {
        // Materialized -> straight to the checkpoint (round 4 / finding 1a):
        // the durable matching receipt IS the Materialized bytes —
        // re-staging/committing the batch would be a pure rewrite AND the
        // publication is built STRAIGHT from the STORED receipt
        // (`publication_from_receipt`: decoded downstream events + the
        // receipt's own recorded absolute counters + ops segmentation), NOT
        // from the freshly rebuilt plan — the stored receipt is the single
        // durable authority for what gets published. Then go directly to
        // `durable_tail` (journal verify/repair + confirm + checkpoint all
        // happen inside it, so every idempotency property is kept).
        let stored = &stored.expect("matched above").receipt;
        // Test seam on the FAST path too (reviewer P1 #2): without this
        // check a sticky `fail_after = ReceiptMaterialized` flag could never
        // bite after the first Materialized commit — the retry takes this
        // fast path straight to `durable_tail` and succeeds, so a test
        // expecting the pipeline to PARK on the sticky flag would observe a
        // successful retry instead. Attempt trace with the flag sticky:
        //   attempt 1: no stored receipt -> materialize+commit -> seam fires
        //              (Storage "injected") -> retry
        //   attempt 2: fast path -> seam fires again -> retry
        //   attempt 3: fast path -> seam fires -> attempt >= 3 -> PARK.
        #[cfg(test)]
        fail_after(repo.name(), Phase::ReceiptMaterialized)?;
        let publication = publish::publication_from_receipt(repo, stored).await?;
        let chunk_local_end = stored
            .mapping
            .iter()
            .map(|mapping| mapping.local_version)
            .max()
            .unwrap_or(0);
        let new_bookmark = durable_tail(repo, permit, info, gate, stored, publication, old).await?;
        return Ok((new_bookmark, chunk_local_end));
    }
    let work = stage::materialize_from_plan(repo, plan).await?;
    let (receipt, counters) =
        stage::commit_materialized_work(repo, permit, info, &work, old).await?;
    #[cfg(test)]
    fail_after(repo.name(), Phase::ReceiptMaterialized)?;
    let publication = publish::publication_from_plan(repo, plan, &counters).await?;
    let chunk_local_end = receipt
        .mapping
        .iter()
        .map(|mapping| mapping.local_version)
        .max()
        .unwrap_or(0);
    let new_bookmark = durable_tail(repo, permit, info, gate, &receipt, publication, old).await?;
    Ok((new_bookmark, chunk_local_end))
}

/// Shared post-materialization durability tail: verify journal coverage
/// (repairing through the WRITER, never direct writes, when a Materialized
/// receipt's rows are missing), confirm the feed, adopt the absolute
/// counters, checkpoint bookmark+J WITH the RAM publication in the same
/// poll, then broadcast.
async fn durable_tail(
    repo: &RepoInstance,
    permit: &RepoWritePermit,
    info: &Arc<dyn Store>,
    gate: &Arc<RepoTxGate>,
    receipt: &ReplicationReceipt,
    publication: publish::Publication,
    old: &BookmarkV3,
) -> DbResult<u64> {
    if let Err(first) = checkpoint::verify_journal(repo, receipt).await {
        log::warn!(
            "follower receipt journal coverage incomplete ({first}); repairing \
             via install_durable_events"
        );
        let handle = repo.changefeed().await?;
        let records = stage::journal_records(receipt)?;
        handle
            .feed
            .install_durable_events(records)
            .await
            .map_err(|error| DbError::Storage(format!("receipt journal repair: {error}")))?;
        checkpoint::verify_journal(repo, receipt).await?;
    }
    let local_end = receipt
        .mapping
        .iter()
        .map(|mapping| mapping.local_version)
        .max()
        .unwrap_or(0);
    checkpoint::confirm_journal(repo, local_end).await?;
    publish::adopt_counters(repo, &publication).await?;
    checkpoint::checkpoint(repo, permit, info, receipt, old, gate, &publication).await?;
    let floor = publication.floor;
    for event in &publication.events {
        repo.broadcast_changefeed_event(Some(Arc::clone(&event.downstream)));
    }
    log::info!(
        "follower receipt: applied batch through local version {floor} \
         (upstream {}..={})",
        receipt.upstream_start,
        receipt.upstream_end
    );
    Ok(old.bookmark.max(receipt.upstream_end))
}

/// Phase-invariant CONTENT match between the durable receipt and the freshly
/// rebuilt plan (finding 1b — full identity, not just the range): the exact
/// local allocations (`mapping`), the upstream batch boundary
/// (`upstream_start`/`upstream_end`), the rollback point
/// (`predecessor_bookmark`), the fence identity (`lineage_id`,
/// `upstream_epoch`), the field-name `bindings`, and the downstream event
/// BYTES. All of these are fixed at Prepared time by `build_plan` (which
/// reads fence lineage/epoch and serializes the downstream bytes) and never
/// change when the receipt is rewritten to Materialized, so the predicate
/// matches the identical batch in either phase and rejects any different
/// batch. `receipt_id` itself is NOT usable across phases: it hashes the
/// whole receipt including phase AND counters (see `stage::set_counters`),
/// and the fresh plan is always rebuilt as Prepared with empty counters, so
/// the freshly computed id can never equal the durable Materialized id even
/// for the identical batch.
fn same_batch(stored: &ReplicationReceipt, plan_receipt: &ReplicationReceipt) -> bool {
    stored.mapping == plan_receipt.mapping
        && stored.upstream_start == plan_receipt.upstream_start
        && stored.upstream_end == plan_receipt.upstream_end
        && stored.predecessor_bookmark == plan_receipt.predecessor_bookmark
        && stored.lineage_id == plan_receipt.lineage_id
        && stored.upstream_epoch == plan_receipt.upstream_epoch
        && stored.bindings == plan_receipt.bindings
        && stored.downstream_events == plan_receipt.downstream_events
}

/// Inject the test seam's one-shot (or sticky) post-phase failure: returns
/// `DbError::Storage("injected")` — or `DbError::RepoRequiresReopen("injected")`
/// when the reopen-kind flag is set — exactly when the flag fires for `phase`.
#[cfg(test)]
fn fail_after(repo_name: &str, phase: Phase) -> DbResult<()> {
    if test_seam::take_fail_after(repo_name, phase) {
        if test_seam::fail_after_reopen(repo_name) {
            Err(DbError::RepoRequiresReopen("injected".into()))
        } else {
            Err(DbError::Storage("injected".into()))
        }
    } else {
        Ok(())
    }
}

/// Deduplicated `(interned_id, name)` bindings of the chunk's original
/// events, in first-seen order. Conflicts were already rejected by
/// `prepare_event`.
fn collect_bindings(events: &[ReplicatedEvent], origins: &[usize]) -> Vec<ReceiptBinding> {
    let mut seen = TFxMap::<u64, ()>::default();
    let mut bindings = Vec::new();
    for &origin in origins {
        for (id, name) in &events[origin].field_bindings {
            if seen.insert(*id, ()).is_none() {
                bindings.push(ReceiptBinding {
                    interned_id: *id,
                    name: name.clone(),
                });
            }
        }
    }
    bindings
}
