//! Orchestration (S5c-X PR6b-2): route one pull batch through the receipt
//! pipeline — resume an outstanding receipt if any, otherwise chunk the
//! prepared events and run Prepared → Materialized → adopt-counters →
//! checkpoint (bookmark commit + RAM publication in one poll) per chunk.
//!
//! Cancellation: the pipeline runs on a REPOSITORY-OWNED bounded worker
//! (`tokio::spawn` of `run_receipt_pipeline`). The caller awaits the
//! worker's `JoinHandle`; dropping the caller future drops only that await
//! — the worker keeps its cheap `Arc` clone of the repo and runs the
//! operation to completion or to a durable pause point. The worker is
//! BOUNDED because the admission mutex inside the pipeline serializes
//! applies (one at a time): concurrent callers queue on the mutex rather
//! than piling up unbounded work.

use std::sync::Arc;

use shamir_collections::TFxMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;
use shamir_tx::RepoTxGate;

use super::super::super::{prepare_event, ApplyOutcome, ApplyPolicy, PreparedEvent};
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
    // Repository-owned bounded worker (S5c-X design, failure matrix
    // "Cancellation"): the pipeline runs on a DETACHED task that owns a
    // cheap Arc clone of the repo. Dropping the caller's future drops only
    // the JoinHandle await — the operation runs to completion or to a
    // durable pause point. Bounded: the admission mutex inside the pipeline
    // serializes applies (one at a time), so concurrent callers queue on
    // the mutex rather than piling up unbounded work.
    let mut handle = tokio::spawn(run_receipt_pipeline(
        repo.clone(),
        Arc::clone(info),
        events.to_vec(),
        bookmark,
        policy,
    ));
    match (&mut handle).await {
        Ok(result) => result,
        Err(join_error) if join_error.is_cancelled() => Err(DbError::RepoRequiresReopen(
            "receipt apply task cancelled".into(),
        )),
        Err(join_error) => Err(DbError::Internal(format!(
            "receipt apply worker panicked: {join_error}"
        ))),
    }
}

async fn run_receipt_pipeline(
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
        let bindings = collect_bindings(events, &origins[start..end]);
        let leader_start = prepared[start].event.commit_version;
        let leader_end = prepared[end - 1].event.commit_version;
        let mut plan = stage::build_plan(
            repo,
            &prepared[start..end],
            bindings,
            old.bookmark,
            leader_start,
            leader_end,
        )
        .await?;
        stage::commit_prepared(repo, info, &plan, &old).await?;
        // From here the durable receipt owns this chunk's versions. The
        // guards are NEVER dropped before publication: the worker owns them
        // across materialize → Materialized batch → publication build →
        // durable_tail, so no Aborted mark can advance the watermark past an
        // unpublished version (the repository-owned worker cannot be dropped
        // mid-pipeline — only its caller's await can). Ordinary Storage I/O
        // failures retry the idempotent materialization inline (the durable
        // receipt is the redo authority); the 3rd failure or any
        // NON-retryable error (RepoRequiresReopen = quarantine, decode
        // errors) drops the chunk guards and returns — the reopen path,
        // where a watermark advance is then acceptable.
        let mut attempt = 0usize;
        let (new_bookmark, chunk_local_end) = loop {
            match run_chunk_tail(repo, info, gate, &mut plan, &old).await {
                Ok(done) => break done,
                Err(error) => {
                    attempt += 1;
                    if !matches!(error, DbError::Storage(_)) || attempt >= 3 {
                        guards.drain(0..end - start);
                        return Err(error);
                    }
                    log::warn!(
                        "follower receipt chunk tail failed (attempt {attempt}/3), \
                         retrying: {error}"
                    );
                }
            }
        };
        // By now the publication inside `checkpoint` has run
        // `publish_materialized` + `mark_durable` via the resume guards, so
        // the fresh guards' Drop Aborted marks land at versions already ≤
        // the watermark and are no-ops.
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
    let work = stage::materialize_from_plan(repo, plan).await?;
    let (receipt, counters) = stage::commit_materialized_work(repo, info, &work, old).await?;
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
