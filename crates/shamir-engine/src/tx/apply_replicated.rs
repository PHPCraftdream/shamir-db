//! R1-a — apply a single leader-emitted [`ChangelogEvent`] on a follower.
//!
//! This is the engine core of single-hop follower replication. The follower
//! pulls already-committed events from the leader's changefeed journal and
//! applies them to its local repo as a TRUSTED raw write: NO SSI validation,
//! NO WAL-begin, NO high-level Set/Delete op resolution. The leader already
//! ran the full write-path (validators, CAS, SSI-ledger, interner
//! derivation); the follower is an executor of the decided result, not a
//! re-decider. See `docs/roadmap/REPLICATION.md` §4.1 / §4.
//!
//! ## Versions — local, not leader's
//!
//! Per the R1-a task brief (which refines §4.1's "version = source"), the
//! follower allocates a LOCAL commit version via its own
//! [`RepoTxGate::assign_next_version`]. The leader's `commit_version` is
//! carried in the event only for IDEMPOTENCY (R1-b stores the high-water
//! mark of applied leader versions durably and feeds it back as
//! `applied_watermark`). Local-version allocation is what lets the follower
//! emit its OWN changefeed event downstream (chain replication) and keeps
//! its gate / MVCC floor internally consistent.
//!
//! ## Idempotency (V4, §4)
//!
//! [`apply_replicated`] takes the currently-applied leader watermark as a
//! parameter. If `event.commit_version <= applied_watermark` the event has
//! already been applied and the call is an O(1) no-op returning
//! [`ApplyOutcome::Skipped`] WITHOUT touching the store. The durable
//! bookmark itself is R1-b's responsibility (a separate task).
//!
//! ## Raw-apply primitive
//!
//! Each [`RecordChange`] carries the RAW storage key (16-byte `RecordId`
//! for data tables — the leader already projected high-level Set/Delete
//! ops + interner derivation into raw bytes) and the FULL new record bytes
//! (Put) or `None` (Delete). The follower reuses the SAME low-level
//! store-primitive the V2 WAL recovery path uses
//! ([`MvccStore::apply_committed_ops`](shamir_tx::MvccStore::apply_committed_ops)
//! — which is itself the pre-D2-cutover composition of
//! `apply_committed_visible` + `write_committed_to_history`), routing the
//! raw `(key, value)` pair straight into the version-log at the
//! follower-local commit version. For unattached tables (no `MvccStore` —
//! system/test), it falls back to the direct `base.transact`, matching
//! `replay_v2_op`'s non-MVCC branch.
//!
//! ## finalize-tail reuse
//!
//! The full [`finalize_sync_post_publish`](crate::tx::finalize::finalize_sync_post_publish)
//! tail is NOT reusable verbatim: it requires a `&TxContext` (for
//! `promote_vectors` — HNSW graph promote) and a `PostPublishState`
//! (produced by [`materialize`](crate::tx::materialize::materialize),
//! carrying the A5 interner-delta max-id used to gate WAL truncation). A
//! replicated raw-apply has NEITHER: there is no `TxContext` (no client tx
//! — the event is already committed on the leader), no WAL entry to
//! truncate (the leader's WAL is its own durability; the follower writes
//! history directly, not through a local WAL), and no staged vectors
//! (record-level replication carries bytes only — HNSW is rebuilt on open
//! on the follower). We therefore reuse the tail's three reusable halves
//! directly and skip the two that do not apply:
//!
//!  - [`RepoInstance::emit_changefeed_event`] — re-emit the event on the
//!    follower's OWN changefeed so downstream chain replicas can pull it.
//!    The event is re-projected with the follower-local version so the
//!    downstream's idempotency watermark keys on the follower's monotonic
//!    sequence, not the (different) leader version.
//!  - [`RepoTxGate::mark_durable`] — advance the durable watermark so the
//!    freshly-written history entry is reported as durable (matches the
//!    post-cutover drainer contract).
//!  - `persist_markers` (Phase 6.5 recovery markers) — best-effort persist
//!    of `last_committed` so a follower restart re-seeds its gate floor.
//!
//!  - SKIPPED: `promote_vectors` (no staged vectors in a record-level
//!    event).
//!  - SKIPPED: `drainer().wake()` (no local WAL entry was offered — the
//!    follower wrote history directly via `apply_committed_ops`; the
//!    drainer has nothing to drain for this version).
//!  - SKIPPED: A5 interner checkpoint (no interner delta in a record-level
//!    event; the follower's interner is populated by its own local writes
//!    and by decode-on-read, NOT by replication).

use std::sync::Arc;

use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{KvOp, Store};
use shamir_tx::{ChangeOp, ChangelogEvent};

use crate::repo::RepoInstance;
use crate::table::table_manager::table_token_for;
use crate::tx::commit_phases::persist_markers;

/// The local outcome of applying one replicated [`ChangelogEvent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The event was applied to the local repo. Carries the follower-local
    /// commit version allocated for it (NOT the leader's `commit_version`).
    Applied { local_version: u64 },
    /// The event was skipped because its leader `commit_version` is at or
    /// below the caller-supplied `applied_watermark` (already applied in a
    /// prior call — idempotent re-delivery).
    Skipped,
}

/// Idempotently apply one leader-originated [`ChangelogEvent`] to the
/// follower's local repo as a trusted raw write.
///
/// See the module docs for the version-allocation, raw-apply primitive, and
/// finalize-tail reuse rationale.
///
/// `applied_watermark` is the highest leader `commit_version` the caller
/// has durably recorded as applied (R1-b owns the durable bookmark). The
/// function performs an O(1) `event.commit_version <= applied_watermark`
/// comparison and short-circuits to [`ApplyOutcome::Skipped`] without
/// touching the store.
///
/// Returns [`ApplyOutcome::Applied`] with the follower-local commit
/// version on success. A `Skipped` outcome is returned BEFORE any version
/// is allocated (a skip consumes no version), so the gate floor is
/// untouched. On error the caller MUST NOT advance its
/// `applied_watermark` — re-delivery will retry with a fresh local version.
pub async fn apply_replicated(
    repo: &RepoInstance,
    event: &ChangelogEvent,
    applied_watermark: u64,
) -> DbResult<ApplyOutcome> {
    // V4 §4 idempotency: O(1) watermark check, no per-record scan. A
    // re-delivered event (at-most-once transport with redelivery) is a
    // silent no-op. Strictly-greater-than so an exact re-delivery of the
    // last applied version is also a skip.
    if event.commit_version <= applied_watermark {
        return Ok(ApplyOutcome::Skipped);
    }

    let gate = repo.tx_gate().await?;

    // Single-hop R1: the follower allocates a LOCAL commit version via its
    // own gate. The leader's `event.commit_version` is for idempotency
    // only; the local version is what the follower's MVCC log, gate floor,
    // and downstream changefeed key on.
    let local_version = gate.assign_next_version();

    // Group changes by table so each table's MvccStore / base store is hit
    // at most once with a single batched `transact` (O(tables), not
    // O(changes)). The token is a deterministic hash of the table name —
    // the same function the leader used to resolve its own tokens — so a
    // leader's `RecordChange.table` resolves to the same `per_table_mvcc`
    // entry on the follower.
    let mut by_table: shamir_collections::TFxMap<u64, Vec<KvOp>> =
        shamir_collections::TFxMap::default();
    for change in &event.changes {
        let token = table_token_for(&change.table);
        let op = match change.op {
            ChangeOp::Put => {
                let Some(value) = change.value.as_ref() else {
                    return Err(DbError::Internal(format!(
                        "apply_replicated: Put change on table '{}' key {:?} \
                         carried no value bytes",
                        change.table, change.key
                    )));
                };
                KvOp::Set(change.key.clone(), value.clone())
            }
            ChangeOp::Delete => KvOp::Remove(change.key.clone()),
        };
        by_table.entry(token).or_default().push(op);
    }

    let mut any_failed: Option<(u64, DbError)> = None;
    for (token, ops) in by_table {
        if any_failed.is_some() {
            // Stop on the first failure — a partial apply leaves the
            // follower with a prefix of the event's changes. The caller
            // MUST NOT advance the watermark; re-delivery re-applies
            // idempotently (last-write-wins at the data layer).
            break;
        }
        // Resolve the MvccStore the SAME way `apply_data_batch` /
        // `replay_v2_op` do: look up the per-table entry the
        // `RepoInstance` registers when the `TableManager` is
        // instantiated. A missing entry means the table is unattached
        // (system/test, no MVCC) — fall back to the direct base store.
        let mvcc_found = repo
            .per_table_mvcc()
            .read_async(&token, |_, mvcc| std::sync::Arc::clone(mvcc))
            .await;
        match mvcc_found {
            Some(mvcc) => {
                // apply_committed_ops = apply_committed_visible (overlay +
                // cell + floor) THEN write_committed_to_history (durable
                // version-log + ts + cell seed). Both halves run inline
                // here: there is no local WAL entry for the follower to
                // offer the drainer, so the split-half (ack-path visible +
                // background drainer durable) does not apply — we do the
                // pre-cutover "both halves at once" composition that
                // `apply_committed_ops` was built for.
                if let Err(e) = mvcc.apply_committed_ops(ops, local_version).await {
                    any_failed = Some((token, e));
                }
            }
            None => {
                // Unattached table: resolve the base data store via the
                // table manager and apply directly, matching
                // `replay_v2_op`'s non-MVCC Put/Delete branches.
                let tbl = match repo.table_by_token(token).await? {
                    Some(t) => t,
                    None => {
                        log::warn!(
                            "apply_replicated: table token {} not found \
                             in repo {}; skipping its {} changes (table may have \
                             been dropped on the follower)",
                            token,
                            repo.name(),
                            ops.len(),
                        );
                        continue;
                    }
                };
                let base: Arc<dyn Store> = Arc::clone(tbl.data_store());
                if let Err(e) = base.transact(ops).await {
                    any_failed = Some((token, e));
                }
            }
        }
    }

    if let Some((token, e)) = any_failed {
        // Roll back the version allocation by marking it aborted so the
        // gate's durable + visibility watermarks advance past it (a hole
        // the drainer / readers will skip). The caller does NOT advance its
        // applied watermark; re-delivery retries with a fresh local version.
        log::warn!(
            "apply_replicated: event leader_v {} tx {} failed on table token {}: \
             {e}; marking local_version {local_version} aborted",
            event.commit_version,
            event.tx_id,
            token
        );
        gate.mark_durable_aborted(local_version);
        return Err(e);
    }

    // ====================================================================
    // finalize-tail reuse (see module docs for why the full
    // `finalize_sync_post_publish` is not reusable verbatim).
    // ====================================================================

    // mark_durable: history was written inline by `apply_committed_ops`,
    // so the version is now durable. Advance the durable watermark so
    // `durable_watermark()` tracks `last_committed()` on a follower that
    // only ingests via replication.
    gate.mark_durable(local_version);

    // Phase 6.5 recovery markers (best-effort): persist `last_committed`
    // so a follower restart re-seeds its gate floor above this version.
    // A failure here is logged and does NOT fail the apply — same contract
    // as the commit-path's `post_publish_cleanup` (the gate's in-memory
    // floor is authoritative; the marker is a restart optimization).
    if let Err(e) = persist_markers(repo, &gate, local_version).await {
        log::warn!(
            "apply_replicated: persist_markers for local_version {} \
             (leader_v {} tx {}) failed: {e}; in-memory gate floor is still correct",
            local_version,
            event.commit_version,
            event.tx_id
        );
    }

    // emit_changefeed_event: re-emit on the follower's OWN changefeed so
    // downstream chain replicas can pull it. The event is re-projected
    // with the follower-local version (the leader's commit_version is
    // meaningless downstream — the downstream keys its idempotency on the
    // monotonic sequence of its upstream, which is THIS follower). The
    // re-emitted event carries the same actor, timestamp, and record
    // changes as the leader's; only the commit_version and repo name
    // change.
    let downstream_event = reproject_for_downstream(event, repo.name(), local_version);
    repo.emit_changefeed_event(Some(downstream_event)).await;

    Ok(ApplyOutcome::Applied { local_version })
}

/// Re-project a leader-originated [`ChangelogEvent`] for the follower's
/// downstream changefeed, swapping in the follower-local commit version
/// and the follower's repo name. The record changes, actor, and timestamp
/// are preserved verbatim — they ARE the replicated payload.
///
/// Returns `None` if the event has no changes (matching
/// `project_event`'s empty-footprint contract), but in practice a leader
/// never emits an empty event.
fn reproject_for_downstream(
    event: &ChangelogEvent,
    local_repo: &str,
    local_version: u64,
) -> ChangelogEvent {
    ChangelogEvent {
        repo: local_repo.to_string(),
        commit_version: local_version,
        tx_id: event.tx_id,
        actor: event.actor.clone(),
        timestamp_ns: event.timestamp_ns,
        changes: event.changes.clone(),
    }
}
