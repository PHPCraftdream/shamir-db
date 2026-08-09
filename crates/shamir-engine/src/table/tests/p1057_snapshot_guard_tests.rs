//! #1057 — Snapshot guard for Phase A online CREATE INDEX.
//!
//! Tests that verify:
//! 1. The guard returned by `TableManager::open_index_build_snapshot` pins
//!    an MVCC version and prevents GC from reclaiming it (anti-GC test).
//! 2. After the guard drops, `min_alive()` recovers to track the live watermark
//!    again (release test).

use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_tx::{MvccStore, RepoTxGate, Retention};
use std::sync::Arc;

use crate::table::TableManager;

/// Helper: create a TableManager with MVCC and changefeed attached.
///
/// This mirrors the production pattern in `RepoInstance::create_table_context`:
/// both `mvcc_store` and `changefeed` are attached using the SAME `gate` instance.
async fn make_table_with_mvcc_and_changefeed() -> (TableManager, Arc<MvccStore>, Arc<RepoTxGate>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    // Keep full history so versions survive GC.
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let tbl = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let tbl = tbl
        .with_mvcc_store(Arc::clone(&mvcc))
        .with_changefeed(Arc::clone(&gate));

    (tbl, mvcc, gate)
}

/// Collect a stream into a BTreeMap for easy comparison.
async fn collect_stream_sorted(
    stream: impl futures::Stream<Item = shamir_storage::error::DbResult<Vec<(Bytes, Bytes)>>> + Unpin,
) -> std::collections::BTreeMap<Vec<u8>, Bytes> {
    let mut out = std::collections::BTreeMap::new();
    let mut stream = std::pin::pin!(stream);
    while let Some(batch) = stream.next().await {
        for (k, v) in batch.unwrap() {
            out.insert(k.to_vec(), v);
        }
    }
    out
}

/// Anti-GC test: the guard pins min_alive(), preventing GC from advancing.
///
/// This test verifies that `open_index_build_snapshot` returns a guard that
/// registers a snapshot in `gate.active_snapshots`, which `min_alive()` reads
/// to compute the GC floor. While the guard is held, `min_alive()` stays at
/// the pinned version; after it drops, `min_alive()` recovers to
/// `last_committed()`.
///
/// This is the core mechanism: GC calls `gc_below(gate.min_alive())`, and
/// `min_alive()` returns the oldest live snapshot (or `last_committed()` if
/// none). The guard keeps a snapshot live.
#[tokio::test]
async fn anti_gc_snapshot_guard_pins_min_alive() {
    let (tbl, _mvcc, gate) = make_table_with_mvcc_and_changefeed().await;

    // Write some data to advance last_committed.
    for i in 0..3 {
        gate.publish_committed_max(i as u64 + 1);
    }

    // No guard held: min_alive() tracks last_committed().
    assert_eq!(gate.last_committed(), 3);
    assert_eq!(
        gate.min_alive(),
        3,
        "no guard, min_alive should track last_committed"
    );

    // Acquire the guard — this should pin min_alive().
    let guard = tbl
        .open_index_build_snapshot()
        .await
        .expect("changefeed attached, should return Some");
    let pin = guard.version();
    assert_eq!(pin, 3, "should pin at version 3");

    // Advance last_committed past the pin.
    gate.publish_committed_max(10);
    assert_eq!(gate.last_committed(), 10);

    // min_alive should STILL be pinned at the guard's version, not last_committed.
    assert_eq!(
        gate.min_alive(),
        pin,
        "guard should keep min_alive at pinned version despite last_committed advancing"
    );

    // Write more data to advance further.
    gate.publish_committed_max(20);
    assert_eq!(gate.last_committed(), 20);
    assert_eq!(
        gate.min_alive(),
        pin,
        "guard should still pin min_alive despite further advancement"
    );

    // Guard is dropped here — min_alive should recover.
    drop(guard);

    // After guard drop, min_alive() should recover to last_committed().
    assert_eq!(
        gate.min_alive(),
        20,
        "after guard drop, min_alive should track last_committed"
    );
}

/// Anti-GC test failure verification: WITHOUT the guard, min_alive() tracks last_committed().
///
/// This test verifies that the guard in `anti_gc_snapshot_guard_pins_min_alive`
/// is actually doing something. Without a guard, min_alive() immediately
/// tracks last_committed() as it advances, which would allow GC to reclaim
/// versions that a long-running scan still needs.
#[tokio::test]
async fn anti_gc_fails_without_guard() {
    let (_tbl, _mvcc, gate) = make_table_with_mvcc_and_changefeed().await;

    // Write some data.
    for i in 0..3 {
        gate.publish_committed_max(i as u64 + 1);
    }

    // Capture the pin WITHOUT holding a guard.
    let pin = gate.last_committed();
    assert_eq!(pin, 3);

    // Advance last_committed past the pin.
    gate.publish_committed_max(10);
    assert_eq!(gate.last_committed(), 10);

    // WITHOUT a guard, min_alive() should immediately track last_committed().
    // This is the hazard: a scan that thought it was "pinned" at version 3
    // would actually be vulnerable because min_alive() moved to 10, allowing
    // GC to reclaim versions in (3, 10] that the scan might need.
    assert_eq!(
        gate.min_alive(),
        10,
        "WITHOUT a guard, min_alive should track last_committed (10), not stay at pin (3)"
    );

    // Verify we can't accidentally "keep" the pin by reading last_committed.
    // The pin is just a value; it doesn't register in active_snapshots.
    assert_ne!(
        gate.min_alive(),
        pin,
        "pin value alone should NOT keep min_alive at 3"
    );
}

/// Release test: after the guard drops, min_alive() recovers.
///
/// Verifies that the guard doesn't permanently pin the watermark — once
/// dropped, GC can advance again.
#[tokio::test]
async fn release_guard_allows_gc_to_resume() {
    let (tbl, mvcc, gate) = make_table_with_mvcc_and_changefeed().await;

    // Write 1 row.
    let k = Bytes::from("key");
    let v = Bytes::from("v1");
    mvcc.set_versioned(RecordKey::from(k.clone()), v.clone())
        .await
        .unwrap();
    gate.publish_committed_max(1);

    // Acquire the guard, verify min_alive() is pinned.
    {
        let guard = tbl
            .open_index_build_snapshot()
            .await
            .expect("changefeed attached, should return Some");
        let pin = guard.version();
        assert_eq!(pin, 1);
        assert_eq!(
            gate.min_alive(),
            1,
            "guard should pin min_alive at version 1"
        );

        // Write 2 more versions.
        for i in 2..4 {
            let k = Bytes::from(format!("key{i}"));
            let v = Bytes::from(format!("v{i}"));
            mvcc.set_versioned(RecordKey::from(k.clone()), v.clone())
                .await
                .unwrap();
            gate.publish_committed_max(i as u64);
        }

        // min_alive should still be pinned at 1 while guard is held.
        assert_eq!(
            gate.min_alive(),
            1,
            "guard should still pin min_alive at 1 despite new writes"
        );

        // Guard drops here.
    }

    // After guard drops, min_alive() should recover to last_committed().
    assert_eq!(gate.last_committed(), 3, "should have 3 committed versions");
    assert_eq!(
        gate.min_alive(),
        3,
        "after guard drop, min_alive should recover to last_committed"
    );

    // Now GC can reclaim below 3.
    let deleted = mvcc.gc_below(3).await.unwrap();
    // We wrote 3 keys, none have old history, so deleted should be 0.
    // The important thing is that gc_below(3) was CALLED and didn't
    // abort due to a stuck watermark.
    assert!(
        deleted == 0,
        "no old history to delete, but GC should have run without error"
    );
}

/// End-to-end test: pinned version survives a real GC pass with guard held,
/// and is reclaimed after guard drops.
///
/// This is the core data-survival proof, not just a `min_alive()` proxy test.
/// The mechanism:
/// - Write v1, v2, v3, v4, v5 for the same key (all 5 exist in history).
/// - Acquire guard after v2 (pin = v2, NOT the latest write).
/// - Write v3, v4, v5 AFTER guard is acquired (exactly the online-build scenario).
/// - Call `gc_below(min_alive())` with guard held → v2 survives as anchor.
/// - Read back via `snapshot_stream(pin)` → gets v2's value, not v3/v4/v5's.
/// - Drop guard, call `gc_below(min_alive())` again → v2 is reclaimed (anchor
///   becomes v4), proving the guard was load-bearing for data survival.
#[tokio::test]
async fn pinned_version_survives_real_gc_pass_with_guard_held() {
    let (tbl, mvcc, gate) = make_table_with_mvcc_and_changefeed().await;

    let k = Bytes::from("key");
    let v1 = Bytes::from("value_v1");
    let v2 = Bytes::from("value_v2");
    let v3 = Bytes::from("value_v3");
    let v4 = Bytes::from("value_v4");
    let v5 = Bytes::from("value_v5");

    // Write v1.
    mvcc.set_versioned(RecordKey::from(k.clone()), v1.clone())
        .await
        .unwrap();
    gate.publish_committed_max(1);

    // Write v2 (this will be our pin).
    mvcc.set_versioned(RecordKey::from(k.clone()), v2.clone())
        .await
        .unwrap();
    gate.publish_committed_max(2);

    // Acquire the guard AFTER v2, BEFORE v3.
    let guard = tbl
        .open_index_build_snapshot()
        .await
        .expect("changefeed attached, should return Some");
    let pin = guard.version();
    assert_eq!(pin, 2, "should pin at version 2 (v2)");

    // Write v3, v4, v5 AFTER the guard is acquired.
    // This is the exact online-build scenario: Phase A pins at version V,
    // then more writes land while Phase A is scanning.
    mvcc.set_versioned(RecordKey::from(k.clone()), v3.clone())
        .await
        .unwrap();
    gate.publish_committed_max(3);

    mvcc.set_versioned(RecordKey::from(k.clone()), v4.clone())
        .await
        .unwrap();
    gate.publish_committed_max(4);

    mvcc.set_versioned(RecordKey::from(k.clone()), v5.clone())
        .await
        .unwrap();
    gate.publish_committed_max(5);

    // Verify the guard keeps min_alive at the pin, not the latest write.
    assert_eq!(
        gate.min_alive(),
        2,
        "guard should keep min_alive at pin (2), not latest commit (5)"
    );
    assert_eq!(gate.last_committed(), 5);

    // STEP 1: GC with guard held.
    // With guard held, min_alive() = 2, so gc_below(2) should NOT delete v2.
    // v1 might be reclaimed as the non-anchor below the pin — that's fine.
    // What matters: v2 SURVIVES.
    let min_alive_guarded = gate.min_alive();
    assert_eq!(min_alive_guarded, 2);
    let _deleted = mvcc.gc_below(min_alive_guarded).await.unwrap();

    // Read back via snapshot_stream at the pin.
    // Should get v2's value, NOT v3's (which is beyond the pin).
    let stream = mvcc.snapshot_stream(64, pin);
    let result = collect_stream_sorted(stream).await;

    assert_eq!(result.len(), 1, "should see exactly 1 key at pin");
    assert_eq!(
        result.get(k.as_ref()),
        Some(&v2),
        "should see v2's value (the pinned version), not v3's"
    );
    assert_ne!(
        result.get(k.as_ref()),
        Some(&v3),
        "should NOT see v3's value (it's beyond the pin)"
    );

    // STEP 2: Drop the guard, then GC again.
    // Now min_alive() tracks last_committed() = 5, so gc_below(5)'s anchor
    // for the key becomes v4, and v2 is no longer protected.
    drop(guard);
    assert_eq!(
        gate.min_alive(),
        5,
        "after guard drop, min_alive should track last_committed"
    );

    let min_alive_released = gate.min_alive();
    assert_eq!(min_alive_released, 5);
    let _deleted = mvcc.gc_below(min_alive_released).await.unwrap();

    // Verify v2 is now gone by reading at pin = 2.
    // With anchor = v4 (latest version < 5) and v2 deleted, snapshot_stream(2) should return empty.
    // The anchor is v4, which is > pin (2), so nothing is visible at version 2.
    let stream = mvcc.snapshot_stream(64, pin);
    let result_after_release = collect_stream_sorted(stream).await;

    assert!(
        result_after_release.is_empty(),
        "after guard drop and GC, v2 should be gone (anchor is v4, which is > pin 2)"
    );
}
