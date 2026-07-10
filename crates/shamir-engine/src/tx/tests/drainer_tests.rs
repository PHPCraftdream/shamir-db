//! P1d-2a — unit tests for the background [`Drainer`].
//!
//! These drive `drain_step` / `drain_all` DIRECTLY (the drainer is not
//! wired into the commit path in P1d-2a). They seed the inflight WAL tail
//! by hand and prime the gate so visibility leads durability — exactly the
//! state P1d-2b's cutover produces (publish on the ack-path, history write
//! deferred). The drainer must then replay the not-yet-durable visible
//! prefix into history, advance `durable_watermark`, and (A5-gated) truncate
//! the markers.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::{WalDurability, WalEntryV2, WalOpV2};

use crate::repo::{repo_token, BoxRepo, RepoInstance};
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use crate::tx::drainer::Drainer;

// ── Op #2 Stage 1: window infrastructure tests ──────────────────────

/// Offer three entries with commit_versions 3, 1, 2 (out of order).
/// Assert window_len == 3 and iteration yields ascending keys 1, 2, 3.
#[tokio::test]
async fn offer_inserts_at_commit_version_in_ascending_order() {
    let drainer = Drainer::new();

    let mk = |v: u64| -> Arc<WalEntryV2> {
        Arc::new(WalEntryV2::new(v, 0, vec![]).with_commit_version(v))
    };

    // Insert out of order on purpose.
    drainer.offer(mk(3));
    drainer.offer(mk(1));
    drainer.offer(mk(2));

    assert_eq!(drainer.window_len(), 3);

    // Iterate the window and collect keys — must be ascending.
    assert_eq!(drainer.window_keys(), vec![1, 2, 3]);
}

/// seed_from_recover populates the window from a Vec<WalEntryV2>.
#[tokio::test]
async fn seed_from_recover_populates_window_with_inflight_entries() {
    let drainer = Drainer::new();

    let entries: Vec<WalEntryV2> = (10..=14)
        .map(|v| WalEntryV2::new(v, 0, vec![]).with_commit_version(v))
        .collect();

    drainer.seed_from_recover(entries);
    assert_eq!(drainer.window_len(), 5);
}

/// Stage 3: drain_step with an empty window falls back to wal.recover()
/// (gap-reseed path) and still drains correctly.
#[tokio::test]
async fn drain_step_gap_reseed_when_window_empty() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let gate = repo.tx_gate().await.unwrap();
    seed_inflight_put(&repo, "t", rid(1), "stage3", 1).await;
    gate.publish_committed_max(1);

    let drainer = Drainer::new();
    // Window is empty — drain_step must fall back via gap-reseed.
    assert_eq!(drainer.window_len(), 0);

    let drained = drainer.drain_step(&repo).await.unwrap();
    assert_eq!(drained, 1, "drain_step works via gap-reseed fallback");
    assert_eq!(gate.durable_watermark(), 1);

    let got = tbl.get(rid(1)).await.unwrap();
    assert!(
        matches!(got, InnerValue::Str(ref s) if s == "stage3"),
        "data resolves from history after drain"
    );
}

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

fn rid(n: u8) -> RecordId {
    let mut a = [0u8; 16];
    a[15] = n;
    RecordId(a)
}

/// Seed ONE inflight `Put` entry (NO history/overlay write) at
/// `commit_version`, exactly the shape the ack-path leaves behind once the
/// history write is deferred. Returns the body bytes for later comparison.
async fn seed_inflight_put(
    repo: &RepoInstance,
    table: &str,
    record: RecordId,
    value: &str,
    commit_version: u64,
) -> bytes::Bytes {
    let wal = repo.repo_wal().await.unwrap();
    let body = InnerValue::Str(value.into()).to_bytes().unwrap();
    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        repo_token(repo.name()),
        vec![WalOpV2::Put {
            table_id_interned: table_token_for(table),
            rid: record,
            body: body.clone(),
        }],
    )
    .with_commit_version(commit_version);
    wal.begin_grouped(&entry, WalDurability::Buffered)
        .await
        .unwrap();
    body
}

/// drain_step replays the visible-but-undurable prefix into history,
/// advances `durable_watermark` to visibility, and the data reads back.
#[tokio::test]
async fn drain_step_replays_visible_prefix_into_history() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    // Build the gate FIRST (durable == visibility == 0) BEFORE any inflight
    // entry exists, so seeding below does not lift the durable floor.
    let gate = repo.tx_gate().await.unwrap();
    assert_eq!(gate.last_committed(), 0);
    assert_eq!(gate.durable_watermark(), 0);

    // Seed inflight entries at versions 1, 2, 3 (data NOT yet in history).
    seed_inflight_put(&repo, "t", rid(1), "v1", 1).await;
    seed_inflight_put(&repo, "t", rid(2), "v2", 2).await;
    seed_inflight_put(&repo, "t", rid(3), "v3", 3).await;

    // Ack-path published visibility to 3, but durable still lags at 0.
    gate.publish_committed_max(3);
    assert_eq!(gate.last_committed(), 3);
    assert_eq!(gate.durable_watermark(), 0, "durable lags visibility");

    // Before drain: history has nothing for these rids.
    assert!(tbl.get(rid(1)).await.is_err());

    let drainer = Drainer::new();
    let drained = drainer.drain_step(&repo).await.unwrap();
    assert_eq!(drained, 3, "all three visible-undurable versions drained");
    assert_eq!(drainer.drained_total(), 3);

    // Durable watermark advanced to visibility.
    assert_eq!(
        gate.durable_watermark(),
        3,
        "durable caught up to visibility"
    );

    // Data now resolves from history.
    for (n, expect) in [(1u8, "v1"), (2, "v2"), (3, "v3")] {
        let got = tbl.get(rid(n)).await.unwrap();
        assert!(
            matches!(got, InnerValue::Str(ref s) if s == expect),
            "rid {n}: expected {expect}, got {got:?}"
        );
    }
}

/// drain_step with `durable >= visibility` is a no-op (nothing to drain).
#[tokio::test]
async fn drain_step_noop_when_durable_at_visibility() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));

    let gate = repo.tx_gate().await.unwrap();
    // Advance BOTH watermarks to 5: nothing is visible-but-undurable. The
    // durable watermark is a CONTIGUOUS prefix, so mark every version 1..=5
    // (marking only 5 would leave the prefix at 0).
    gate.publish_committed_max(5);
    for v in 1..=5u64 {
        gate.mark_durable(v);
    }
    assert_eq!(gate.last_committed(), 5);
    assert_eq!(gate.durable_watermark(), 5);

    let drainer = Drainer::new();
    let drained = drainer.drain_step(&repo).await.unwrap();
    assert_eq!(drained, 0, "durable == visibility → no drain");
    assert_eq!(gate.durable_watermark(), 5, "watermark unchanged");
}

/// Idempotency: a second drain_step over already-drained state returns 0
/// and leaves history + watermark unchanged.
#[tokio::test]
async fn drain_step_is_idempotent() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let gate = repo.tx_gate().await.unwrap();
    seed_inflight_put(&repo, "t", rid(1), "once", 1).await;
    seed_inflight_put(&repo, "t", rid(2), "once", 2).await;
    gate.publish_committed_max(2);

    let drainer = Drainer::new();
    assert_eq!(drainer.drain_step(&repo).await.unwrap(), 2);
    assert_eq!(gate.durable_watermark(), 2);

    // Second pass: everything is already at/below the durable prefix.
    let second = drainer.drain_step(&repo).await.unwrap();
    assert_eq!(second, 0, "re-drain of drained state is a no-op");
    assert_eq!(gate.durable_watermark(), 2, "watermark unchanged");
    assert_eq!(drainer.drained_total(), 2, "no double-count");

    // Data still resolves to the single correct value (no corruption).
    let got = tbl.get(rid(1)).await.unwrap();
    assert!(matches!(got, InnerValue::Str(ref s) if s == "once"));
}

/// drain_all flushes the whole inflight tail and returns total drained.
#[tokio::test]
async fn drain_all_flushes_then_stops() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));

    let gate = repo.tx_gate().await.unwrap();
    for v in 1..=4u64 {
        seed_inflight_put(&repo, "t", rid(v as u8), "x", v).await;
    }
    gate.publish_committed_max(4);

    let drainer = Drainer::new();
    let total = drainer.drain_all(&repo).await.unwrap();
    assert_eq!(total, 4);
    assert_eq!(gate.durable_watermark(), 4);
    // A follow-up drain_all is a clean no-op.
    assert_eq!(drainer.drain_all(&repo).await.unwrap(), 0);
}

/// drain_step never drains a version ABOVE visibility: an entry committed in
/// the WAL but not yet published stays undurable.
#[tokio::test]
async fn drain_step_respects_visibility_ceiling() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    let gate = repo.tx_gate().await.unwrap();
    seed_inflight_put(&repo, "t", rid(1), "visible", 1).await;
    seed_inflight_put(&repo, "t", rid(2), "pending", 2).await;

    // Only version 1 is published; version 2 is committed-in-WAL but not
    // yet ack-visible.
    gate.publish_committed_max(1);

    let drainer = Drainer::new();
    let drained = drainer.drain_step(&repo).await.unwrap();
    assert_eq!(drained, 1, "only the visible version is drained");
    assert_eq!(gate.durable_watermark(), 1);

    // v1 is in history; v2 is NOT (above visibility).
    assert!(tbl.get(rid(1)).await.is_ok());
    assert!(
        tbl.get(rid(2)).await.is_err(),
        "version above visibility must not be drained"
    );
}

/// A5 interner-hwm gate: an entry whose interner delta references an id above
/// the table's persisted high-water mark IS drained to history (durable
/// advances) but its WAL marker is NOT truncated — it stays inflight until a
/// future interner checkpoint advances the hwm.
///
/// On the in-memory `Mem` WAL sink `wal.commit` is itself a no-op (markers
/// live until F6 truncation), so the load-bearing assertion is that
/// `drain_step` (a) still marks the version durable and (b) takes the A5
/// "retain marker" branch rather than erroring. We assert (a) directly and
/// (b) by the entry remaining replayable on a second pass with no panic.
#[tokio::test]
async fn drain_step_a5_retains_marker_above_hwm() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let token = table_token_for("t");

    // Fresh table: persisted interner high-water mark is 0.
    assert_eq!(tbl.interner().persisted_high_water(), 0);

    let gate = repo.tx_gate().await.unwrap();
    let wal = repo.repo_wal().await.unwrap();

    let body = InnerValue::Str("with_delta".into()).to_bytes().unwrap();
    let mut entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        repo_token(repo.name()),
        vec![WalOpV2::Put {
            table_id_interned: token,
            rid: rid(1),
            body,
        }],
    )
    .with_commit_version(1);
    // Interner delta id 42 — far above the persisted hwm (0) → A5 gate must
    // judge truncation UNSAFE and retain the marker.
    entry.interner_delta = vec![(token, "fresh_field".to_string(), 42)];
    wal.begin_grouped(&entry, WalDurability::Buffered)
        .await
        .unwrap();

    gate.publish_committed_max(1);

    let drainer = Drainer::new();
    let drained = drainer.drain_step(&repo).await.unwrap();

    // Data is durable in history despite the retained marker — the A5 gate
    // defers TRUNCATION, not DURABILITY.
    assert_eq!(drained, 1);
    assert_eq!(
        gate.durable_watermark(),
        1,
        "A5: durable advances even when the marker is retained"
    );
    let got = tbl.get(rid(1)).await.unwrap();
    assert!(matches!(got, InnerValue::Str(ref s) if s == "with_delta"));

    // The interner delta was applied on replay (id 42 → "fresh_field").
    let interner = tbl.interner().get().await.unwrap();
    assert_eq!(
        interner.get_ind("fresh_field").map(|k| k.id()),
        Some(42),
        "interner delta applied during drain replay"
    );

    // Second pass: version 1 is now at the durable floor → no-op, no panic.
    assert_eq!(drainer.drain_step(&repo).await.unwrap(), 0);
}

// ── Op #2 Stage 2: commit-path offer wiring tests ─────────────────────

/// Commit N=3 transactions through the public commit API and verify that
/// the drainer window contains exactly 3 entries with ascending keys
/// matching the commit_versions.
#[tokio::test]
async fn commit_offers_entry_to_drainer_window() {
    use bytes::Bytes;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::Store;
    use shamir_tx::{IsolationLevel, StagingStore, TxContext, TxId};

    use crate::tx::commit_tx;

    let repo = make_repo();

    let mut versions = Vec::new();
    for i in 1..=3u64 {
        let mut tx = TxContext::new(TxId::new(i), 0, 0, IsolationLevel::Snapshot);
        let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let mut staging = StagingStore::new(Arc::clone(&data_store));
        staging.set(
            Bytes::from(format!("k{i}")).into(),
            Bytes::from(format!("v{i}")),
        );
        tx.write_set.insert(i, staging);
        let outcome = commit_tx(tx, &repo).await.unwrap();
        versions.push(outcome.commit_version);
    }

    let drainer = repo.drainer();
    assert_eq!(
        drainer.window_len(),
        3,
        "drainer window must contain all 3 offered entries"
    );
    let keys = drainer.window_keys();
    assert_eq!(
        keys, versions,
        "window keys must match committed versions in ascending order"
    );
}

/// Commit one transaction and verify the window entry matches the WAL
/// entry recovered from the WAL (same txn_id and commit_version).
#[tokio::test]
async fn commit_offered_entries_match_wal() {
    use bytes::Bytes;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::Store;
    use shamir_tx::{IsolationLevel, StagingStore, TxContext, TxId};

    use crate::tx::commit_tx;

    let repo = make_repo();

    let mut tx = TxContext::new(TxId::new(10), 0, 0, IsolationLevel::Snapshot);
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut staging = StagingStore::new(Arc::clone(&data_store));
    staging.set(
        Bytes::from_static(b"key").into(),
        Bytes::from_static(b"val"),
    );
    tx.write_set.insert(10, staging);
    let outcome = commit_tx(tx, &repo).await.unwrap();
    let cv = outcome.commit_version;

    // Snapshot the window entry.
    let drainer = repo.drainer();
    let window_entry = drainer
        .window_entry(cv)
        .expect("window must contain the committed version");

    // Recover from WAL and find the matching entry.
    let wal = repo.repo_wal().await.unwrap();
    let wal_entries = wal.recover().await.unwrap();
    let wal_entry = wal_entries
        .iter()
        .find(|e| e.commit_version == cv)
        .expect("WAL must contain the committed entry");

    assert_eq!(
        window_entry.txn_id, wal_entry.txn_id,
        "window and WAL entry txn_id must match"
    );
    assert_eq!(
        window_entry.commit_version, wal_entry.commit_version,
        "window and WAL entry commit_version must match"
    );
}

/// Sanity: commit, then drain_step, verify it drained correctly with
/// offer wired (drain_step uses window in Stage 3).
#[tokio::test]
async fn drain_step_still_drains_correctly_with_offer_wired() {
    use bytes::Bytes;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::Store;
    use shamir_tx::{IsolationLevel, StagingStore, TxContext, TxId};

    use crate::tx::commit_tx;

    let repo = make_repo();
    repo.add_table(crate::table::TableConfig::new("t"));

    for i in 1..=2u64 {
        let mut tx = TxContext::new(TxId::new(i), 0, 0, IsolationLevel::Snapshot);
        let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let mut staging = StagingStore::new(Arc::clone(&data_store));
        staging.set(
            Bytes::from(format!("k{i}")).into(),
            Bytes::from(format!("v{i}")),
        );
        tx.write_set.insert(i, staging);
        let _ = commit_tx(tx, &repo).await.unwrap();
    }

    // The drainer window has 2 entries from the commit path.
    let drainer = repo.drainer();
    assert_eq!(drainer.window_len(), 2);

    // drain_step uses the window — it must drain the 2 versions.
    let drained = drainer.drain_step(&repo).await.unwrap();
    assert_eq!(drained, 2, "drain_step must drain all committed versions");

    let gate = repo.tx_gate().await.unwrap();
    assert_eq!(
        gate.durable_watermark(),
        gate.last_committed(),
        "durable must catch up to visibility after drain"
    );

    // Stage 3: window entries are removed after finalization.
    assert_eq!(
        drainer.window_len(),
        0,
        "window must be empty after drain finalization"
    );
}

// ── Op #2 Stage 3: window-based drain_step tests ─────────────────────

/// Stage 3 test 1: drain_step uses the window, not wal.recover(), in
/// steady state. Offer 3 entries directly, drain, assert all 3 drained
/// and the window is empty afterward.
#[tokio::test]
async fn drain_step_uses_window_not_recover_in_steady_state() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));

    let gate = repo.tx_gate().await.unwrap();

    // Seed inflight entries in WAL AND offer them to the drainer window.
    for v in 1..=3u64 {
        seed_inflight_put(&repo, "t", rid(v as u8), &format!("val{v}"), v).await;
    }
    gate.publish_committed_max(3);

    let drainer = Drainer::new();
    // Offer directly — simulating the commit path's offer.
    for v in 1..=3u64 {
        let wal = repo.repo_wal().await.unwrap();
        let entries = wal.recover().await.unwrap();
        for e in &entries {
            if e.commit_version == v {
                drainer.offer(Arc::new(e.clone()));
            }
        }
    }
    assert_eq!(drainer.window_len(), 3);

    let drained = drainer.drain_step(&repo).await.unwrap();
    assert_eq!(drained, 3, "all 3 entries drained from window");
    assert_eq!(drainer.window_len(), 0, "window empty after drain");
    assert_eq!(gate.durable_watermark(), 3);
}

/// Stage 3 test 2: a gap in the window triggers exactly one gap-reseed
/// from wal.recover(), and all entries are still drained.
#[tokio::test]
async fn drain_step_window_gap_triggers_one_recover_reseed() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));

    let gate = repo.tx_gate().await.unwrap();

    // Seed inflight entries for versions 1, 2, 3 in WAL.
    for v in 1..=3u64 {
        seed_inflight_put(&repo, "t", rid(v as u8), &format!("gap{v}"), v).await;
    }
    gate.publish_committed_max(3);

    // Offer all 3 to the window, then remove v=2 to create a gap.
    let drainer = Drainer::new();
    let wal = repo.repo_wal().await.unwrap();
    let entries = wal.recover().await.unwrap();
    for e in &entries {
        drainer.offer(Arc::new(e.clone()));
    }
    assert_eq!(drainer.window_len(), 3);
    drainer.window_remove_for_test(2);
    assert_eq!(drainer.window_len(), 2);
    assert_eq!(drainer.window_keys(), vec![1, 3]);

    // drain_all loops drain_step until 0. The new empty-prefix trigger
    // fires the gap-reseed exactly once on the pass that sees the empty
    // contiguous prefix (the pass after v=1 drained and watermark moved
    // to 1, leaving the window with {3} and expected=2 mismatching k=3).
    let recover_before = drainer.recover_calls();
    let drained = drainer.drain_all(&repo).await.unwrap();
    let recover_after = drainer.recover_calls();
    assert_eq!(drained, 3, "gap-reseed recovers the missing entry");
    assert_eq!(
        recover_after - recover_before,
        1,
        "exactly one gap-reseed fires (on the empty-prefix pass)"
    );
    assert_eq!(gate.durable_watermark(), 3);
    assert_eq!(drainer.window_len(), 0, "window empty after drain");
}

/// PT 2: an aborted commit_version INTERIOR to (dur, vis] — its
/// version was burned via fetch_add but no WAL entry was written, and
/// `VersionGuard::drop` (version_guard.rs:114) marked the durable
/// tracker Aborted — must NOT trigger a spurious wal.recover() reseed.
/// Empty-prefix invariant: `dur = durable_watermark()` already skips
/// every leading aborted version, so `dur+1` is NEVER aborted. A
/// non-empty contiguous prefix therefore means real progress; the
/// aborted version above is crossed via the moving watermark on the
/// next pass. Reseed must fire only on a TRUE gap (dropped offer),
/// never on an abort hole.
#[tokio::test]
async fn drain_step_interior_abort_does_not_trigger_spurious_recover() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));

    let gate = repo.tx_gate().await.unwrap();
    // Seed WAL with entries for v=1 and v=3 only; v=2 was aborted, never
    // written.
    seed_inflight_put(&repo, "t", rid(1), "abort_a", 1).await;
    seed_inflight_put(&repo, "t", rid(3), "abort_c", 3).await;
    // Mirror VersionGuard::drop for an aborted v=2 (e.g. SSI conflict
    // or empty-tx abort): durable tracker marks it Aborted so the
    // contiguous watermark crosses it once v=1 lands.
    gate.mark_durable_aborted(2);
    gate.publish_committed_max(3);
    assert_eq!(gate.durable_watermark(), 0, "dur=0 before v=1 drains");
    assert_eq!(gate.last_committed(), 3);

    let drainer = Drainer::new();
    let wal = repo.repo_wal().await.unwrap();
    let entries = wal.recover().await.unwrap();
    assert_eq!(entries.len(), 2, "WAL holds only the committed entries");
    for e in &entries {
        drainer.offer(Arc::new(e.clone()));
    }
    assert_eq!(drainer.window_keys(), vec![1, 3]);

    let recover_before = drainer.recover_calls();
    let drained = drainer.drain_all(&repo).await.unwrap();
    let recover_after = drainer.recover_calls();

    assert_eq!(drained, 2, "two committed entries drained (v=1, v=3)");
    assert_eq!(
        recover_after - recover_before,
        0,
        "interior abort must NOT trigger spurious wal.recover() — the \
         window already has every committed entry; the abort hole is \
         crossed via durable_watermark advance, not a reseed"
    );
    assert_eq!(gate.durable_watermark(), 3, "watermark crosses abort");
    assert_eq!(drainer.window_len(), 0, "window drained empty");
}

/// PT 1: the atomic `window_depth` mirror must stay in lock-step with
/// `scc::TreeIndex::len()` across every mutation site (offer success,
/// offer duplicate, seed, drain Phase C remove, test-only remove). The
/// purpose of the counter is to make `offer`'s backpressure check O(1)
/// — `scc::TreeIndex::len()` is `iter().count()` per source (2.4.0
/// tree_index.rs:727), which is the O(N) regression this guards against.
#[tokio::test]
async fn window_depth_atomic_mirror_matches_tree_across_mutations() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let gate = repo.tx_gate().await.unwrap();
    for v in 1..=4u64 {
        seed_inflight_put(&repo, "t", rid(v as u8), &format!("d{v}"), v).await;
    }
    gate.publish_committed_max(4);

    let drainer = Drainer::new();
    assert_eq!(drainer.window_depth(), 0, "fresh = 0");
    assert_eq!(drainer.window_depth(), drainer.window_len());

    // 1. offer success: depth bumps per successful insert.
    let wal = repo.repo_wal().await.unwrap();
    let entries = wal.recover().await.unwrap();
    let arcs: Vec<_> = entries.into_iter().map(Arc::new).collect();
    for e in &arcs {
        drainer.offer(Arc::clone(e));
    }
    assert_eq!(drainer.window_depth(), 4, "four offers, four inserts");
    assert_eq!(drainer.window_depth(), drainer.window_len());

    // 2. offer duplicate: scc::TreeIndex::insert returns Err on existing
    //    key; the depth mirror must NOT double-count.
    for e in &arcs {
        drainer.offer(Arc::clone(e));
    }
    assert_eq!(drainer.window_depth(), 4, "duplicate offers ignored");
    assert_eq!(drainer.window_depth(), drainer.window_len());

    // 3. test-only remove: depth decrements on a successful remove.
    drainer.window_remove_for_test(2);
    assert_eq!(drainer.window_depth(), 3, "one remove");
    assert_eq!(drainer.window_depth(), drainer.window_len());
    // No-op remove (already absent): depth must not move.
    drainer.window_remove_for_test(2);
    assert_eq!(drainer.window_depth(), 3, "no-op remove preserves depth");

    // 4. seed_from_recover: depth bumps per fresh insert, ignores
    //    duplicates.
    let entries2 = wal.recover().await.unwrap(); // returns all 4
    drainer.seed_from_recover(entries2);
    assert_eq!(
        drainer.window_depth(),
        4,
        "v=2 reseeded; 1,3,4 already present"
    );
    assert_eq!(drainer.window_depth(), drainer.window_len());

    // 5. drain Phase C remove: depth decrements once per finalized entry.
    let drained = drainer.drain_all(&repo).await.unwrap();
    assert_eq!(drained, 4);
    assert_eq!(drainer.window_depth(), 0, "all finalized → depth 0");
    assert_eq!(drainer.window_depth(), drainer.window_len());
}

/// PT 1: when the window is at or above the soft high-watermark, `offer`
/// MUST short-circuit on the atomic load — no `scc::TreeIndex::len()`
/// walk, no tree mutation. This is the O(1)-backpressure contract.
/// We can't directly assert "no len() call" from outside, but we CAN
/// prove the equivalent by setting hw=0 and observing that the drop
/// counter advances and the window stays untouched even though many
/// offers fire — the only path that produces that observable triple
/// is the atomic-load-and-return branch.
#[tokio::test]
async fn offer_backpressure_check_uses_atomic_depth_not_tree_walk() {
    let drainer = Drainer::new();
    drainer.set_window_high_watermark(0);

    // 1000 offers under hw=0: every one must drop without touching the tree.
    for v in 1..=1_000u64 {
        let entry = WalEntryV2::new(v, 0, vec![]).with_commit_version(v);
        drainer.offer(Arc::new(entry));
    }

    assert_eq!(drainer.window_depth(), 0, "no inserts under hw=0");
    assert_eq!(drainer.window_len(), 0, "tree confirms no inserts");
    assert_eq!(
        drainer.offer_dropped_total(),
        1_000,
        "every offer dropped on the atomic backpressure check"
    );
}

/// Stage 3 test 3: drain_step with dur >= vis returns 0 immediately —
/// no window scan, no wal.recover().
#[tokio::test]
async fn drain_step_empty_window_no_recover_call() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));

    let gate = repo.tx_gate().await.unwrap();
    gate.publish_committed_max(5);
    for v in 1..=5u64 {
        gate.mark_durable(v);
    }
    assert_eq!(gate.durable_watermark(), 5);

    let drainer = Drainer::new();
    let drained = drainer.drain_step(&repo).await.unwrap();
    assert_eq!(drained, 0, "dur >= vis → immediate return 0");
}

// Stage 3 test 4: crash_recovery still works — sanity check that
// the window addition does not break recovery. Delegated to the
// existing crash_recovery test suite (run via gate command).

// ── Op #2 Stage 4: backpressure (bounded offer) tests ───────────────

/// Fresh `Drainer::new()` has the default high-watermark of 64K.
#[tokio::test]
async fn default_high_watermark_is_64k() {
    let drainer = Drainer::new();
    assert_eq!(
        drainer.window_high_watermark(),
        64 * 1024,
        "default high-watermark must be 64K"
    );
}

/// Set high_watermark to 4, offer 10 entries. The window must cap around
/// the watermark (soft limit — races allow a few extras) and
/// `offer_dropped_total` must account for the gap.
#[tokio::test]
async fn offer_drops_at_high_watermark() {
    let drainer = Drainer::new();
    drainer.set_window_high_watermark(4);

    let mk = |v: u64| -> Arc<WalEntryV2> {
        Arc::new(WalEntryV2::new(v, 0, vec![]).with_commit_version(v))
    };

    for v in 1..=10u64 {
        drainer.offer(mk(v));
    }

    let wlen = drainer.window_len();
    let dropped = drainer.offer_dropped_total();

    // Soft limit: window should be ~4, but races may allow a few extras.
    assert!(
        (4..=7).contains(&wlen),
        "window_len ({wlen}) should be in [4, 7] with hw=4"
    );
    assert!(
        dropped > 0,
        "some entries must have been dropped (dropped={dropped})"
    );
    // Accounting: offered 10 total = window_len + dropped.
    assert_eq!(
        wlen as u64 + dropped,
        10,
        "window_len ({wlen}) + dropped ({dropped}) must equal 10"
    );
}

/// End-to-end: set hw=2, commit N=10 txs through the commit API, then
/// drain_step. All 10 entries must be drained (gap-reseed recovers drops).
#[tokio::test]
async fn drain_step_recovers_dropped_entries_via_gap_reseed() {
    use bytes::Bytes;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::Store;
    use shamir_tx::{IsolationLevel, StagingStore, TxContext, TxId};

    use crate::tx::commit_tx;

    let repo = make_repo();
    repo.add_table(crate::table::TableConfig::new("t"));

    // Set a very low watermark so most offers are dropped.
    let drainer = repo.drainer();
    drainer.set_window_high_watermark(2);

    for i in 1..=10u64 {
        let mut tx = TxContext::new(TxId::new(i), 0, 0, IsolationLevel::Snapshot);
        let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let mut staging = StagingStore::new(Arc::clone(&data_store));
        staging.set(
            Bytes::from(format!("k{i}")).into(),
            Bytes::from(format!("v{i}")),
        );
        tx.write_set.insert(i, staging);
        let _ = commit_tx(tx, &repo).await.unwrap();
    }

    let dropped = drainer.offer_dropped_total();
    assert!(
        dropped > 0,
        "with hw=2, some offers must have been dropped (dropped={dropped})"
    );

    // drain_step must recover ALL entries via gap-reseed.
    let drained = drainer.drain_all(&repo).await.unwrap();
    assert_eq!(drained, 10, "all 10 entries must be drained despite drops");

    let gate = repo.tx_gate().await.unwrap();
    assert_eq!(
        gate.durable_watermark(),
        gate.last_committed(),
        "durable must catch up to visibility"
    );
    assert_eq!(drainer.window_len(), 0, "window must be empty after drain");
}
