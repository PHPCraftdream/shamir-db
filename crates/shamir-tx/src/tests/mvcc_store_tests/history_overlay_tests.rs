//! `history_of` must see versions that are committed and visible but not yet
//! drained into the `history` log.
//!
//! Since the D2 P1d-2b cutover the ack-path writes a commit only to the
//! in-memory overlay (`apply_committed_visible`); the background drainer lands
//! it in `history` later (`write_committed_to_history`). A timeline read in
//! that window used to miss the newest versions — an intermittent
//! `ts-e2e-nightly` failure (`history:*` e2e tests saw 1-2 of 3 versions).

use bytes::Bytes;
use shamir_storage::types::KvOp;

use super::helpers::{make_gate, make_mvcc_with_gate};

fn set(key: &'static [u8], val: &'static [u8]) -> Vec<KvOp> {
    vec![KvOp::Set(
        Bytes::from_static(key).into(),
        Bytes::from_static(val),
    )]
}

/// Three commits acked through the overlay only (no drain yet): the whole
/// timeline must be visible, with each version's commit-time ts.
#[tokio::test]
async fn history_of_sees_overlay_only_versions() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    for (i, (val, ts)) in [(b"v1", 1_000u64), (b"v2", 2_000), (b"v3", 3_000)]
        .into_iter()
        .enumerate()
    {
        mvcc.set_test_now(ts);
        let v = gate.assign_next_version();
        assert_eq!(v, i as u64 + 1);
        mvcc.apply_committed_visible(&set(b"k", val), v);
    }

    let timeline = mvcc.history_of(b"k").await.unwrap();
    let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
    assert_eq!(versions, vec![1, 2, 3], "undrained versions must be listed");
    let values: Vec<&[u8]> = timeline.iter().map(|e| e.value.as_ref()).collect();
    assert_eq!(values, vec![b"v1".as_slice(), b"v2", b"v3"]);
    let stamps: Vec<Option<u64>> = timeline.iter().map(|e| e.ts_millis).collect();
    assert_eq!(
        stamps,
        vec![Some(1_000), Some(2_000), Some(3_000)],
        "commit-time ts must come from the pending stamp until drained"
    );
}

/// Drainer is partway through: v1/v2 already in `history`, v3 only in the
/// overlay (and v1/v2 still in the overlay too, GC hasn't run). The merge
/// must yield each version exactly once.
#[tokio::test]
async fn history_of_merges_overlay_and_history_without_duplicates() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let vals: [&'static [u8]; 3] = [b"v1", b"v2", b"v3"];
    let mut all_ops = Vec::new();
    for (i, val) in vals.into_iter().enumerate() {
        mvcc.set_test_now(1_000 * (i as u64 + 1));
        let v = gate.assign_next_version();
        let ops = set(b"k", val);
        mvcc.apply_committed_visible(&ops, v);
        all_ops.push((v, ops));
    }
    for (v, ops) in all_ops.iter().take(2) {
        mvcc.write_committed_to_history(ops, *v).await.unwrap();
    }

    let timeline = mvcc.history_of(b"k").await.unwrap();
    let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
    assert_eq!(versions, vec![1, 2, 3], "no duplicates, no gaps");
    assert_eq!(timeline[2].value.as_ref(), b"v3");
    assert_eq!(timeline[2].ts_millis, Some(3_000));
}

/// A delete acked through the overlay shows up as a tombstone (empty value),
/// the same convention `history` uses.
#[tokio::test]
async fn history_of_overlay_tombstone_is_empty_value() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let v1 = gate.assign_next_version();
    mvcc.apply_committed_visible(&set(b"k", b"alive"), v1);
    let v2 = gate.assign_next_version();
    mvcc.apply_committed_visible(&[KvOp::Remove(Bytes::from_static(b"k").into())], v2);

    let timeline = mvcc.history_of(b"k").await.unwrap();
    assert_eq!(timeline.len(), 2);
    assert_eq!(timeline[0].value.as_ref(), b"alive");
    assert!(timeline[1].value.is_empty(), "delete = tombstone");
}

/// Another key's overlay entries must not leak into this key's timeline.
#[tokio::test]
async fn history_of_overlay_is_scoped_to_the_key() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let v1 = gate.assign_next_version();
    mvcc.apply_committed_visible(&set(b"k", b"mine"), v1);
    let v2 = gate.assign_next_version();
    mvcc.apply_committed_visible(&set(b"k2", b"other"), v2);
    let v3 = gate.assign_next_version();
    mvcc.apply_committed_visible(&set(b"a", b"other"), v3);

    let timeline = mvcc.history_of(b"k").await.unwrap();
    assert_eq!(timeline.len(), 1);
    assert_eq!(timeline[0].version, v1);
}
