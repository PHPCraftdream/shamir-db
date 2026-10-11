//! R1-a — apply a single leader-emitted [`ChangelogEvent`] on a follower.
//!
//! This is the engine core of single-hop follower replication. The follower
//! pulls already-committed events from the leader's changefeed journal and
//! applies them to its local repo as a TRUSTED raw write: NO SSI validation,
//! NO WAL-begin, NO high-level Set/Delete op resolution. The leader already
//! ran the full write-path (validators, CAS, SSI-ledger, interner
//! derivation); the follower is an executor of the decided result, not a
//! re-decider. See `docs/dev-artifacts/roadmap/REPLICATION.md` §4.1 / §4.
//!
//! ## Trust precondition (#1199)
//!
//! This module performs **no validator/schema/DAC/integrity check** on
//! `event.changes` beyond optional payload validation and the mandatory
//! decoding needed to translate field ids by name. The entire
//! security of this path rests on the replication TRANSPORT already
//! having authenticated the sender as a legitimate leader/upstream before
//! this function is ever called — a precondition owned by sibling crates
//! (`shamir-connect`, `shamir-transport-tcp`/`-ws`, `shamir-server`'s
//! replication supervisor), not by this crate. See
//! `docs/dev-artifacts/roadmap/REPLICATION.md` §4.4 for the full threat
//! model and what is/isn't implemented against it.
//!
//! ## Versions — local, not leader's
//!
//! Per the R1-a task brief (which refines §4.1's "version = source"), the
//! follower allocates a LOCAL commit version via its own
//! [`RepoTxGate::assign_next_version_guarded`] (the RAII `VersionGuard`
//! variant). The leader's `commit_version` is carried in the event only for
//! IDEMPOTENCY (R1-b stores the high-water mark of applied leader versions
//! durably and feeds it back as `applied_watermark`). Local-version
//! allocation is what lets the follower emit its OWN changefeed event
//! downstream (chain replication) and keeps its gate / MVCC floor internally
//! consistent. The guarded allocator ensures every allocated version is
//! terminally marked exactly once in the gate's completion (visibility)
//! tracker — `Materialized` on success, `Aborted` on any failure / early
//! return — so the contiguous visibility watermark can never wedge at an
//! untracked version (audit A12).
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
//! follower-local commit version. Tables without MVCC or with secondary
//! indexes are rejected before allocation; there is no base-store fallback.
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
//!  - [`RepoInstance::prepare_journal_event`] + `publish_journaled` — resolve
//!    the follower's downstream event after all table applies succeed, then
//!    enqueue it (or record a gap) immediately before publishing the local
//!    version, with no await or fallible step between enqueue and commit.
//!    [`RepoInstance::broadcast_changefeed_event`] broadcasts the same Arc
//!    after publication and recovery markers. The event is re-projected with
//!    the follower-local version so downstream idempotency keys on this
//!    follower's monotonic sequence, not the (different) leader version.
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
//!  - Interner mappings are translated by name and persisted before allocation.

use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::KvOp;
use shamir_tx::{
    collect_referenced_ids, remap_inner_value_bytes, ChangeOp, ChangelogEvent, MvccStore,
};
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::RecordView;
use shamir_types::types::value::InnerValue;

use crate::repo::RepoInstance;
use crate::table::table_manager::table_token_for;
use crate::table::RecordCounter;
use crate::tx::commit_phases::persist_markers;
use crate::tx::publish_journaled::publish_journaled;

/// The local outcome of applying one replicated [`ChangelogEvent`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The event was applied to the local repo. Carries the follower-local
    /// commit version allocated for it (NOT the leader's `commit_version`).
    Applied { local_version: u64 },
    /// The event was skipped because its leader `commit_version` is at or
    /// below the caller-supplied `applied_watermark` (already applied in a
    /// prior call — idempotent re-delivery).
    Skipped,
    /// The event is incompatible with this follower. No version is spent,
    /// no data is written, and no downstream event is emitted.
    Rejected { reason: ApplyRejectReason },
}

/// Permanent incompatibilities detected before allocating a local version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyRejectReason {
    UnknownTable,
    NoMvccStore,
    IndexedTable { table: String, index: String },
    MissingValue,
    UndecodablePayload,
    MissingFieldBinding,
    ConflictingFieldBinding,
}

/// Follower-side trust policy for [`apply_replicated`] (#1199). See
/// `docs/dev-artifacts/roadmap/REPLICATION.md` §4.4 for the full threat
/// model this addresses — and does NOT address.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApplyPolicy {
    /// Trust the leader's decisions, skipping optional payload validation.
    /// Decoding required for field-id translation is never skipped.
    #[default]
    Trusted,
    /// Decode-validate every `Put`'s record bytes BEFORE allocating a
    /// version or touching the store: the same two-step decode
    /// `table/read_exec.rs` uses to classify corrupt records
    /// ([`RecordView::new`], falling back to [`InnerValue::from_bytes`] for
    /// legacy-encoded rows). Rejects the WHOLE event (`Rejected`, no version
    /// consumed) if any `Put` payload decodes as neither.
    ///
    /// This does NOT re-run schema/validator/DAC checks against the
    /// decoded record — see REPLICATION.md §4.4 for why that is explicitly
    /// out of scope here (no actor-authorization signal survives the wire,
    /// and schema/validator registries live in `shamir-db`, a crate this
    /// one cannot depend on). It only closes the "structurally garbage
    /// bytes get forwarded verbatim" gap — a decode failure here would
    /// otherwise surface later as a `corrupt_records` entry on some
    /// unrelated future read, or propagate silently to a downstream
    /// chain-replica via `reproject_for_downstream`.
    ValidatePayload,
}

struct PreparedTable {
    name: String,
    token: u64,
    mvcc: Arc<MvccStore>,
    counter: Arc<RecordCounter>,
    ops: Vec<KvOp>,
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
/// `policy` selects the trust model — see [`ApplyPolicy`]'s doc. Passing
/// [`ApplyPolicy::Trusted`] skips optional validation, not translation decoding.
///
/// Returns [`ApplyOutcome::Applied`] with the follower-local commit
/// version on success. A `Skipped` outcome is returned BEFORE any version
/// is allocated (a skip consumes no version), so the gate floor is
/// untouched. `Rejected` is terminal and also consumes no version. Neither a
/// rejection nor an error permits advancing `applied_watermark`. Storage
/// errors remain retryable with a fresh local version.
pub async fn apply_replicated(
    repo: &RepoInstance,
    event: &ChangelogEvent,
    field_bindings: &[(u64, String)],
    applied_watermark: u64,
    policy: ApplyPolicy,
) -> DbResult<ApplyOutcome> {
    // V4 §4 idempotency: O(1) watermark check, no per-record scan. A
    // re-delivered event (at-most-once transport with redelivery) is a
    // silent no-op. Strictly-greater-than so an exact re-delivery of the
    // last applied version is also a skip.
    if event.commit_version <= applied_watermark {
        return Ok(ApplyOutcome::Skipped);
    }

    if event
        .changes
        .iter()
        .any(|change| !repo.has_table(&change.table))
    {
        return Ok(ApplyOutcome::Rejected {
            reason: ApplyRejectReason::UnknownTable,
        });
    }

    let mut bindings = shamir_collections::TFxMap::default();
    let mut names = shamir_collections::TFxMap::default();
    for (id, name) in field_bindings {
        if bindings.insert(*id, name).is_some_and(|old| old != name)
            || names.insert(name, *id).is_some_and(|old| old != *id)
        {
            return Ok(ApplyOutcome::Rejected {
                reason: ApplyRejectReason::ConflictingFieldBinding,
            });
        }
    }
    let mut referenced = shamir_collections::TFxMap::default();
    let mut by_table: shamir_collections::TFxMap<String, Vec<KvOp>> =
        shamir_collections::TFxMap::default();
    for change in &event.changes {
        let op = match change.op {
            ChangeOp::Put => {
                let Some(value) = change.value.as_ref() else {
                    return Ok(ApplyOutcome::Rejected {
                        reason: ApplyRejectReason::MissingValue,
                    });
                };
                if policy == ApplyPolicy::ValidatePayload
                    && RecordView::new(value).is_err()
                    && InnerValue::from_bytes(value).is_err()
                {
                    return Ok(ApplyOutcome::Rejected {
                        reason: ApplyRejectReason::UndecodablePayload,
                    });
                }
                let decoded = match InnerValue::from_bytes(value) {
                    Ok(decoded) => decoded,
                    Err(_) => {
                        return Ok(ApplyOutcome::Rejected {
                            reason: ApplyRejectReason::UndecodablePayload,
                        });
                    }
                };
                collect_referenced_ids(&decoded, &mut referenced);
                KvOp::Set(change.key.clone().into(), value.clone())
            }
            ChangeOp::Delete => KvOp::Remove(change.key.clone().into()),
        };
        by_table.entry(change.table.clone()).or_default().push(op);
    }

    if referenced.keys().any(|id| !bindings.contains_key(id)) {
        return Ok(ApplyOutcome::Rejected {
            reason: ApplyRejectReason::MissingFieldBinding,
        });
    }

    let mut prepared: Vec<PreparedTable> = Vec::with_capacity(by_table.len());
    for (table_name, ops) in by_table {
        let table = match repo.get_table(&table_name).await {
            Ok(table) => table,
            Err(DbError::NotFound(_)) => {
                return Ok(ApplyOutcome::Rejected {
                    reason: ApplyRejectReason::UnknownTable,
                });
            }
            Err(error) => return Err(error),
        };
        let legacy_index = table
            .index_manager_ref()
            .iter_indexes()
            .chain(table.index_manager_ref().iter_unique_indexes())
            .map(|index| index.name_interned)
            .next()
            .or_else(|| {
                table
                    .sorted_indexes()
                    .iter_indexes()
                    .first()
                    .map(|index| index.name_interned)
            });
        let index_name = if let Some(id) = legacy_index {
            let interner = table.interner().get().await?;
            Some(
                interner
                    .get_str(&InternerKey::new(id))
                    .ok_or_else(|| DbError::Codec("missing follower index name".into()))?
                    .to_string(),
            )
        } else {
            table
                .index2_registry()
                .all_descriptors()
                .await
                .first()
                .map(|descriptor| descriptor.name.clone())
        };
        if let Some(index) = index_name {
            return Ok(ApplyOutcome::Rejected {
                reason: ApplyRejectReason::IndexedTable {
                    table: table_name,
                    index,
                },
            });
        }
        let token = table_token_for(&table_name);
        let Some(mvcc) = repo
            .per_table_mvcc()
            .read_sync(&token, |_, mvcc| Arc::clone(mvcc))
        else {
            return Ok(ApplyOutcome::Rejected {
                reason: ApplyRejectReason::NoMvccStore,
            });
        };
        prepared.push(PreparedTable {
            name: table_name,
            token,
            mvcc,
            counter: Arc::clone(table.counter()),
            ops,
        });
    }

    // All payloads, bindings, and tables passed preflight before first-touch.
    let interner_manager = repo.repo_interner().await?;
    let interner = interner_manager.get().await?;
    let mut remap = shamir_collections::TFxMap::default();
    for (id, name) in bindings {
        let local = interner
            .touch_ind(name)
            .map_err(|error| DbError::Codec(error.into()))?
            .into_key()
            .id();
        remap.insert(id, local);
    }
    // Required even on retry: an earlier persist may have failed after touch.
    interner_manager.persist().await?;

    let mut rewritten_event = event.clone();
    for change in &mut rewritten_event.changes {
        if change.op == ChangeOp::Put {
            let value = change
                .value
                .take()
                .ok_or_else(|| DbError::Codec("preflighted Put has no value".into()))?;
            change.value = Some(remap_inner_value_bytes(value, &remap).map_err(|error| {
                DbError::Codec(format!("replication translation failed: {error}"))
            })?);
        }
    }
    let mut rewritten_by_table: shamir_collections::TFxMap<String, Vec<KvOp>> =
        shamir_collections::TFxMap::default();
    for change in &rewritten_event.changes {
        let op = match &change.value {
            Some(value) if change.op == ChangeOp::Put => {
                KvOp::Set(change.key.clone().into(), value.clone())
            }
            _ => KvOp::Remove(change.key.clone().into()),
        };
        rewritten_by_table
            .entry(change.table.clone())
            .or_default()
            .push(op);
    }
    for PreparedTable { name, ops, .. } in &mut prepared {
        // Preserve per-table ordering while replacing every Put's bytes.
        *ops = rewritten_by_table
            .remove(name)
            .ok_or_else(|| DbError::Codec("missing preflighted table".into()))?;
    }

    let gate = repo.tx_gate().await?;

    // Single-hop R1: the follower allocates a LOCAL commit version via its
    // own gate. The leader's `event.commit_version` is for idempotency
    // only; the local version is what the follower's MVCC log, gate floor,
    // and downstream changefeed key on.
    //
    // A12: allocate via the GUARDED allocator (`assign_next_version_guarded`)
    // so the version's terminal-mark obligation in the gate's `completion`
    // (visibility) tracker is owned by RAII — the guard's `commit()` marks
    // it `Materialized` on the success path and `Drop` marks it `Aborted` on
    // ANY early return / panic / failure path. The BARE `assign_next_version`
    // previously used here left the completion tracker with NO entry for the
    // version on the success path (only `mark_durable_aborted` ran on
    // failure, and it touches the SEPARATE `durable_completion` tracker),
    // which permanently clogged the contiguous watermark at `version - 1`
    // until a restart re-seeded the floor. Holding the guard across every
    // exit path is compiler-enforced (no `Default`/`Clone`; `Drop` is the
    // only terminal transition aside from `commit()`).
    let version_guard = gate.assign_next_version_guarded();
    let local_version = version_guard.version();

    let mut any_failed: Option<(u64, DbError)> = None;
    for PreparedTable {
        name,
        token,
        mvcc,
        counter,
        ops,
    } in prepared
    {
        // Count only the final operation for each key against ACTUAL current
        // state, not a leader footprint or a snapshot captured for all tables.
        // A successfully applied earlier table therefore contributes zero on retry.
        let mut final_ops = shamir_collections::TFxMap::default();
        for op in &ops {
            match op {
                KvOp::Set(key, _) | KvOp::Remove(key) => {
                    final_ops.insert(key.clone(), op.clone());
                }
            }
        }
        let keys: Vec<Bytes> = final_ops
            .keys()
            .map(|key| Bytes::copy_from_slice(key.as_ref()))
            .collect();
        let delta = match mvcc.get_current_many(&keys).await {
            Ok(pre_images) => {
                let mut delta = 0i64;
                for (op, old) in final_ops.values().zip(pre_images) {
                    // Bounded by the number of distinct keys in this allocated batch.
                    delta += match (op, old.is_some()) {
                        (KvOp::Set(_, _), false) => 1,
                        (KvOp::Remove(_), true) => -1,
                        (KvOp::Set(_, _), true) | (KvOp::Remove(_), false) => 0,
                    };
                }
                delta
            }
            Err(error) => {
                any_failed = Some((token, error));
                break;
            }
        };
        // The overlay is first-writer-wins at (key, version). Collapse the
        // data batch too, so repeated keys actually expose the final op.
        let ops = final_ops.into_values().collect();
        if let Err(error) = mvcc.apply_committed_ops(ops, local_version).await {
            any_failed = Some((token, error));
            break;
        }
        // Best effort, like the leader: never turn a counter failure into a
        // retryable data error after successful apply. Finish before next table.
        if let Err(error) = counter.increment(delta).await {
            log::warn!("apply_replicated: counter for table {name}: {error}");
        }
    }

    if let Some((token, e)) = any_failed {
        // Roll back the version allocation: the `VersionGuard` held in
        // `version_guard` is NOT committed here, so its `Drop` (firing at
        // the `return` below) marks the version `Aborted` in BOTH the
        // visibility tracker (`completion`) AND the durable tracker
        // (`durable_completion`), advancing both contiguous watermarks past
        // the hole. This subsumes the previous explicit
        // `gate.mark_durable_aborted(local_version)` call (which marked only
        // the durable tracker) AND adds the previously-missing visibility-
        // tracker terminal mark — closing the A12 clog. The caller does NOT
        // advance its applied watermark; re-delivery retries with a fresh
        // local version.
        log::warn!(
            "apply_replicated: event leader_v {} tx {} failed on table token {}: \
             {e}; marking local_version {local_version} aborted",
            event.commit_version,
            event.tx_id,
            token
        );
        return Err(e);
    }

    // ====================================================================
    // finalize-tail reuse (see module docs for why the full
    // `finalize_sync_post_publish` is not reusable verbatim).
    // ====================================================================

    // Resolve only after the last failure exit: an aborted apply must enqueue
    // nothing and must not mark a changefeed gap. Preserve translated bytes,
    // actor and timestamp, but key downstream replication on the local version.
    let downstream_event = reproject_for_downstream(&rewritten_event, repo.name(), local_version);
    let prepared = repo.prepare_journal_event(Some(downstream_event)).await;

    // A12: discharge the VersionGuard's terminal-mark obligation by calling
    // `commit()` — this marks the version `Materialized` in the gate's
    // VISIBILITY tracker (`completion`) and advances `last_committed_version`
    // from the resulting watermark. This is the previously-MISSING call that
    // closed the A12 clog (the bare `assign_next_version` + `mark_durable`
    // pair left the visibility tracker with no entry for the version).
    //
    // `commit()` consumes the guard (disarms its Drop), so the subsequent
    // `mark_durable` call below is NOT redundant: `commit()` covers the
    // visibility tracker, while `mark_durable` covers the SEPARATE durable
    // tracker (`durable_completion`) AND fires `durable_progress.notify_waiters()`
    // to wake any backpressured committer. Both calls are needed — this
    // matches the canonical ordering the rest of the commit pipeline uses
    // (`guard.commit()` first, `gate.mark_durable(v)` second — see
    // `durable_watermark_tests::mixed_tx_and_nontx_durable_equals_visibility_at_end`).
    // S5b-4: enqueue-or-gap and guard.commit() are one synchronous pair.
    let downstream_event = publish_journaled(repo, version_guard, prepared);

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

    // Live only: the same Arc was already journaled before publication.
    #[cfg(test)]
    repo.fire_post_publish_pre_broadcast_test_hook().await;
    repo.broadcast_changefeed_event(downstream_event);

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
