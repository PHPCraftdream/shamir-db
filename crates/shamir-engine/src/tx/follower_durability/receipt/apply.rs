//! Orchestration (S5c-X PR6b-2): route one pull batch through the receipt
//! pipeline — resume an outstanding receipt if any, otherwise chunk the
//! prepared events and run Prepared → Materialized → adopt-counters →
//! checkpoint (bookmark commit + RAM publication in one poll) per chunk.
//!
//! Cancellation: the pipeline runs on the REPOSITORY-OWNED bounded worker
//! (`resume::ReceiptWorker`, ONE lifecycle spawn per repo — see worker.rs).
//! The caller submits a job and awaits only the oneshot reply; dropping the
//! caller future drops that await only — the worker retains the job (with
//! its own repo clone) and runs it to completion or to a durable pause
//! point. Bounded: `try_send` into a capacity-4 queue; the admission mutex
//! inside the pipeline serializes applies (one at a time). Errors while
//! fresh chunk guards are held PARK those guards in the worker (finding 2);
//! retries re-load the durable receipt so the recorded counters are never
//! recomputed from the live cache after Materialized committed (finding 3).

use std::sync::Arc;

use shamir_collections::TFxMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;
use shamir_tx::RepoTxGate;

use super::super::super::{prepare_event, ApplyOutcome, ApplyPolicy, PreparedEvent};
#[cfg(test)]
use super::super::test_seam::{self, Phase};
use super::{checkpoint, publish, resume, stage};
use crate::meta::repl::bookmark_v3::{load_bookmark_v3, BookmarkV3};
use crate::meta::repl::{ReceiptBinding, ReceiptPhase, ReplicationReceipt, MAX_RECEIPT_EVENTS};
use crate::meta::repl_bookmark::{load_fence_record, load_replication_bookmark};
use crate::repo::RepoInstance;
use crate::tx::ReplicatedEvent;

pub(crate) async fn apply_batch_receipt(
    repo: &RepoInstance,
    info: &Arc<dyn Store>,
    events: &[ReplicatedEvent],
    bookmark: u64,
    policy: ApplyPolicy,
) -> DbResult<ApplyOutcome> {
    // Repository-owned bounded worker (review finding 4, round 2): the
    // pipeline runs on the repo's single lazily-spawned worker task. Only
    // the oneshot reply is awaited — a cancelled caller drops the receiver,
    // and the worker KEEPS RUNNING the job with its own repo clone. No
    // orphan `tokio::spawn` outside the worker's single lifecycle spawn.
    let worker = repo.receipt_worker().await?;
    worker
        .submit(
            repo.clone(),
            Arc::clone(info),
            events.to_vec(),
            bookmark,
            policy,
        )
        .await
}

pub(crate) async fn run_receipt_pipeline(
    repo: RepoInstance,
    info: Arc<dyn Store>,
    events: Vec<ReplicatedEvent>,
    bookmark: u64,
    policy: ApplyPolicy,
) -> DbResult<ApplyOutcome> {
    let gate = repo.tx_gate().await?;
    if gate.requires_reopen() {
        return Err(DbError::RepoRequiresReopen(repo.name().to_string()));
    }
    // Repository write-admission domain for the receipt path: serializes
    // receipt applies against marker/floor persistence AND against local DDL
    // (both take `ddl_admission` — a follower apply can never interleave
    // with a create/drop/rename index mid-pipeline). The contention model is
    // ONE follower apply at a time — this mutex is what bounds the
    // repository-owned worker queue, and the path stays off any read path.
    let _admission = repo.marker_write_mutex().lock().await;
    let _ddl = repo.ddl_admission_lock().await;
    if let Some(outstanding) = resume::lookup(&repo, &info).await? {
        resume::match_offered(&outstanding.receipt, &events)?;
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
        // Converge both resume phases on the SAME materialize/checkpoint/
        // publish row building.
        let work = match outstanding.receipt.phase {
            ReceiptPhase::Prepared => {
                stage::materialize_from_prepared_receipt(&repo, &outstanding.receipt).await?
            }
            ReceiptPhase::Materialized => {
                stage::materialize_from_materialized_receipt(&outstanding.receipt)?
            }
            ReceiptPhase::Checkpointed => {
                return Err(DbError::RepoRequiresReopen(
                    "bookmark references a Checkpointed receipt".into(),
                ));
            }
        };
        let (receipt, _counters) =
            stage::commit_materialized_work(&repo, &info, &work, &old).await?;
        let publication = publish::publication_from_receipt(&repo, &receipt).await?;
        let floor = publication.floor;
        let new_bookmark = durable_tail(&repo, &info, &gate, &receipt, publication, &old).await?;
        // Publication succeeded: parked guards' versions are ≤ the watermark
        // now — retire (drop) them, their Aborted marks are no-ops.
        repo.receipt_worker().await?.retire_parked_guards().await;
        if !tail.is_empty() {
            let tail: Vec<ReplicatedEvent> = tail.into_iter().cloned().collect();
            apply_fresh(&repo, &info, &gate, &tail, new_bookmark, policy).await?;
        }
        return Ok(ApplyOutcome::Applied {
            local_version: floor,
        });
    }
    apply_fresh(&repo, &info, &gate, &events, bookmark, policy).await
}

/// The fresh path: translate the whole batch (rejections stay pre-write, like
/// the legacy batch path), then chunk the prepared events greedily under the
/// receipt's combined structural bounds and run the pipeline per chunk.
async fn apply_fresh(
    repo: &RepoInstance,
    info: &Arc<dyn Store>,
    gate: &Arc<RepoTxGate>,
    events: &[ReplicatedEvent],
    bookmark: u64,
    policy: ApplyPolicy,
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
    let worker = repo.receipt_worker().await?;
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
        // PARKED in the worker (never dropped), because a dropped guard
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
                        worker.park_guards(parked).await;
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
        worker.retire_parked_guards().await;
        guards.drain(0..end - start);
        old = BookmarkV3 {
            fence: old.fence.clone(),
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
    // not by `receipt_id`: the id is a hash of phase+counters, so a freshly
    // rebuilt Prepared plan can never share it with the durable Materialized
    // receipt even for the identical batch.
    let already_materialized = matches!(
        resume::lookup(repo, info).await?.as_ref(),
        Some(outstanding)
            if outstanding.receipt.phase == ReceiptPhase::Materialized
                && same_batch(&outstanding.receipt, &plan.receipt),
    );
    if !already_materialized {
        stage::commit_prepared(repo, info, &plan, old).await?;
        #[cfg(test)]
        fail_after(repo.name(), Phase::ReceiptPrepared)?;
    }
    run_chunk_tail(repo, info, gate, &mut plan, old).await
}

/// Post-Prepared tail of one fresh chunk: the idempotent materialization,
/// the durable Materialized batch, the publication build, and the
/// durability tail. Returns `(new_bookmark, chunk_local_end)`.
async fn run_chunk_tail(
    repo: &RepoInstance,
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
        // Materialized -> straight to the checkpoint (round 4): the durable
        // matching receipt IS the Materialized bytes — re-staging/committing
        // the batch would be a pure rewrite. Decode the STORED receipt (it
        // carries the recorded absolute counters), build the publication
        // from the plan + the STORED counters, and go directly to
        // `durable_tail` (journal verify/repair + confirm + checkpoint all
        // happen inside it, so every idempotency property is kept).
        let stored = &stored.expect("matched above").receipt;
        let counters: Vec<(String, u64)> = stored
            .counters
            .iter()
            .map(|delta| (delta.table.clone(), delta.after))
            .collect();
        let publication = publish::publication_from_plan(repo, plan, &counters).await?;
        let chunk_local_end = stored
            .mapping
            .iter()
            .map(|mapping| mapping.local_version)
            .max()
            .unwrap_or(0);
        let new_bookmark = durable_tail(repo, info, gate, stored, publication, old).await?;
        return Ok((new_bookmark, chunk_local_end));
    }
    let work = stage::materialize_from_plan(repo, plan).await?;
    let (receipt, counters) = stage::commit_materialized_work(repo, info, &work, old).await?;
    #[cfg(test)]
    fail_after(repo.name(), Phase::ReceiptMaterialized)?;
    let publication = publish::publication_from_plan(repo, plan, &counters).await?;
    let chunk_local_end = receipt
        .mapping
        .iter()
        .map(|mapping| mapping.local_version)
        .max()
        .unwrap_or(0);
    let new_bookmark = durable_tail(repo, info, gate, &receipt, publication, old).await?;
    Ok((new_bookmark, chunk_local_end))
}

/// Shared post-materialization durability tail: verify journal coverage
/// (repairing through the WRITER, never direct writes, when a Materialized
/// receipt's rows are missing), confirm the feed, adopt the absolute
/// counters, checkpoint bookmark+J WITH the RAM publication in the same
/// poll, then broadcast.
async fn durable_tail(
    repo: &RepoInstance,
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
    checkpoint::checkpoint(repo, info, receipt, old, gate, &publication).await?;
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
/// rebuilt plan: the version mapping (exact local allocations), the upstream
/// batch boundary, and the predecessor bookmark. `receipt_id` is NOT usable
/// here — it hashes phase+counters (see `stage::set_counters`), and `plan` is
/// rebuilt each attempt as Prepared with empty counters, so the ids can never
/// match for the same batch across phases. These four fields are fixed at
/// Prepared time and never change when the receipt is rewritten to
/// Materialized, so the predicate matches the identical batch in either
/// phase and rejects any different batch.
fn same_batch(stored: &ReplicationReceipt, plan_receipt: &ReplicationReceipt) -> bool {
    stored.mapping == plan_receipt.mapping
        && stored.upstream_start == plan_receipt.upstream_start
        && stored.upstream_end == plan_receipt.upstream_end
        && stored.predecessor_bookmark == plan_receipt.predecessor_bookmark
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
