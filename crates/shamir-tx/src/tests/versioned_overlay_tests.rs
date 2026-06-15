use bytes::Bytes;

use crate::versioned_overlay::VersionedOverlay;

// ============================================================================
// insert + get — basic point lookup
// ============================================================================

#[test]
fn insert_and_get_returns_value() {
    let ov = VersionedOverlay::new();
    let key = Bytes::from_static(b"alice");
    let val = Bytes::from_static(b"v1-data");

    ov.insert(key.clone(), 1, val.clone());

    assert_eq!(ov.get(b"alice", 1), Some(val));
    assert_eq!(ov.len(), 1);
    assert!(ov.approx_bytes() > 0);
}

#[test]
fn get_missing_key_returns_none() {
    let ov = VersionedOverlay::new();
    assert_eq!(ov.get(b"ghost", 42), None);
}

#[test]
fn get_wrong_version_returns_none() {
    let ov = VersionedOverlay::new();
    ov.insert(Bytes::from_static(b"k"), 5, Bytes::from_static(b"v"));

    assert_eq!(ov.get(b"k", 4), None);
    assert_eq!(ov.get(b"k", 6), None);
    assert_eq!(ov.get(b"k", 5), Some(Bytes::from_static(b"v")));
}

// ============================================================================
// newest_visible — range-based latest version lookup
// ============================================================================

#[test]
fn newest_visible_single_version() {
    let ov = VersionedOverlay::new();
    ov.insert(Bytes::from_static(b"k"), 10, Bytes::from_static(b"ten"));

    assert_eq!(
        ov.newest_visible(b"k", 10),
        Some((10, Bytes::from_static(b"ten")))
    );
    assert_eq!(
        ov.newest_visible(b"k", 100),
        Some((10, Bytes::from_static(b"ten")))
    );
    assert_eq!(ov.newest_visible(b"k", 9), None);
}

#[test]
fn newest_visible_multiple_versions() {
    let ov = VersionedOverlay::new();
    ov.insert(Bytes::from_static(b"k"), 3, Bytes::from_static(b"v3"));
    ov.insert(Bytes::from_static(b"k"), 7, Bytes::from_static(b"v7"));
    ov.insert(Bytes::from_static(b"k"), 12, Bytes::from_static(b"v12"));

    // Exact matches.
    assert_eq!(
        ov.newest_visible(b"k", 12),
        Some((12, Bytes::from_static(b"v12")))
    );
    assert_eq!(
        ov.newest_visible(b"k", 7),
        Some((7, Bytes::from_static(b"v7")))
    );
    assert_eq!(
        ov.newest_visible(b"k", 3),
        Some((3, Bytes::from_static(b"v3")))
    );

    // Between versions — returns the floor.
    assert_eq!(
        ov.newest_visible(b"k", 10),
        Some((7, Bytes::from_static(b"v7")))
    );
    assert_eq!(
        ov.newest_visible(b"k", 5),
        Some((3, Bytes::from_static(b"v3")))
    );

    // Below all.
    assert_eq!(ov.newest_visible(b"k", 2), None);
}

// ============================================================================
// tombstone (empty Bytes) — stored and returned as Some(empty)
// ============================================================================

#[test]
fn tombstone_stored_as_empty_bytes() {
    let ov = VersionedOverlay::new();
    let key = Bytes::from_static(b"deleted");
    ov.insert(key, 5, Bytes::new());

    // Point lookup returns Some(empty) — NOT None.
    assert_eq!(ov.get(b"deleted", 5), Some(Bytes::new()));

    // newest_visible also returns the tombstone.
    assert_eq!(ov.newest_visible(b"deleted", 5), Some((5, Bytes::new())));
}

// ============================================================================
// gc_upto — threshold-based garbage collection
// ============================================================================

#[test]
fn gc_upto_removes_below_threshold() {
    let ov = VersionedOverlay::new();
    ov.insert(Bytes::from_static(b"a"), 1, Bytes::from_static(b"v1"));
    ov.insert(Bytes::from_static(b"a"), 5, Bytes::from_static(b"v5"));
    ov.insert(Bytes::from_static(b"a"), 10, Bytes::from_static(b"v10"));
    ov.insert(Bytes::from_static(b"b"), 3, Bytes::from_static(b"bv3"));
    assert_eq!(ov.len(), 4);

    // GC with threshold = min(durable_wm=5, floor=5) = 5.
    ov.gc_upto(5, 5);

    // Versions 1, 3, 5 are <= 5 → removed.
    assert_eq!(ov.len(), 1);
    assert_eq!(ov.get(b"a", 1), None);
    assert_eq!(ov.get(b"a", 5), None);
    assert_eq!(ov.get(b"b", 3), None);

    // Version 10 survives.
    assert_eq!(ov.get(b"a", 10), Some(Bytes::from_static(b"v10")));
    assert!(ov.approx_bytes() > 0);
}

#[test]
fn gc_upto_uses_min_of_both_args() {
    let ov = VersionedOverlay::new();
    ov.insert(Bytes::from_static(b"k"), 3, Bytes::from_static(b"v3"));
    ov.insert(Bytes::from_static(b"k"), 7, Bytes::from_static(b"v7"));

    // durable_watermark=10, floor=3 → threshold = 3.
    ov.gc_upto(10, 3);

    assert_eq!(ov.len(), 1);
    assert_eq!(ov.get(b"k", 3), None);
    assert_eq!(ov.get(b"k", 7), Some(Bytes::from_static(b"v7")));
}

#[test]
fn gc_upto_zero_is_noop() {
    let ov = VersionedOverlay::new();
    ov.insert(Bytes::from_static(b"k"), 1, Bytes::from_static(b"v"));
    ov.gc_upto(0, 0);
    assert_eq!(ov.len(), 1);
}

#[test]
fn approx_bytes_consistent_after_gc() {
    let ov = VersionedOverlay::new();
    ov.insert(Bytes::from_static(b"k"), 1, Bytes::from_static(b"val1"));
    ov.insert(Bytes::from_static(b"k"), 2, Bytes::from_static(b"val2"));
    let bytes_before = ov.approx_bytes();
    assert!(bytes_before > 0);

    ov.gc_upto(1, 1);
    let bytes_after = ov.approx_bytes();
    assert!(bytes_after < bytes_before);
    assert!(bytes_after > 0); // version 2 still present
}

// ============================================================================
// key isolation — newest_visible must not leak across keys
// ============================================================================

#[test]
fn newest_visible_key_isolation() {
    let ov = VersionedOverlay::new();
    // Key "a" has versions 1, 5.
    ov.insert(Bytes::from_static(b"a"), 1, Bytes::from_static(b"a1"));
    ov.insert(Bytes::from_static(b"a"), 5, Bytes::from_static(b"a5"));
    // Key "b" has version 3.
    ov.insert(Bytes::from_static(b"b"), 3, Bytes::from_static(b"b3"));
    // Key "ab" has version 2 — prefix of neither "a" nor "b" in Ord terms,
    // but lexicographically between "a" and "b".
    ov.insert(Bytes::from_static(b"ab"), 2, Bytes::from_static(b"ab2"));

    // "a" at snapshot=10 → version 5, not version 3 from "b" or 2 from "ab".
    assert_eq!(
        ov.newest_visible(b"a", 10),
        Some((5, Bytes::from_static(b"a5")))
    );

    // "b" at snapshot=10 → version 3, not version 5 from "a".
    assert_eq!(
        ov.newest_visible(b"b", 10),
        Some((3, Bytes::from_static(b"b3")))
    );

    // "ab" at snapshot=10 → version 2 only.
    assert_eq!(
        ov.newest_visible(b"ab", 10),
        Some((2, Bytes::from_static(b"ab2")))
    );

    // "c" — no entries at all.
    assert_eq!(ov.newest_visible(b"c", 10), None);
}

// ============================================================================
// empty / default
// ============================================================================

#[test]
fn default_is_empty() {
    let ov = VersionedOverlay::default();
    assert!(ov.is_empty());
    assert_eq!(ov.len(), 0);
    assert_eq!(ov.approx_bytes(), 0);
}

// ============================================================================
// idempotent insert (duplicate version)
// ============================================================================

#[test]
fn duplicate_insert_is_idempotent() {
    let ov = VersionedOverlay::new();
    ov.insert(Bytes::from_static(b"k"), 1, Bytes::from_static(b"v"));
    ov.insert(Bytes::from_static(b"k"), 1, Bytes::from_static(b"v"));

    assert_eq!(ov.len(), 1);
    assert_eq!(ov.get(b"k", 1), Some(Bytes::from_static(b"v")));
}

// ============================================================================
// concurrent inserts (optional — stress test)
// ============================================================================

#[test]
fn concurrent_inserts_consistent_count() {
    use std::sync::Arc;
    use std::thread;

    let ov = Arc::new(VersionedOverlay::new());
    let n_threads = 4;
    let n_per_thread = 100;

    let handles: Vec<_> = (0..n_threads)
        .map(|t| {
            let ov = Arc::clone(&ov);
            thread::spawn(move || {
                for i in 0..n_per_thread {
                    let key = Bytes::from(format!("t{t}-k{i}"));
                    let version = (t as u64) * 10_000 + (i as u64);
                    let value = Bytes::from(format!("val-{t}-{i}"));
                    ov.insert(key, version, value);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(ov.len(), n_threads * n_per_thread);
    assert!(ov.approx_bytes() > 0);
}

// ============================================================================
// snapshot_le — per-key winner ≤ floor across the whole overlay (P1b)
// ============================================================================

#[test]
fn snapshot_le_empty_overlay_is_empty() {
    let ov = VersionedOverlay::new();
    assert!(ov.snapshot_le(100).is_empty());
}

#[test]
fn snapshot_le_floor_zero_is_empty() {
    let ov = VersionedOverlay::new();
    ov.insert(Bytes::from_static(b"k"), 1, Bytes::from_static(b"v"));
    assert!(ov.snapshot_le(0).is_empty());
}

#[test]
fn snapshot_le_picks_highest_version_per_key_le_floor() {
    let ov = VersionedOverlay::new();
    // key a: versions 1, 3, 7 ; key b: versions 2, 5
    ov.insert(Bytes::from_static(b"a"), 1, Bytes::from_static(b"a1"));
    ov.insert(Bytes::from_static(b"a"), 3, Bytes::from_static(b"a3"));
    ov.insert(Bytes::from_static(b"a"), 7, Bytes::from_static(b"a7"));
    ov.insert(Bytes::from_static(b"b"), 2, Bytes::from_static(b"b2"));
    ov.insert(Bytes::from_static(b"b"), 5, Bytes::from_static(b"b5"));

    // floor = 4 → a winner = v3 (a3), b winner = v2 (b2). v7/v5 excluded.
    let mut got = ov.snapshot_le(4);
    got.sort_by(|x, y| x.0.cmp(&y.0));
    assert_eq!(
        got,
        vec![
            (Bytes::from_static(b"a"), 3, Bytes::from_static(b"a3")),
            (Bytes::from_static(b"b"), 2, Bytes::from_static(b"b2")),
        ]
    );

    // floor = 100 → newest of each: a7, b5.
    let mut got = ov.snapshot_le(100);
    got.sort_by(|x, y| x.0.cmp(&y.0));
    assert_eq!(
        got,
        vec![
            (Bytes::from_static(b"a"), 7, Bytes::from_static(b"a7")),
            (Bytes::from_static(b"b"), 5, Bytes::from_static(b"b5")),
        ]
    );
}

#[test]
fn snapshot_le_includes_tombstone_winner() {
    let ov = VersionedOverlay::new();
    ov.insert(Bytes::from_static(b"k"), 2, Bytes::from_static(b"v2"));
    // newer version is a tombstone (empty value).
    ov.insert(Bytes::from_static(b"k"), 5, Bytes::new());

    // floor 10 → winner is the tombstone at v5 (caller suppresses it).
    let got = ov.snapshot_le(10);
    assert_eq!(got, vec![(Bytes::from_static(b"k"), 5, Bytes::new())]);

    // floor 4 → tombstone excluded, winner is v2.
    let got = ov.snapshot_le(4);
    assert_eq!(
        got,
        vec![(Bytes::from_static(b"k"), 2, Bytes::from_static(b"v2"))]
    );
}

#[test]
fn snapshot_le_key_only_above_floor_omitted() {
    let ov = VersionedOverlay::new();
    ov.insert(Bytes::from_static(b"hi"), 50, Bytes::from_static(b"x"));
    // floor below the only version → key omitted entirely.
    assert!(ov.snapshot_le(10).is_empty());
}
