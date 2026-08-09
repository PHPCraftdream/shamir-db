//! Tests for `MvccStore::snapshot_stream(batch, at_version)`.
//!
//! This is the version-pinned scan primitive for online CREATE INDEX's Phase A
//! backfill (RFC v2 §2.2, slice 1).

use super::helpers::{make_gate, make_mvcc_with_gate};
use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_storage::types::RecordKey;
use std::collections::BTreeMap;

/// Collect a stream into a sorted BTreeMap for easy comparison.
async fn collect_stream_sorted(
    stream: impl futures::Stream<Item = DbResult<Vec<(Bytes, Bytes)>>>,
) -> BTreeMap<Vec<u8>, Bytes> {
    let mut out = BTreeMap::new();
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        for (k, v) in batch.unwrap() {
            out.insert(k.to_vec(), v);
        }
    }
    out
}

/// Test 1: Pin excludes post-pin writes.
///
/// Write N rows, capture `gate.last_committed()` as the pin, write M more rows,
/// call `snapshot_stream(batch, pin)` → exactly N rows returned, not N+M.
#[tokio::test]
async fn snapshot_stream_excludes_post_pin_writes() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    // Write 3 initial rows (N=3).
    for i in 0..3 {
        let k = Bytes::from(format!("key{i}"));
        let v = Bytes::from(format!("v{i}_initial"));
        mvcc.set_versioned(RecordKey::from(k.clone()), v.clone())
            .await
            .unwrap();
        gate.publish_committed_max(i as u64 + 1);
    }

    // Capture the pin: last_committed after initial writes.
    let pin = gate.last_committed();
    assert_eq!(pin, 3, "should have pinned at version 3");

    // Write 2 more rows (M=2) AFTER the pin.
    for i in 3..5 {
        let k = Bytes::from(format!("key{i}"));
        let v = Bytes::from(format!("v{i}_post_pin"));
        mvcc.set_versioned(RecordKey::from(k.clone()), v.clone())
            .await
            .unwrap();
        gate.publish_committed_max(i as u64 + 1);
    }

    // Stream at the pin: should see only the first 3 rows.
    let stream = mvcc.snapshot_stream(64, pin);
    let result = collect_stream_sorted(stream).await;

    assert_eq!(result.len(), 3, "should see exactly N=3 rows, not N+M=5");
    for i in 0..3 {
        let k = format!("key{i}");
        let expected_v = Bytes::from(format!("v{i}_initial"));
        assert_eq!(
            result.get(k.as_bytes()),
            Some(&expected_v),
            "should see initial value for {k}"
        );
    }

    // Post-pin rows should NOT appear.
    assert!(
        !result.contains_key(&b"key3"[..]),
        "key3 (post-pin) should not appear"
    );
    assert!(
        !result.contains_key(&b"key4"[..]),
        "key4 (post-pin) should not appear"
    );
}

/// Test 2: Pin sees the value AS OF the pin, not current.
///
/// Write a row v1, capture the pin, update the row to v2,
/// `snapshot_stream(batch, pin)` → returns v1's bytes, not v2's.
#[tokio::test]
async fn snapshot_stream_sees_value_as_of_pin() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let k = Bytes::from("key");
    let v1 = Bytes::from("value_v1");
    let v2 = Bytes::from("value_v2");

    // Write v1 and publish.
    mvcc.set_versioned(RecordKey::from(k.clone()), v1.clone())
        .await
        .unwrap();
    gate.publish_committed_max(1);

    // Capture the pin.
    let pin = gate.last_committed();
    assert_eq!(pin, 1);

    // Update to v2 and publish.
    mvcc.set_versioned(RecordKey::from(k.clone()), v2.clone())
        .await
        .unwrap();
    gate.publish_committed_max(2);

    // Stream at the pin: should see v1, NOT v2.
    let stream = mvcc.snapshot_stream(64, pin);
    let result = collect_stream_sorted(stream).await;

    assert_eq!(result.len(), 1, "should see exactly one key");
    assert_eq!(
        result.get(k.as_ref()),
        Some(&v1),
        "should see v1 (as-of pin), NOT v2 (current)"
    );
    assert_ne!(
        result.get(k.as_ref()),
        Some(&v2),
        "should NOT see v2 (current) when pinned at v1"
    );
}

/// Test 3: Equivalence with `current_stream`.
///
/// On the same fixture, `snapshot_stream(b, gate.last_committed())` produces
/// byte-identical output to `current_stream(b)` — same rows, same order, same values.
#[tokio::test]
async fn snapshot_stream_equivalence_with_current_stream() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    // Write a mix of data: some keys updated, some single-writes.
    let initial: &[(Bytes, Bytes)] = &[
        (Bytes::from("k1"), Bytes::from("v1a")),
        (Bytes::from("k2"), Bytes::from("v2a")),
        (Bytes::from("k3"), Bytes::from("v3")),
    ];
    for (k, v) in initial {
        mvcc.set_versioned(RecordKey::from(k.clone()), v.clone())
            .await
            .unwrap();
        gate.publish_committed_max(gate.last_committed() + 1);
    }

    // Update a couple of keys.
    mvcc.set_versioned(RecordKey::from(Bytes::from("k1")), Bytes::from("v1b"))
        .await
        .unwrap();
    gate.publish_committed_max(gate.last_committed() + 1);
    mvcc.set_versioned(RecordKey::from(Bytes::from("k2")), Bytes::from("v2b"))
        .await
        .unwrap();
    gate.publish_committed_max(gate.last_committed() + 1);

    // Now stream both ways and compare.
    let pin = gate.last_committed();

    let snapshot_stream = mvcc.snapshot_stream(64, pin);
    let snapshot_result = collect_stream_sorted(snapshot_stream).await;

    let current_stream = mvcc.current_stream(64);
    let current_result = collect_stream_sorted(current_stream).await;

    // Byte-identical output: same keys, same values, same order (BTreeMap sorted).
    assert_eq!(
        snapshot_result, current_result,
        "snapshot_stream with pin=last_committed must be byte-identical to current_stream"
    );

    // Verify expected current values.
    assert_eq!(snapshot_result.get(&b"k1"[..]), Some(&Bytes::from("v1b")));
    assert_eq!(snapshot_result.get(&b"k2"[..]), Some(&Bytes::from("v2b")));
    assert_eq!(snapshot_result.get(&b"k3"[..]), Some(&Bytes::from("v3")));
}

/// Test 4: Overlay branch scenarios.
///
/// Test with a key's winner in the overlay (not yet drained to history).
/// Pin BEFORE the overlay write → row excluded.
/// Pin AFTER the overlay write → row included, value from overlay.
#[tokio::test]
async fn snapshot_stream_overlay_branch() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    // Set floor to 5 (overlay writes will be at versions <= 5).
    gate.publish_committed_max(5);

    // Write to overlay directly (version 3, before floor).
    mvcc.overlay().insert(
        RecordKey::from(Bytes::from("overlay_key")),
        3,
        Bytes::from("overlay_value"),
    );

    // Write to history for a different key.
    mvcc.set_versioned(
        RecordKey::from(Bytes::from("history_key")),
        Bytes::from("history_value"),
    )
    .await
    .unwrap();
    gate.publish_committed_max(5);

    // Scenario A: pin BEFORE the overlay write (version 2).
    // Overlay entry at v3 should be excluded (3 > 2).
    let stream_before = mvcc.snapshot_stream(64, 2);
    let result_before = collect_stream_sorted(stream_before).await;

    assert!(
        !result_before.contains_key(b"overlay_key".as_ref()),
        "overlay entry at v3 should be excluded when pinned at v2"
    );
    assert!(
        result_before.contains_key(b"history_key".as_ref()),
        "history entry should be included"
    );

    // Scenario B: pin AFTER the overlay write (version 4).
    // Overlay entry at v3 should be included (3 <= 4).
    let stream_after = mvcc.snapshot_stream(64, 4);
    let result_after = collect_stream_sorted(stream_after).await;

    assert!(
        result_after.contains_key(b"overlay_key".as_ref()),
        "overlay entry at v3 should be included when pinned at v4"
    );
    assert_eq!(
        result_after.get(b"overlay_key".as_ref()),
        Some(&Bytes::from("overlay_value")),
        "overlay value should match"
    );
    assert!(
        result_after.contains_key(b"history_key".as_ref()),
        "history entry should still be included"
    );

    // Scenario C: pin at floor (version 5).
    // Both overlay and history should be included.
    let stream_floor = mvcc.snapshot_stream(64, 5);
    let result_floor = collect_stream_sorted(stream_floor).await;

    assert_eq!(
        result_floor.len(),
        2,
        "both overlay and history entries should be included at floor"
    );
    assert_eq!(
        result_floor.get(b"overlay_key".as_ref()),
        Some(&Bytes::from("overlay_value")),
        "overlay value at floor"
    );
    assert_eq!(
        result_floor.get(b"history_key".as_ref()),
        Some(&Bytes::from("history_value")),
        "history value at floor"
    );
}
