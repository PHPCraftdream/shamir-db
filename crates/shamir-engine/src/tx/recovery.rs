//! V2 WAL recovery — applies inflight WalEntryV2 entries on repo open.
//!
//! Crashes between commit_tx Phase 4 (WAL begin) and Phase 7 (WAL
//! commit) leave durable entries that need replay. Without recovery
//! tx writes are lost despite the WAL marker.
//!
//! Per stage 7.1 plan in docs/pre-transactional/08-tests-landing.md.

use shamir_collections::TFxMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::KvOp;
use shamir_tx::CompletionState;
use shamir_wal::WalOpV2;

use crate::repo::RepoInstance;

/// cancel-safe: NO — multi-step state mutation. Each branch issues a
/// table lookup followed by one or more store writes (and the broadcast
/// branches loop across tables). Cancellation mid-broadcast leaves the
/// store in a partially-applied state; data-layer ops are idempotent so
/// a subsequent re-replay converges, but the caller cannot rely on
/// atomicity at this boundary.
///
/// Replay a single WalOpV2 against the given RepoInstance.
///
/// Stage 7.1.c: Put / Delete / CounterDelta are applied for real.
/// Stage 7.1.d: IndexPut / IndexDel are applied (table_id_interned=0
///   broadcasts to all tables' info_stores).
/// InternerOverlayMerge is deferred (Stage 5 — repo-level interner).
pub async fn replay_v2_op(op: &WalOpV2, repo: &RepoInstance) -> DbResult<()> {
    match op {
        WalOpV2::Put {
            table_id_interned,
            rid,
            body,
        } => {
            let tbl = match repo.table_by_token(*table_id_interned).await? {
                Some(t) => t,
                None => {
                    log::warn!(
                        "replay_v2_op Put: table token {} not found in repo {}; \
                         skipping (table may have been dropped)",
                        table_id_interned,
                        repo.name()
                    );
                    return Ok(());
                }
            };
            // Collapse-main: attached tables (with an MvccStore) are recovered
            // into the version log by `seed_version_cache_for_entry` (writes
            // `history.set(key‖v, body)`); the raw `data_store` is vestigial for
            // them, so skip the redundant write. Only unattached tables (no
            // MvccStore — system/test) still materialize into `data_store`.
            if tbl.mvcc_store().is_none() {
                tbl.data_store().set(rid.to_bytes(), body.clone()).await?;
            }
            Ok(())
        }
        WalOpV2::Delete {
            table_id_interned,
            rid,
        } => {
            let tbl = match repo.table_by_token(*table_id_interned).await? {
                Some(t) => t,
                None => {
                    log::warn!(
                        "replay_v2_op Delete: table token {} not found; skipping",
                        table_id_interned
                    );
                    return Ok(());
                }
            };
            // Collapse-main: attached tables record the delete tombstone into
            // the version log via `seed_version_cache_for_entry`
            // (`history.set(key‖v, EMPTY)`); the raw `data_store` is vestigial,
            // so skip the redundant remove. Only unattached tables (no
            // MvccStore) still mutate `data_store`.
            //
            // Delete-replay is idempotent: a key already gone (the delete
            // landed before the crash, or the record never existed) is
            // benign and must not fail recovery. But a genuine storage
            // I/O error must propagate — swallowing it would report a
            // successful replay having NOT applied the delete.
            if tbl.mvcc_store().is_none() {
                match tbl.data_store().remove(rid.to_bytes()).await {
                    Ok(_) | Err(DbError::NotFound(_)) => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        }
        WalOpV2::CounterDelta {
            table_id_interned: _,
            delta: _,
        } => {
            // CRIT-3 idempotency (Option A):
            //
            // The happy-path commit (`commit::commit_tx_inner` Phase 5b)
            // now applies counter deltas in-memory BEFORE writing the
            // WAL commit marker. If the marker survives a crash mid-
            // Phase 7, the in-memory counter has already advanced —
            // replaying the delta here would double-count.
            //
            // Skipping the replay accepts a worst-case drift of one
            // tx-worth of counter delta when the crash falls between
            // Phase 5b (counter applied in RAM) and Phase 6.5 (marker
            // persistence). On restart the in-memory `RecordCounter`
            // re-hydrates from the info_store, which only reflects
            // whatever was durably persisted before the crash. The
            // authoritative durability is `last_committed_version`
            // (Phase 6.5) — the record counter is a metric.
            //
            // The cleaner Option B (write a per-tx "counter-applied"
            // marker that recovery checks) is not worth the I/O for
            // an at-most-by-one drift on a counter that the doctor
            // already reconciles by full data-store scan.
            Ok(())
        }
        WalOpV2::IndexPut {
            table_id_interned,
            idx_id: _,
            key,
            value,
        } => {
            if *table_id_interned != 0 {
                let tbl = match repo.table_by_token(*table_id_interned).await? {
                    Some(t) => t,
                    None => {
                        log::warn!(
                            "replay_v2_op IndexPut: table token {} not found",
                            table_id_interned
                        );
                        return Ok(());
                    }
                };
                tbl.info_store().set(key.clone(), value.clone()).await?;
                return Ok(());
            }
            for name in repo.list_table_names() {
                if let Ok(tbl) = repo.get_table(&name).await {
                    let _ = tbl.info_store().set(key.clone(), value.clone()).await;
                }
            }
            Ok(())
        }
        WalOpV2::IndexDel {
            table_id_interned,
            idx_id: _,
            key,
        } => {
            if *table_id_interned != 0 {
                let tbl = match repo.table_by_token(*table_id_interned).await? {
                    Some(t) => t,
                    None => {
                        log::warn!(
                            "replay_v2_op IndexDel: table token {} not found",
                            table_id_interned
                        );
                        return Ok(());
                    }
                };
                // Idempotent like Delete: a missing posting is benign,
                // but a real storage error must propagate rather than be
                // swallowed as a phantom success.
                match tbl.info_store().remove(key.clone()).await {
                    Ok(_) | Err(DbError::NotFound(_)) => {}
                    Err(e) => return Err(e),
                }
                return Ok(());
            }
            // Broadcast (table_id_interned == 0): the same key is removed
            // from every table's info_store. Most tables never held it, so
            // NotFound is the expected norm and benign — but a genuine I/O
            // error on any table propagates.
            for name in repo.list_table_names() {
                if let Ok(tbl) = repo.get_table(&name).await {
                    match tbl.info_store().remove(key.clone()).await {
                        Ok(_) | Err(DbError::NotFound(_)) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            Ok(())
        }
        WalOpV2::InternerOverlayMerge { entries } => {
            // Stage I: the interner is per-REPO, so the old per-table
            // broadcast collapses to a single merge against the repo
            // interner. `touch_ind` is idempotent — interning a key that
            // already exists returns the existing id. (Pre-Stage-I this
            // looped over every table because each had its own interner;
            // now they all share one.)
            if let Ok(repo_interner) = repo.repo_interner().await {
                if let Ok(interner) = repo_interner.get().await {
                    for (_overlay_id, key_str) in entries {
                        let _ = interner.touch_ind(key_str);
                    }
                    let _ = repo_interner.persist().await;
                }
            }
            Ok(())
        }
    }
}

/// cancel-safe: NO — iterates inflight entries, replaying each. Replay
/// ops are idempotent at the data layer so eventual convergence is fine
/// (entries stay in the segment until F6 truncation and the next
/// recovery replays them again), but the function itself is not safe to
/// drop mid-flight as a single atomic step.
///
/// Walk all inflight V2 WAL entries for the repo and replay each one.
///
/// HIGH-5: entries are sorted by `commit_version` ascending before
/// replay so the order in which post-crash recovery applies multi-tx
/// state matches the order the original commit pipeline assigned.
/// `wal.recover()` returns entries in WAL append order, but txn_id
/// and commit_version are allocated independently — two concurrent
/// transactions can commit out of append order. Without this sort,
/// last-write-wins ops (Put/IndexPut) would resolve to the wrong
/// final value.
///
/// Legacy entries written before HIGH-5 carry `commit_version = 0`;
/// they sort first, preserving the previous lexical-key behaviour
/// on mixed-version corpora.
///
/// Called from `RepoInstance::recover_v2_inflight`.
///
/// CRIT-B: restoring MVCC version state.
///
/// 1. Force gate construction up front (`repo.tx_gate()`). The gate seeds
///    its `version_counter`/`last_committed_version` floor from
///    `max(persisted marker, max inflight commit_version)` (see
///    `RepoInstance::tx_gate`), so `assign_next_version()` is already
///    guaranteed to exceed every version the entries below replay at —
///    this is the part that fixes the monotonicity violation, because the
///    gate exposes no after-the-fact counter setter.
/// 2. Replay all inflight entries (sorted by `commit_version`, HIGH-5)
///    and remove their markers.
/// 3. Persist `last_committed = gate.last_committed()` so the recovered
///    floor survives the *next* restart. Once step 2 clears the markers,
///    the inflight pre-scan in step 1 would find nothing, so the durable
///    marker must now carry the recovered max or the floor would rewind.
pub async fn recover_inflight_v2(repo: &RepoInstance) -> DbResult<usize> {
    // Step 1: build the gate before replay so its version floor is seeded
    // from the inflight commit_versions while the markers still exist.
    let gate = repo.tx_gate().await?;

    let wal = repo.repo_wal().await?;
    let mut entries = wal.recover().await?;
    entries.sort_by_key(|e| e.commit_version);
    let count = entries.len();

    // Track the highest replayed commit_version for logging / assertion.
    // entries are sorted ascending so the last one carries the max, but
    // fold defensively in case of legacy commit_version == 0 entries.
    let mut max_replayed = 0u64;
    for entry in entries {
        max_replayed = max_replayed.max(entry.commit_version);
        replay_v2_entry(&entry, repo).await?;
        // P1d: mark version as materialized so the completion tracker
        // rebuilds the contiguous prefix. Every durable WAL entry is
        // treated as Materialized — there is no commit/abort marker
        // distinction in the WAL (durable = committed). Legacy entries
        // with commit_version == 0 are skipped (watermark already ≥ 0).
        if entry.commit_version > 0 {
            gate.completion()
                .mark(entry.commit_version, CompletionState::Materialized);
            // D2 P1d-2b: `replay_v2_entry` just wrote this version's data into
            // `history` (via `write_committed_to_history`) — it is now durable.
            // Advance the durable watermark so a freshly-opened repo's
            // `durable_watermark()` catches up to its `last_committed()` (on
            // open the overlay is empty, so reads go straight to history, and
            // the durable/visibility gap must be closed). The drainer's normal
            // warm path does this same `mark_durable`; recovery is the cold
            // path doing it for the inflight tail.
            gate.mark_durable(entry.commit_version);
        }
        // No-op: there are no per-entry markers; the segment is cleaned by
        // F6 truncation and replay is idempotent.
        wal.commit(entry.txn_id).await?;
    }

    if count > 0 {
        // P1d: sync last_committed from the tracker watermark so readers
        // see the full recovered prefix without gaps.
        gate.sync_last_committed_from_watermark();

        // Step 3: persist the (possibly advanced) floor so a clean
        // restart — which sees no inflight markers — still seeds the gate
        // above the recovered commit versions. `gate.last_committed()`
        // already reflects max(marker, max_inflight) from step 1.
        let floor = gate.last_committed();
        let info_store = repo.tx_info_store().await?;
        crate::meta::recovery_marker::save_last_committed(&info_store, floor).await?;

        log::info!(
            "V2 recovery replayed {} inflight tx entries (max commit_version {}, \
             gate version floor {})",
            count,
            max_replayed,
            floor
        );
    }
    Ok(count)
}

/// cancel-safe: NO — iterates ops in the entry, applying each one via
/// `replay_v2_op`. Cancellation mid-entry leaves the entry partially
/// applied; ops are idempotent at the data layer so re-replay converges,
/// but mid-flight cancellation is not atomic.
///
/// Replay all ops in one WAL entry. Iterates ops in declared order
/// (counter → interner → index → data per `wal_ops_from_tx` emission
/// order, though replay order is logically commutative within one entry).
///
/// CRIT-B: after the data ops land in `main`, seed each touched table's
/// `MvccStore::version_cache` with this entry's `commit_version`. Replay
/// writes go straight to `data_store()` (bypassing `apply_committed_ops`),
/// so without this the version cache would report `0` for recovered keys.
/// See [`seed_version_cache_for_entry`] for why this matters.
pub async fn replay_v2_entry(entry: &shamir_wal::WalEntryV2, repo: &RepoInstance) -> DbResult<()> {
    // A4-recovery + Stage I keystone: apply the interner delta BEFORE
    // replaying data ops. The ops' record bytes reference intern-ids from the
    // delta, so the interner must know them before any decode/replay occurs.
    //
    // Stage I: the interner is per-REPO (one id-namespace across every
    // table), so the old per-token routing (`repo.table_by_token(token)` →
    // `tbl.interner()`) collapses to a SINGLE resolution of the repo
    // interner. The first `u64` of each triple is now a repo-scope constant
    // (see `REPO_INTERNER_SCOPE`) and is ignored here. This is the
    // correctness keystone: every replayed record byte encodes ids from
    // THIS ONE interner, regardless of which table the op targets.
    if !entry.interner_delta.is_empty() {
        let repo_interner = repo.repo_interner().await?;
        let interner = repo_interner.get().await?;
        for (_scope, name, id) in &entry.interner_delta {
            interner.touch_with_id(name, *id).map_err(|e| {
                DbError::Internal(format!(
                    "replay tx {} interner delta failed: {}",
                    entry.txn_id, e
                ))
            })?;
        }
    }

    for op in &entry.ops {
        replay_v2_op(op, repo).await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "replay tx {} op failed: {}",
                entry.txn_id, e
            ))
        })?;
    }
    seed_version_cache_for_entry(entry, repo).await;
    Ok(())
}

/// cancel-safe: NO — routes each table's data ops through
/// `write_committed_to_history` (history transact + ts + cell seed).
/// Cancellation mid-loop leaves some tables written, others not; the ops are
/// idempotent (last-write-wins) so a re-replay / re-drain converges.
///
/// Write every data (`Put`/`Delete`) op in `entry` to the table's durable
/// version-log at the entry's `commit_version` (D2 P1d-2b: this is the
/// history-write half — the drainer and cold recovery share it). Index /
/// counter / interner ops do not flow through the `MvccStore` (it wraps only
/// the data store), so they are skipped here.
///
/// Looks up the table's `MvccStore` via `per_table_mvcc` — populated when
/// `table_by_token` (called during replay) instantiates the
/// `TableManager`. A missing entry (dropped/unconfigured table) is a
/// silent skip, matching the replay ops' own graceful handling.
async fn seed_version_cache_for_entry(entry: &shamir_wal::WalEntryV2, repo: &RepoInstance) {
    let v = entry.commit_version;

    // D2 P1d-2b: group this entry's DATA ops per table and route them through
    // `MvccStore::write_committed_to_history` — the SAME history-write half the
    // background drainer uses. This is what makes `replay_v2_entry` the genuine
    // history writer after the cutover (the ack-path writes only the overlay).
    // For an MVCC table this writes the version-log entry (Put body / Delete
    // tombstone), records the commit ts, and seeds the cell + floor. Cold
    // recovery (overlay empty) thus lands every recovered version in `history`
    // so post-restart reads resolve correctly; warm drain re-runs the same path
    // idempotently (last-write-wins).
    let mut by_table: TFxMap<u64, Vec<KvOp>> = TFxMap::default();
    for op in &entry.ops {
        let (table_id, kvop) = match op {
            WalOpV2::Put {
                table_id_interned,
                rid,
                body,
            } => (*table_id_interned, KvOp::Set(rid.to_bytes(), body.clone())),
            WalOpV2::Delete {
                table_id_interned,
                rid,
            } => (*table_id_interned, KvOp::Remove(rid.to_bytes())),
            _ => continue,
        };
        by_table.entry(table_id).or_default().push(kvop);
    }

    for (table_id, ops) in by_table {
        if let Some(mvcc) = repo
            .per_table_mvcc()
            .read_async(&table_id, |_, m| std::sync::Arc::clone(m))
            .await
        {
            // C2 + ts: write the version-log + commit ts + seed cell/floor.
            // Best-effort on error: leaving a partial history write inflight is
            // safe — the WAL marker is untouched on the recovery path (the
            // caller `recover_inflight_v2` only `wal.commit`s after this
            // returns Ok-ish), and the drainer / next recovery converges.
            if let Err(e) = mvcc.write_committed_to_history(&ops, v).await {
                log::warn!(
                    "seed_version_cache_for_entry: history write for tx {} \
                     table {} commit_version {} failed: {e}",
                    entry.txn_id,
                    table_id,
                    v
                );
            }
        }
    }
    // R3: advance the reader-visible floor even when no MVCC table matched
    // (e.g. an entry touching only unattached tables). `write_committed_to_history`
    // already advances it for MVCC tables; this is the catch-all. Monotonic
    // fetch_max — safe to call redundantly.
    if v > 0 {
        if let Ok(gate) = repo.tx_gate().await {
            gate.publish_committed_max(v);
        }
    }
}
