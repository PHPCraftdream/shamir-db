//! A14 race-reproduction tests: two independent drain paths (background
//! drainer's batched path + a forced `flush_buffers`/`drain_to_history`-style
//! single-entry drain) can BOTH try to consume the SAME committed version's
//! `pending_ts` stamp. Pre-fix, the FIRST caller to reach
//! `pending_ts.remove(&v)` won the stamp and the SECOND caller found `None`
//! and fell back to `now_millis()` — writing a WRONG (drain-time, not
//! commit-time) timestamp into durable history for that version.
//!
//! These tests reproduce the race DETERMINISTICALLY by calling both drain
//! entry-points in sequence for the SAME version (no real concurrency needed
//! — the destructive `remove` is what loses the stamp, not the scheduler).

use bytes::Bytes;
use futures::StreamExt;

use crate::mvcc_store::{decode_ts_key, ts_key, MvccStore};
use crate::repo_tx_gate::RepoTxGate;
use crate::version_codec::decode_version_key;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::KvOp;
use std::sync::Arc;

/// Build a test MvccStore backed by an in-memory history store + shared gate.
fn make_mvcc() -> (MvccStore, Arc<RepoTxGate>) {
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = MvccStore::new(Arc::new(InMemoryStore::new()), gate.clone());
    (mvcc, gate)
}

/// Read the durable commit-ts recorded for `version` in the history store.
/// Returns `None` if no ts-key entry exists for that version.
async fn durable_ts_of(mvcc: &MvccStore, version: u64) -> Option<u64> {
    let raw = mvcc.history_store().get(ts_key(version)).await.ok()?;
    if raw.len() != 8 {
        return None;
    }
    let bytes: [u8; 8] = raw.as_ref().try_into().ok()?;
    Some(u64::from_le_bytes(bytes))
}

/// Tally ts-key entries from a full history scan (unused by the race tests
/// but kept for parity with the sibling ts test modules).
#[allow(dead_code)]
async fn scan_ts_entries(mvcc: &MvccStore) -> Vec<(u64, u64)> {
    let stream = mvcc.history_store().iter_stream(64);
    futures::pin_mut!(stream);
    let mut out = Vec::new();
    while let Some(batch) = stream.next().await {
        for (phys_key, val) in batch.unwrap() {
            if let Some(version) = decode_ts_key(&phys_key) {
                let ts_bytes: [u8; 8] = val.as_ref().try_into().unwrap();
                let ts_ms = u64::from_le_bytes(ts_bytes);
                out.push((version, ts_ms));
            }
        }
    }
    out
}

// =========================================================================
// 1. CORE RACE: batched-drain THEN single-entry-drain for the SAME version.
//
// Simulates the background drainer (`write_committed_batch_to_history`) and
// a concurrent forced flush (`drain_to_history` → `write_committed_to_history`)
// both reaching the SAME commit_version. The FIRST call (batched) consumes
// the stamp; the SECOND (single-entry) pre-fix fell back to `now_millis()`
// and OVERWROTE the correct commit-time ts with a drain-time ts.
// =========================================================================
#[tokio::test]
async fn race_batch_then_single_keeps_commit_ts() {
    let (mvcc, gate) = make_mvcc();

    // Commit-time clock — the value that MUST end up durable.
    let commit_ts: u64 = 1_700_000_000_000;
    mvcc.set_test_now(commit_ts);

    let v = gate.assign_next_version();
    let ops = vec![KvOp::Set(
        Bytes::from_static(b"rk"),
        Bytes::from_static(b"rv"),
    )];

    // Ack-path: stamps pending_ts[v] = commit_ts.
    mvcc.apply_committed_visible(&ops, v);

    // Drain-time clock — distinct from commit time so a fallback is detectable.
    let drain_ts: u64 = commit_ts + 60_000;
    mvcc.set_test_now(drain_ts);

    // FIRST drain path: the background drainer's batched path.
    let pass: Vec<(u64, Vec<KvOp>)> = vec![(v, ops.clone())];
    mvcc.write_committed_batch_to_history(&pass).await.unwrap();

    // SECOND drain path: the forced-flush / rename single-entry path,
    // hitting the SAME version. Pre-fix this overwrote ts_key(v) with drain_ts.
    mvcc.write_committed_to_history(&ops, v).await.unwrap();

    // The durable ts MUST still be the commit-time stamp, NOT drain_ts.
    let durable = durable_ts_of(&mvcc, v).await.expect("ts entry must exist");
    assert_eq!(
        durable, commit_ts,
        "A14: second drain must NOT overwrite commit-time ts with drain-time ts"
    );
    assert_ne!(
        durable, drain_ts,
        "regression guard: durable ts must not be the drain-time fallback"
    );
}

// =========================================================================
// 2. CORE RACE (reverse order): single-entry-drain THEN batched-drain.
//
// The race is symmetric — whichever caller runs SECOND is the one that
// loses the stamp. Cover both orderings.
// =========================================================================
#[tokio::test]
async fn race_single_then_batch_keeps_commit_ts() {
    let (mvcc, gate) = make_mvcc();

    let commit_ts: u64 = 1_700_000_000_000;
    mvcc.set_test_now(commit_ts);

    let v = gate.assign_next_version();
    let ops = vec![KvOp::Set(
        Bytes::from_static(b"rk2"),
        Bytes::from_static(b"rv2"),
    )];

    mvcc.apply_committed_visible(&ops, v);

    let drain_ts: u64 = commit_ts + 120_000;
    mvcc.set_test_now(drain_ts);

    // FIRST drain path this time: single-entry (forced flush / rename).
    mvcc.write_committed_to_history(&ops, v).await.unwrap();

    // SECOND drain path: the batched background-drainer path.
    let pass: Vec<(u64, Vec<KvOp>)> = vec![(v, ops.clone())];
    mvcc.write_committed_batch_to_history(&pass).await.unwrap();

    let durable = durable_ts_of(&mvcc, v).await.expect("ts entry must exist");
    assert_eq!(
        durable, commit_ts,
        "A14: batched drain after single drain must preserve commit-time ts"
    );
}

// =========================================================================
// 3. COLD-RECOVERY FALLBACK: a version whose pending_ts was NEVER stamped
// (genuine cold start — recovery replayed the WAL without an ack-path)
// still correctly falls back to now_millis(). This must NOT regress into
// "always require a stamp or panic."
// =========================================================================
#[tokio::test]
async fn cold_recovery_unstamped_version_falls_back_to_now() {
    let (mvcc, gate) = make_mvcc();

    // NO apply_committed_visible call — simulate cold recovery: pending_ts
    // was never stamped for this version.
    let recovery_ts: u64 = 1_700_000_500_000;
    mvcc.set_test_now(recovery_ts);

    let v = gate.assign_next_version();
    let ops = vec![KvOp::Set(
        Bytes::from_static(b"ck"),
        Bytes::from_static(b"cv"),
    )];

    // Drain directly — no stamp present.
    mvcc.write_committed_to_history(&ops, v).await.unwrap();

    // Falls back to now_millis() — the conservative recovery behaviour.
    let durable = durable_ts_of(&mvcc, v).await.expect("ts entry must exist");
    assert_eq!(
        durable, recovery_ts,
        "cold-recovery unstamped version must fall back to now_millis()"
    );

    // And the same holds for the batched path.
    let v2 = gate.assign_next_version();
    let ops2 = vec![KvOp::Set(
        Bytes::from_static(b"ck2"),
        Bytes::from_static(b"cv2"),
    )];
    let recovery_ts2: u64 = recovery_ts + 5_000;
    mvcc.set_test_now(recovery_ts2);
    let pass: Vec<(u64, Vec<KvOp>)> = vec![(v2, ops2.clone())];
    mvcc.write_committed_batch_to_history(&pass).await.unwrap();
    let durable2 = durable_ts_of(&mvcc, v2).await.expect("ts entry must exist");
    assert_eq!(
        durable2, recovery_ts2,
        "cold-recovery unstamped version (batched) must fall back to now_millis()"
    );
}

// =========================================================================
// 4. SINGLE-CALLER HYGIENE: a version drained by exactly ONE path still
// resolves its pending_ts correctly, and after a gc_overlay_to sweep the
// stamp is reclaimed (does not leak forever).
// =========================================================================
#[tokio::test]
async fn single_drain_consumes_then_gc_reclaims_stamp() {
    let (mvcc, gate) = make_mvcc();

    let commit_ts: u64 = 1_700_001_000_000;
    mvcc.set_test_now(commit_ts);

    let v = gate.assign_next_version();
    let ops = vec![KvOp::Set(
        Bytes::from_static(b"sk"),
        Bytes::from_static(b"sv"),
    )];
    mvcc.apply_committed_visible(&ops, v);

    // Exactly one drain path.
    mvcc.write_committed_to_history(&ops, v).await.unwrap();

    // The durable ts is the commit-time stamp.
    let durable = durable_ts_of(&mvcc, v).await.expect("ts entry must exist");
    assert_eq!(durable, commit_ts);

    // After the version is marked durable and gc_overlay_to runs, the
    // pending_ts entry for v is reclaimed (no unbounded leak).
    mvcc.gate.mark_durable(v);
    mvcc.gc_overlay_to(v);
    assert_eq!(
        mvcc.pending_ts_len(),
        0,
        "pending_ts stamp must be reclaimed by gc_overlay_to after drain"
    );
}

// =========================================================================
// 5. RACE + GC HYGIENE: after both racers run for the same version, the
// stamp is still eventually reclaimed by gc_overlay_to (the non-destructive
// read must not leave a permanent resident behind).
// =========================================================================
#[tokio::test]
async fn race_then_gc_reclaims_stamp() {
    let (mvcc, gate) = make_mvcc();

    let commit_ts: u64 = 1_700_002_000_000;
    mvcc.set_test_now(commit_ts);

    let v = gate.assign_next_version();
    let ops = vec![KvOp::Set(
        Bytes::from_static(b"gk"),
        Bytes::from_static(b"gv"),
    )];
    mvcc.apply_committed_visible(&ops, v);

    mvcc.set_test_now(commit_ts + 99_000);

    // Both racers.
    mvcc.write_committed_to_history(&ops, v).await.unwrap();
    let pass: Vec<(u64, Vec<KvOp>)> = vec![(v, ops.clone())];
    mvcc.write_committed_batch_to_history(&pass).await.unwrap();

    // Pre-fix the destructive remove would have already cleared the entry
    // on the first call; post-fix (non-destructive read) the entry survives
    // both calls and is reclaimed ONLY by gc_overlay_to. Verify that path.
    mvcc.gate.mark_durable(v);
    mvcc.gc_overlay_to(v);
    assert_eq!(
        mvcc.pending_ts_len(),
        0,
        "pending_ts stamp must be reclaimed after race + gc"
    );

    // And the durable ts is still correct.
    let durable = durable_ts_of(&mvcc, v).await.expect("ts entry must exist");
    assert_eq!(durable, commit_ts);
}

// =========================================================================
// 6. Unused helper guard: keep decode_version_key import live for parity
// with the sibling test modules (scan_history-style helpers).
// =========================================================================
#[test]
fn helpers_compile() {
    // Smoke: the codec round-trips a (key, version) pair.
    let key = b"zk";
    let v: u64 = 42;
    let enc = crate::version_codec::encode_version_key(key, v);
    let (k, dec) = decode_version_key(&enc).expect("decode");
    assert_eq!(k, key);
    assert_eq!(dec, v);
}
