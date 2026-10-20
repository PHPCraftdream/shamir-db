use crate::staging_store::{StagedKind, StagingStore};
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store};
use std::sync::Arc;

fn mem_store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

#[tokio::test]
async fn get_after_set_returns_staged_value() {
    let base = mem_store();
    let mut staging = StagingStore::new(base);
    let k: RecordKey = Bytes::from_static(b"k1").into();
    staging.set(k.clone(), Bytes::from_static(b"v1"));
    assert_eq!(staging.get(k).await.unwrap(), Bytes::from_static(b"v1"));
}

#[tokio::test]
async fn get_after_remove_returns_not_found_even_if_base_has_key() {
    let base = mem_store();
    let k: RecordKey = Bytes::from_static(b"k1").into();
    base.set(k.clone(), Bytes::from_static(b"original"))
        .await
        .unwrap();

    let mut staging = StagingStore::new(base);
    staging.remove(k.clone());
    assert!(staging.get(k).await.is_err());
}

#[tokio::test]
async fn get_falls_through_to_base_if_not_staged() {
    let base = mem_store();
    let k: RecordKey = Bytes::from_static(b"k1").into();
    base.set(k.clone(), Bytes::from_static(b"from_base"))
        .await
        .unwrap();

    let staging = StagingStore::new(base);
    assert_eq!(
        staging.get(k).await.unwrap(),
        Bytes::from_static(b"from_base")
    );
}

#[tokio::test]
async fn set_then_remove_collapses_to_remove() {
    let base = mem_store();
    let mut staging = StagingStore::new(base);
    let k: RecordKey = Bytes::from_static(b"k1").into();

    staging.set(k.clone(), Bytes::from_static(b"v"));
    staging.remove(k.clone());

    assert!(staging.get(k).await.is_err());
    assert_eq!(staging.len(), 1); // one key, final op = Remove
}

#[tokio::test]
async fn remove_then_set_collapses_to_set() {
    let base = mem_store();
    let k: RecordKey = Bytes::from_static(b"k1").into();
    base.set(k.clone(), Bytes::from_static(b"original"))
        .await
        .unwrap();

    let mut staging = StagingStore::new(base);
    staging.remove(k.clone());
    staging.set(k.clone(), Bytes::from_static(b"new"));

    assert_eq!(staging.get(k).await.unwrap(), Bytes::from_static(b"new"));
}

#[tokio::test]
async fn drain_produces_kvop_batch() {
    let base = mem_store();
    let mut staging = StagingStore::new(base);
    let k1: RecordKey = Bytes::from_static(b"k1").into();
    let k2: RecordKey = Bytes::from_static(b"k2").into();
    let k3: RecordKey = Bytes::from_static(b"k3").into();

    staging.set(k1.clone(), Bytes::from_static(b"v1"));
    staging.remove(k2.clone());
    staging.set(k3.clone(), Bytes::from_static(b"v3"));

    let ops = staging.drain();
    assert_eq!(ops.len(), 3);

    let sets: Vec<_> = ops
        .iter()
        .filter(|o| matches!(o, KvOp::Set(_, _)))
        .collect();
    let removes: Vec<_> = ops
        .iter()
        .filter(|o| matches!(o, KvOp::Remove(_)))
        .collect();
    assert_eq!(sets.len(), 2);
    assert_eq!(removes.len(), 1);
}

#[tokio::test]
async fn len_tracks_unique_keys() {
    let base = mem_store();
    let mut staging = StagingStore::new(base);
    let k: RecordKey = Bytes::from_static(b"k1").into();

    assert!(staging.is_empty());
    staging.set(k.clone(), Bytes::from_static(b"v1"));
    assert_eq!(staging.len(), 1);
    staging.set(k.clone(), Bytes::from_static(b"v2"));
    assert_eq!(staging.len(), 1); // same key, still 1
}

#[tokio::test]
async fn staged_op_returns_set_for_staged_value() {
    let base = mem_store();
    let mut staging = StagingStore::new(base);
    let k: RecordKey = Bytes::from_static(b"k1").into();
    staging.set(k.clone(), Bytes::from_static(b"v1"));

    assert_eq!(
        staging.staged_op(k.as_ref()),
        Some(StagedKind::Set(Bytes::from_static(b"v1")))
    );
}

#[tokio::test]
async fn staged_op_returns_removed_for_staged_remove() {
    // Even when the base store has the key, a staged Remove reports
    // Removed (and never consults the base — that is `get`'s job).
    let base = mem_store();
    let k: RecordKey = Bytes::from_static(b"k1").into();
    base.set(k.clone(), Bytes::from_static(b"original"))
        .await
        .unwrap();

    let mut staging = StagingStore::new(base);
    staging.remove(k.clone());

    assert_eq!(staging.staged_op(k.as_ref()), Some(StagedKind::Removed));
}

#[tokio::test]
async fn staged_op_returns_none_when_not_staged_even_if_base_has_key() {
    // `staged_op` reports ONLY this tx's staging; a key that lives only
    // in the base is `None` (no fall-through), unlike `get`.
    let base = mem_store();
    let k: RecordKey = Bytes::from_static(b"k1").into();
    base.set(k.clone(), Bytes::from_static(b"from_base"))
        .await
        .unwrap();

    let staging = StagingStore::new(base);
    assert_eq!(staging.staged_op(k.as_ref()), None);
}

#[tokio::test]
async fn staged_op_reflects_last_write_wins() {
    let base = mem_store();
    let mut staging = StagingStore::new(base);
    let k: RecordKey = Bytes::from_static(b"k1").into();

    staging.set(k.clone(), Bytes::from_static(b"v"));
    staging.remove(k.clone());
    assert_eq!(staging.staged_op(k.as_ref()), Some(StagedKind::Removed));

    staging.set(k.clone(), Bytes::from_static(b"again"));
    assert_eq!(
        staging.staged_op(k.as_ref()),
        Some(StagedKind::Set(Bytes::from_static(b"again")))
    );
}

#[tokio::test]
async fn staged_op_borrow_probe_matches_arbitrary_length_keys() {
    // The probe takes `&[u8]`; it must find staged entries regardless of
    // key length (NOT restricted to 16-byte record ids) and allocate no
    // `Bytes` to look up.
    let base = mem_store();
    let mut staging = StagingStore::new(base);

    let short: RecordKey = Bytes::from_static(b"ab").into(); // 2 bytes
    let long: RecordKey = Bytes::from_static(b"this-key-is-forty-bytes-long-padding-here").into(); // > 16
    let empty: RecordKey = Bytes::from_static(b"").into(); // 0 bytes

    staging.set(short.clone(), Bytes::from_static(b"s"));
    staging.remove(long.clone());
    staging.set(empty.clone(), Bytes::from_static(b"e"));

    assert_eq!(
        staging.staged_op(short.as_ref()),
        Some(StagedKind::Set(Bytes::from_static(b"s")))
    );
    assert_eq!(staging.staged_op(long.as_ref()), Some(StagedKind::Removed));
    assert_eq!(
        staging.staged_op(empty.as_ref()),
        Some(StagedKind::Set(Bytes::from_static(b"e")))
    );
    // A never-staged key of yet another length is still None.
    assert_eq!(staging.staged_op(b"never-staged-key".as_ref()), None);
}

#[tokio::test]
async fn staged_bytes_sums_keys_and_values() {
    let base = mem_store();
    let mut staging = StagingStore::new(base);

    // Empty staging → 0 bytes.
    assert_eq!(staging.staged_bytes(), 0);

    // One Set("ab", "12345") → key 2 + value 5 = 7 bytes.
    staging.set(
        Bytes::from_static(b"ab").into(),
        Bytes::from_static(b"12345"),
    );
    assert_eq!(staging.staged_bytes(), 7);

    // Add Remove("xyz") → key 3 bytes. Total = 7 + 3 = 10.
    staging.remove(Bytes::from_static(b"xyz").into());
    assert_eq!(staging.staged_bytes(), 10);
}

#[tokio::test]
async fn snapshot_ops_does_not_consume() {
    let base: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut staging = StagingStore::new(base);
    staging.set(
        RecordKey::from(Bytes::from_static(b"k1")),
        Bytes::from_static(b"v1"),
    );
    staging.remove(RecordKey::from(Bytes::from_static(b"k2")));

    let snapshot1 = staging.snapshot_ops();
    let snapshot2 = staging.snapshot_ops();
    assert_eq!(snapshot1.len(), 2);
    assert_eq!(snapshot2.len(), 2, "snapshot_ops must NOT consume");
    assert_eq!(staging.len(), 2);
}

#[tokio::test]
async fn iter_ops_yields_same_content_as_snapshot_ops() {
    // `iter_ops()` must be a drop-in, non-materializing replacement for
    // `snapshot_ops()`: same items, same order, no consumption.
    let base: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut staging = StagingStore::new(base);
    staging.set(
        RecordKey::from(Bytes::from_static(b"k1")),
        Bytes::from_static(b"v1"),
    );
    staging.remove(RecordKey::from(Bytes::from_static(b"k2")));

    let via_snapshot: Vec<KvOp> = staging.snapshot_ops();
    let via_iter: Vec<KvOp> = staging.iter_ops().collect();
    assert_eq!(via_snapshot.len(), via_iter.len());
    for (a, b) in via_snapshot.iter().zip(via_iter.iter()) {
        match (a, b) {
            (KvOp::Set(ka, va), KvOp::Set(kb, vb)) => {
                assert_eq!(ka.as_ref(), kb.as_ref());
                assert_eq!(va, vb);
            }
            (KvOp::Remove(ka), KvOp::Remove(kb)) => assert_eq!(ka.as_ref(), kb.as_ref()),
            _ => panic!("snapshot_ops and iter_ops disagree on op kind: {a:?} vs {b:?}"),
        }
    }
    // iter_ops must not consume either — staging is still fully intact.
    assert_eq!(staging.len(), 2);
}

/// P1 perf regression (task-group 7, shamir-engine cross-crate review):
/// `ValidatorDb::staged_field_matches` / `fk_restrict::staged_field_matches` /
/// `fk_on_update::staged_field_matches` call this staging probe once PER
/// RECORD validated in a batch insert against a table with a unique/FK rule.
/// Before the fix, every one of those calls went through
/// `staging.snapshot_ops().into_iter().any(...)`: `snapshot_ops()`'s
/// `.collect()` unconditionally materializes a fresh `Vec<KvOp>` covering
/// the ENTIRE staged set BEFORE `.any()` gets to scan (and possibly
/// short-circuit) it — so even a match sitting at position 0 still pays the
/// full O(staged) allocation cost. Summed across M probes against a
/// growing staged set, that is O(M²) allocation work.
///
/// This test stages a "conflicting" record FIRST (the realistic case: a
/// batch insert repeatedly violating a unique constraint against an
/// earlier row in the SAME batch), then simulates M probes — one per
/// inserted record — using both the OLD call shape (`snapshot_ops()`) and
/// the NEW one (`iter_ops()`), counting how many items each actually
/// visits/materializes in total across the M calls. It fails on the
/// pre-fix code (no `iter_ops` method exists to call — the API this test
/// exercises did not exist before this fix) and, more importantly, proves
/// the NEW call shape's total visited-item count stays LINEAR in M while
/// the OLD shape's total materialized-item count is exactly the quadratic
/// sum `Σ_{i=1}^{M} i`.
#[tokio::test]
async fn staged_probe_scales_linearly_not_quadratically_with_early_match() {
    let m: usize = 500;
    let base: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut staging = StagingStore::new(base);

    // The conflicting record every probe below must find.
    staging.set(
        RecordKey::from(Bytes::from_static(b"k_conflict")),
        Bytes::from_static(b"MATCH"),
    );

    let mut old_total_materialized = 0usize;
    let mut new_total_visited = 0usize;

    for i in 0..m {
        // OLD call-site shape: `staging.snapshot_ops().into_iter().any(...)`.
        // The `.len()` of the unconditionally-materialized Vec stands in for
        // the allocation/copy work `.collect()` pays regardless of where
        // (or whether) a match is found.
        old_total_materialized += staging.snapshot_ops().len();

        // NEW call-site shape: `staging.iter_ops().any(...)` — lazy, so
        // `.any()` stops at the first match instead of forcing a full pass.
        let mut visited_this_call = 0usize;
        let found = staging.iter_ops().any(|op| {
            visited_this_call += 1;
            matches!(op, KvOp::Set(_, ref v) if v.as_ref() == b"MATCH")
        });
        assert!(found, "conflicting record must be found on probe {i}");
        new_total_visited += visited_this_call;

        // Grow the staged set as a real batch insert would (this record's
        // own, non-conflicting, staged row) before the next probe.
        staging.set(
            RecordKey::from(Bytes::from(format!("k{i}"))),
            Bytes::from_static(b"NOMATCH"),
        );
    }

    let expected_quadratic = m * (m + 1) / 2; // Σ_{i=1}^{m} i
    assert_eq!(
        old_total_materialized, expected_quadratic,
        "snapshot_ops()-based probe must unconditionally materialize the \
         full staged set on every call — this IS the O(M²) allocation defect"
    );
    assert_eq!(
        new_total_visited, m,
        "iter_ops().any() must short-circuit on the first (matching) item \
         every time, keeping total visited work linear in M"
    );
    assert!(
        old_total_materialized > new_total_visited * (m / 10),
        "expected the old pattern's total materialized work ({old_total_materialized}) \
         to dwarf the new pattern's linear total ({new_total_visited}) for M={m}"
    );
}
