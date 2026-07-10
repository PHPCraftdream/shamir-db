// O(N) ack: test assertions on scc map cardinality — not a hot path.
#![allow(clippy::disallowed_methods)]

use super::helpers::{make_gate, make_mvcc, make_mvcc_with_gate};
use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::types::RecordKey;

#[tokio::test]
async fn set_without_snapshots_skips_history() {
    let mvcc = make_mvcc();
    let key = Bytes::from("k1");
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();

    // The value is in the log (single append).
    // Assert via the seam — the single authoritative source.
    let via_seam = mvcc.get_current(RecordKey::from(key.clone())).await.unwrap();
    assert_eq!(via_seam, Some(Bytes::from("v1")));

    // Exactly 1 version-key entry exists in the log alongside ts-keys.
    let stream = mvcc.history.iter_stream(64);
    futures::pin_mut!(stream);
    let mut version_keys = 0;
    while let Some(batch) = stream.next().await {
        for (hk, _) in batch.unwrap() {
            if crate::version_codec::decode_version_key(&hk).is_some() {
                version_keys += 1;
            }
        }
    }
    assert_eq!(
        version_keys, 1,
        "FINAL-A: exactly 1 version-key entry in the log (single append)"
    );
}

#[tokio::test]
async fn set_with_snapshot_archives_old_value() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    // Open a snapshot so active_snapshots is non-empty.
    let _guard = gate.open_snapshot().await;

    let key = Bytes::from("k1");
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v2"))
        .await
        .unwrap();

    // v2 is the current entry in the log (single log append).
    // Assert via the seam.
    let via_seam = mvcc.get_current(RecordKey::from(key.clone())).await.unwrap();
    assert_eq!(via_seam, Some(Bytes::from("v2")));

    // The log contains BOTH v1 (written when it was current)
    // and v2 (the latest current — single log append).
    let stream = mvcc.history.iter_stream(64);
    futures::pin_mut!(stream);
    let mut found_v1 = false;
    let mut found_v2 = false;
    while let Some(batch) = stream.next().await {
        for (hk, hv) in batch.unwrap() {
            let Some((orig_key, ver)) = crate::version_codec::decode_version_key(&hk) else {
                continue;
            };
            assert_eq!(orig_key, &b"k1"[..]);
            if ver == 1 {
                assert_eq!(hv, Bytes::from("v1"));
                found_v1 = true;
            } else if ver == 2 {
                assert_eq!(hv, Bytes::from("v2"));
                found_v2 = true;
            }
        }
    }
    assert!(found_v1, "history should contain v1");
    assert!(
        found_v2,
        "C1: history should contain v2 (current in the log)"
    );
}

#[tokio::test]
async fn get_at_current_version() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let _guard = gate.open_snapshot().await;
    let key = Bytes::from("k1");
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();

    let v = gate.assign_next_version();
    // snapshot_version >> v → direct-read path returns from log
    let result = mvcc.get_at(b"k1", v + 100).await.unwrap();
    assert_eq!(result, Some(Bytes::from("v1")));
}

#[tokio::test]
async fn get_at_old_version() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let _guard = gate.open_snapshot().await;
    let key = Bytes::from("k1");

    // v1 written at version 1
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    // v2 written at version 2
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v2"))
        .await
        .unwrap();

    // Query at snapshot between v1 and v2 — should find v1 in history.
    // After two set_versioned calls, version_cache[k1] = 2.
    // get_at(snapshot=1) → cur_v(2) > 1 → log range-scan.
    let result = mvcc.get_at(b"k1", 1).await.unwrap();
    assert_eq!(result, Some(Bytes::from("v1")));
}

#[tokio::test]
async fn get_at_missing_key() {
    let mvcc = make_mvcc();
    let result = mvcc.get_at(b"nonexistent", 999).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn delete_versioned_archives() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let _guard = gate.open_snapshot().await;
    let key = Bytes::from("k1");

    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    mvcc.delete_versioned(RecordKey::from(key.clone())).await.unwrap();

    // The seam returns None: the log contains a tombstone for this key.
    let via_seam = mvcc.get_current(RecordKey::from(key.clone())).await.unwrap();
    assert!(
        via_seam.is_none(),
        "FINAL-A: get_current must be None after delete_versioned (tombstone in log)"
    );

    // log contains v1 (written when it was current) and v2 (the delete's
    // tombstone — empty value).
    let stream = mvcc.history.iter_stream(64);
    futures::pin_mut!(stream);
    let mut found_v1 = false;
    let mut found_tombstone = false;
    while let Some(batch) = stream.next().await {
        for (hk, hv) in batch.unwrap() {
            let Some((_orig_key, ver)) = crate::version_codec::decode_version_key(&hk) else {
                continue;
            };
            if ver == 1 {
                assert_eq!(hv, Bytes::from("v1"));
                found_v1 = true;
            } else if ver == 2 {
                assert_eq!(
                    hv,
                    Bytes::new(),
                    "C1: delete writes tombstone (empty value)"
                );
                found_tombstone = true;
            }
        }
    }
    assert!(found_v1, "history should contain v1");
    assert!(
        found_tombstone,
        "C1: history should contain the delete tombstone"
    );
}

#[tokio::test]
async fn get_at_after_delete() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let _guard = gate.open_snapshot().await;
    let key = Bytes::from("k1");

    // v1 at version 1
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    // delete at version 2
    mvcc.delete_versioned(RecordKey::from(key.clone())).await.unwrap();

    // get_at between v1 and delete → log range-scan → v1
    let result = mvcc.get_at(b"k1", 1).await.unwrap();
    assert_eq!(result, Some(Bytes::from("v1")));

    // get_at after delete → direct read (cur_v=2 <= 15) → history.get(tombstone) → None
    let result = mvcc.get_at(b"k1", 15).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn get_at_busy_history_five_versions() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let _guard = gate.open_snapshot().await;

    let key = Bytes::from("busy");
    let mut version_at = Vec::new();

    for i in 1..=5u32 {
        mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
        let v = mvcc.version_of(&key);
        version_at.push(v);
    }

    // Query at each historical version and verify we get the right value.
    // version_at[0] was assigned when v1 was written, so get_at(version_at[0]) → v1
    for (idx, &snap) in version_at.iter().enumerate() {
        let result = mvcc.get_at(key.as_ref(), snap).await.unwrap();
        let expected = format!("v{}", idx + 1);
        assert_eq!(
            result,
            Some(Bytes::from(expected.clone())),
            "at snapshot {} expected {}",
            snap,
            expected
        );
    }

    // Query at version 0 (before any write) → log range-scan → empty → None
    let result_before = mvcc.get_at(key.as_ref(), 0).await.unwrap();
    assert!(
        result_before.is_none(),
        "no value should exist before first write"
    );

    // Query at a very high version → direct-read path → current (v5)
    let result_latest = mvcc.get_at(key.as_ref(), u64::MAX - 1).await.unwrap();
    assert_eq!(result_latest, Some(Bytes::from("v5")));
}

/// `get_current` reads the log: a written key returns `Some(v)`, an
/// absent key returns `Ok(None)` (NOT an error). Assertions are against
/// the single log (no `main` store exists).
#[tokio::test]
async fn get_current_matches_main_get() {
    let mvcc = make_mvcc();
    let key = Bytes::from("cur-k1");
    let val = Bytes::from("cur-v1");

    mvcc.set_versioned(RecordKey::from(key.clone()), val.clone()).await.unwrap();

    // Seam returns the written value (from the log).
    let via_seam = mvcc.get_current(RecordKey::from(key.clone())).await.unwrap();
    assert_eq!(via_seam, Some(val.clone()));

    // A key never written → Ok(None), NOT an Err.
    let absent = mvcc
        .get_current(RecordKey::from(Bytes::from("never-written")))
        .await
        .unwrap();
    assert!(
        absent.is_none(),
        "absent key must be Ok(None), not an error"
    );
}

/// After `delete_versioned`, the log has a tombstone and `get_current`
/// returns `Ok(None)`. The log is the only source of truth (no `main`
/// store exists).
#[tokio::test]
async fn get_current_none_after_delete() {
    let mvcc = make_mvcc();
    let key = Bytes::from("del-k1");

    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    // Sanity: present before delete (reads from the log).
    assert_eq!(
        mvcc.get_current(RecordKey::from(key.clone())).await.unwrap(),
        Some(Bytes::from("v1"))
    );

    mvcc.delete_versioned(RecordKey::from(key.clone())).await.unwrap();

    // Seam reads tombstone from the log → Ok(None).
    let after = mvcc.get_current(RecordKey::from(key.clone())).await.unwrap();
    assert!(after.is_none(), "get_current must be Ok(None) after delete");
}

#[tokio::test]
async fn zero_overhead_no_snapshots() {
    let mvcc = make_mvcc();
    for i in 0..100u32 {
        let key = Bytes::copy_from_slice(&i.to_be_bytes());
        mvcc.set_versioned(RecordKey::from(key), Bytes::from("val")).await.unwrap();
    }

    // C1: every write puts the current version into the log, so 100
    // version-key entries exist (one per key).
    let stream = mvcc.history.iter_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        for (hk, _) in batch.unwrap() {
            if crate::version_codec::decode_version_key(&hk).is_some() {
                count += 1;
            }
        }
    }
    assert_eq!(
        count, 100,
        "C1: every write puts current into the log (100 version-key entries)"
    );
}

/// `set_versioned_many` runs the single log-write path
/// (publish_cell → log transact). For a batch of brand-new keys every
/// key gets its own cell entry carrying its assigned version (one version
/// per record); the log contains exactly n version-key entries.
///
/// T1b.1: uses `KeepHistory` so the eager vacuum does not prune the
/// cells before the assertions (this test checks cell-population, not
/// vacuum behaviour).
#[tokio::test]
async fn set_versioned_many_batches_no_snapshot() {
    use crate::mvcc_store::Retention;

    let mvcc = make_mvcc();
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let n = 50u32;
    let items: Vec<(Bytes, Bytes)> = (0..n)
        .map(|i| {
            (
                Bytes::copy_from_slice(&i.to_be_bytes()),
                Bytes::from(format!("val{i}")),
            )
        })
        .collect();
    mvcc.set_versioned_many(items.into_iter().map(|(k, v)| (RecordKey::from(k), v)).collect::<Vec<_>>()).await.unwrap();

    // Every record is in the log (single log append). Assert via the seam.
    for i in 0..n {
        let k = Bytes::copy_from_slice(&i.to_be_bytes());
        let val = mvcc.get_current(RecordKey::from(k.clone())).await.unwrap();
        assert_eq!(val, Some(Bytes::from(format!("val{i}"))));
    }

    // Every write puts the current version into the log, so n version-key
    // entries exist (one per key).
    let stream = mvcc.history.iter_stream(64);
    futures::pin_mut!(stream);
    let mut hist = 0usize;
    while let Some(batch) = stream.next().await {
        for (hk, _) in batch.unwrap() {
            if crate::version_codec::decode_version_key(&hk).is_some() {
                hist += 1;
            }
        }
    }
    assert_eq!(
        hist, n as usize,
        "FINAL-A: every key has exactly one version-key entry in the log (sole write)"
    );

    // Every key is published into the cells (one entry per key).
    assert_eq!(
        mvcc.cells.len(),
        n as usize,
        "T1a always-archive: every batch key gets a cell entry"
    );
}

/// Snapshot-active path: new values go directly to the log (sole write),
/// and every key gets a fresh monotonic version in the cache
/// (one version per record, like the per-record `set_versioned` loop).
#[tokio::test]
async fn set_versioned_many_with_snapshot_archives_and_versions() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let _guard = gate.open_snapshot().await;

    // k0 is a brand-new key (no prior in log).
    let k0 = Bytes::from("k0");

    let items: Vec<(Bytes, Bytes)> = vec![
        (k0.clone(), Bytes::from("new0")),
        (Bytes::from("k1"), Bytes::from("v1")),
        (Bytes::from("k2"), Bytes::from("v2")),
    ];
    mvcc.set_versioned_many(items.into_iter().map(|(k, v)| (RecordKey::from(k), v)).collect::<Vec<_>>()).await.unwrap();

    // All new values are in the log (sole write). Assert via seam.
    assert_eq!(
        mvcc.get_current(RecordKey::from(k0.clone())).await.unwrap(),
        Some(Bytes::from("new0"))
    );
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from("k1"))).await.unwrap(),
        Some(Bytes::from("v1"))
    );
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from("k2"))).await.unwrap(),
        Some(Bytes::from("v2"))
    );

    // The log contains the new values at their assigned versions.
    let stream = mvcc.history.iter_stream(64);
    futures::pin_mut!(stream);
    let mut found_new0 = false;
    while let Some(batch) = stream.next().await {
        for (hk, hv) in batch.unwrap() {
            if let Some((orig, _ver)) = crate::version_codec::decode_version_key(&hk) {
                if orig == b"k0" && hv.as_ref() == b"new0" {
                    found_new0 = true;
                }
            }
        }
    }
    assert!(
        found_new0,
        "C1: k0's new value is in the log (current-into-log)"
    );

    // Every key got a positive version in the cache.
    assert!(mvcc.version_of(&k0) > 0);
    assert!(mvcc.version_of(b"k1") > 0);
    assert!(mvcc.version_of(b"k2") > 0);
    assert_eq!(mvcc.cells.len(), 3);
}

/// Empty input is a no-op (no panic, no writes).
#[tokio::test]
async fn set_versioned_many_empty_is_noop() {
    let mvcc = make_mvcc();
    mvcc.set_versioned_many(Vec::<(RecordKey, Bytes)>::new())
        .await
        .unwrap();
    assert_eq!(mvcc.cells.len(), 0);
}

/// Regression guard: writing a brand-new key under an active snapshot
/// succeeds (there is no prior version to supersede) and the key is
/// readable via the log immediately after the write.
#[tokio::test]
async fn set_versioned_new_key_under_snapshot_succeeds() {
    use super::helpers::count_history_entries;

    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let _guard = gate.open_snapshot().await;

    let key = Bytes::from("brand_new_key");
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();

    // FINAL-A: key is readable via the seam (log).
    let val = mvcc.get_current(RecordKey::from(key.clone())).await.unwrap();
    assert_eq!(val, Some(Bytes::from("v1")));

    // The single log append produces exactly 1 entry for a brand-new key.
    let count = count_history_entries(&mvcc).await;
    assert_eq!(
        count, 1,
        "C1: current version in the log for a brand-new key"
    );
}

// ================================================================
// C1 — current-into-log / tombstone / vacuum-guard tests.
// ================================================================

/// The current version is written into the log on every write.
#[tokio::test]
async fn c1_current_version_is_in_the_log() {
    use crate::version_codec::encode_version_key;

    let mvcc = make_mvcc();
    let key = Bytes::from("c1_key");
    let val = Bytes::from("c1_val");
    let v = mvcc.set_versioned(RecordKey::from(key.clone()), val.clone()).await.unwrap();

    // The log entry at encode_version_key(key, v) must hold val.
    let log_val = mvcc
        .history_store()
        .get(encode_version_key(&key, v).into())
        .await
        .unwrap();
    assert_eq!(log_val, val, "C1: current version must be in the log");
}

/// C1 guarantee: delete writes a tombstone (empty value) into the log.
#[tokio::test]
async fn c1_delete_writes_tombstone() {
    use crate::version_codec::encode_version_key;

    let mvcc = make_mvcc();
    let key = Bytes::from("c1_del");
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("val"))
        .await
        .unwrap();
    let del_v = mvcc.delete_versioned(RecordKey::from(key.clone())).await.unwrap();

    let tombstone = mvcc
        .history_store()
        .get(encode_version_key(&key, del_v).into())
        .await
        .unwrap();
    assert_eq!(
        tombstone,
        Bytes::new(),
        "C1: delete must write an empty tombstone into the log"
    );
}

// ================================================================
// C2 — reads resolve from the single LOG, not `main`.
// ================================================================

/// C2: `get_current` reads the log. The structural guarantee that there is
/// no `main` makes this invariant enforcement permanent.
#[tokio::test]
async fn c2_get_current_reads_log_not_main() {
    let mvcc = make_mvcc();
    let key = Bytes::from("c2_k");
    let val = Bytes::from("c2_v");
    mvcc.set_versioned(RecordKey::from(key.clone()), val.clone()).await.unwrap();

    let got = mvcc.get_current(RecordKey::from(key.clone())).await.unwrap();
    assert_eq!(got, Some(val), "C2: get_current must read the log");
}

/// C2: `get_current` of a deleted key returns `None` (reads the tombstone).
#[tokio::test]
async fn c2_get_current_none_for_tombstone() {
    let mvcc = make_mvcc();
    let key = Bytes::from("c2_del");
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v"))
        .await
        .unwrap();
    mvcc.delete_versioned(RecordKey::from(key.clone())).await.unwrap();

    let got = mvcc.get_current(RecordKey::from(key)).await.unwrap();
    assert!(got.is_none(), "C2: deleted key → None (tombstone)");
}

/// C2: `get_at` at the delete version → `None`; at a pre-delete snapshot →
/// the old value. KeepHistory so the pre-delete version is not vacuumed.
#[tokio::test]
async fn c2_get_at_tombstone_is_none() {
    use crate::mvcc_store::Retention;

    let mvcc = make_mvcc();
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let key = Bytes::from("c2_asof");
    let va = mvcc
        .set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    let vd = mvcc.delete_versioned(RecordKey::from(key.clone())).await.unwrap();

    // As-of the delete version → deleted (None).
    let at_delete = mvcc.get_at(&key, vd).await.unwrap();
    assert!(at_delete.is_none(), "C2: as-of delete version → None");

    // As-of the pre-delete version → the old value.
    let at_before = mvcc.get_at(&key, va).await.unwrap();
    assert_eq!(
        at_before,
        Some(Bytes::from("v1")),
        "C2: pre-delete snapshot sees the old value"
    );
}

/// C2: cold-start — a fresh MvccStore over the SAME log (empty cell cache)
/// resolves `get_current` by seeking the latest version in the log.
#[tokio::test]
async fn c2_cold_start_seek() {
    use crate::mvcc_store::MvccStore;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::Store;
    use std::sync::Arc;

    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let key = Bytes::from("c2_cold");
    let val = Bytes::from("cold_val");

    // First store writes the value into the shared log.
    let mvcc1 = MvccStore::new(Arc::clone(&history), make_gate());
    mvcc1.set_versioned(RecordKey::from(key.clone()), val.clone()).await.unwrap();

    // Second store over the SAME log has an empty cell cache → cur_v == 0
    // → must seek the latest version from the log.
    let mvcc2 = MvccStore::new(Arc::clone(&history), make_gate());
    assert_eq!(
        mvcc2.version_of(&key),
        0,
        "precondition: cold cell (no cached version)"
    );
    let got = mvcc2.get_current(RecordKey::from(key)).await.unwrap();
    assert_eq!(
        got,
        Some(val),
        "C2: cold-start get_current must seek the log"
    );
}
