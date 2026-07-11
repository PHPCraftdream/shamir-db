//! Basic tests for `SortedIndexManager`:
//! register / drop_index, lifecycle hooks, lookups, persistence.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::legacy::sorted_index_manager::{SortedIndexDefinition, SortedIndexManager};

use super::helpers::{enc_i64, enc_str, fresh_mgr, record_with_int, record_with_str};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_makes_has_indexes_true() {
    let (_, mgr) = fresh_mgr().await;
    assert!(!mgr.has_indexes());
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    assert!(mgr.has_indexes());
}

#[tokio::test]
async fn find_by_field_returns_matching_definition() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201, 202]))
        .await
        .unwrap();
    mgr.register(SortedIndexDefinition::new(102, vec![203]))
        .await
        .unwrap();

    let def = mgr.find_by_field(&[201, 202]).expect("found");
    assert_eq!(def.name_interned, 101);
    assert!(mgr.find_by_field(&[999]).is_none());
}

#[tokio::test]
async fn on_record_created_then_lookup_range_inclusive() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();

    // Seed 10 records with scores 0..10.
    let mut ids = Vec::new();
    for score in 0..10i64 {
        let id = RecordId::new();
        let rec = record_with_int(201, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
        ids.push((score, id));
    }

    // Range [3 ..= 6] → 4 records.
    let result = mgr
        .lookup_range(101, Some(&enc_i64(3)), Some(&enc_i64(6)))
        .await
        .unwrap();
    assert_eq!(result.len(), 4);
    // Verify only the right ids are in the result set.
    for (score, id) in &ids {
        let expected = (3..=6).contains(score);
        assert_eq!(result.contains(id), expected, "score {score}");
    }
}

#[tokio::test]
async fn lookup_range_open_lower_bound() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    for score in 1..=5i64 {
        let id = RecordId::new();
        let rec = record_with_int(201, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
    }

    // (-∞ .. 3] → 1, 2, 3.
    let result = mgr
        .lookup_range(101, None, Some(&enc_i64(3)))
        .await
        .unwrap();
    assert_eq!(result.len(), 3);
}

#[tokio::test]
async fn lookup_range_open_upper_bound() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    for score in 1..=5i64 {
        let id = RecordId::new();
        let rec = record_with_int(201, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
    }

    // [3 .. ∞) → 3, 4, 5.
    let result = mgr
        .lookup_range(101, Some(&enc_i64(3)), None)
        .await
        .unwrap();
    assert_eq!(result.len(), 3);
}

#[tokio::test]
async fn lookup_range_fully_unbounded_returns_all() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    for score in 1..=7i64 {
        let id = RecordId::new();
        let rec = record_with_int(201, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
    }
    let result = mgr.lookup_range(101, None, None).await.unwrap();
    assert_eq!(result.len(), 7);
}

#[tokio::test]
async fn lookup_range_handles_negative_values() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let scores = [-50, -10, -1, 0, 1, 10, 50];
    for &s in &scores {
        let id = RecordId::new();
        let rec = record_with_int(201, s);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
    }

    // [-10 ..= 10] → five matches: -10, -1, 0, 1, 10.
    let result = mgr
        .lookup_range(101, Some(&enc_i64(-10)), Some(&enc_i64(10)))
        .await
        .unwrap();
    assert_eq!(result.len(), 5);
}

#[tokio::test]
async fn lookup_range_strings_lexicographic() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let words = ["alpha", "beta", "gamma", "delta", "epsilon"];
    for w in &words {
        let id = RecordId::new();
        let rec = record_with_str(201, w);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
    }

    // ["beta" ..= "gamma"] → beta, delta, epsilon, gamma (lex order).
    let result = mgr
        .lookup_range(101, Some(&enc_str("beta")), Some(&enc_str("gamma")))
        .await
        .unwrap();
    assert_eq!(result.len(), 4);
}

#[tokio::test]
async fn lookup_min_returns_smallest() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let mut min_id_for_score = None;
    for score in [50, 10, 30, 5, 20] {
        let id = RecordId::new();
        let rec = record_with_int(201, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
        if score == 5 {
            min_id_for_score = Some(id);
        }
    }
    let got = mgr.lookup_min(101).await.unwrap();
    assert_eq!(got, min_id_for_score);
}

#[tokio::test]
async fn lookup_min_empty_index_returns_none() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let got = mgr.lookup_min(101).await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn lookup_first_k_in_value_order() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let mut score_to_id = Vec::new();
    for score in [50, 10, 30, 5, 20, 40] {
        let id = RecordId::new();
        let rec = record_with_int(201, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
        score_to_id.push((score, id));
    }
    let got = mgr.lookup_first_k(101, 3).await.unwrap();
    assert_eq!(got.len(), 3);
    // Expect the three records with scores 5, 10, 20 — in that order.
    let mut expected = score_to_id.clone();
    expected.sort_by_key(|(s, _)| *s);
    let expected_ids: Vec<RecordId> = expected.iter().take(3).map(|(_, id)| *id).collect();
    assert_eq!(got, expected_ids);
}

// ── task #537: record-id tie-breaker on lookup_range_first_k_page ──────────

/// Build a RecordId whose 16 bytes are all `b` — gives a deterministic,
/// controllable byte ordering for tie-breaker tests (unlike time-based ids).
fn rid_byte(b: u8) -> RecordId {
    RecordId([b; 16])
}

/// ASC first page WITH a tie-breaker: rows tied on the seek value are skipped
/// ONLY when their record_id is `<= after_id`; ties strictly past it are
/// returned. Without a tie-breaker (`after_id = None`) ALL ties are skipped
/// (backward-compatible behavior). This is the core #537 index-layer fix.
#[tokio::test]
async fn lookup_range_first_k_page_asc_tiebreaker_skips_only_up_to_id() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();

    // Three rows tied at value 20 with controlled ascending ids, plus a 30.
    let id20_a = rid_byte(0x10);
    let id20_b = rid_byte(0x20);
    let id20_c = rid_byte(0x30);
    let id30 = rid_byte(0x40);
    for (id, score) in [
        (id20_a, 20i64),
        (id20_b, 20),
        (id20_c, 20),
        (id30, 30),
    ] {
        mgr.on_record_created(&id, &record_with_int(201, score), 1)
            .await
            .unwrap();
    }

    let seek = enc_i64(20);

    // No tie-breaker → skip ALL three 20s (today's behavior); only 30 left.
    let (ids, _) = mgr
        .lookup_range_first_k_page(101, &seek, None, None, 10, true)
        .await
        .unwrap();
    assert_eq!(ids, vec![id30], "no tie-breaker must skip all ties");

    // Tie-breaker = id20_b → skip 20a, 20b (<= b); return 20c then 30.
    let (ids, _) = mgr
        .lookup_range_first_k_page(101, &seek, None, Some(&id20_b), 10, true)
        .await
        .unwrap();
    assert_eq!(
        ids,
        vec![id20_c, id30],
        "tie-breaker must return only ties strictly past after_id"
    );
}

/// DESC mirror: ties skipped only when record_id `>= after_id`; ties strictly
/// below it (walking high→low) are returned.
#[tokio::test]
async fn lookup_range_first_k_page_desc_tiebreaker_skips_only_from_id() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();

    let id20_a = rid_byte(0x10);
    let id20_b = rid_byte(0x20);
    let id20_c = rid_byte(0x30);
    let id10 = rid_byte(0x05);
    for (id, score) in [
        (id20_a, 20i64),
        (id20_b, 20),
        (id20_c, 20),
        (id10, 10),
    ] {
        mgr.on_record_created(&id, &record_with_int(201, score), 1)
            .await
            .unwrap();
    }

    let seek = enc_i64(20);

    // No tie-breaker → skip all three 20s; only 10 remains (DESC).
    let (ids, _) = mgr
        .lookup_range_first_k_page(101, &seek, None, None, 10, false)
        .await
        .unwrap();
    assert_eq!(ids, vec![id10], "no tie-breaker must skip all ties (DESC)");

    // Tie-breaker = id20_b → skip 20c, 20b (>= b); return 20a then 10 (DESC).
    let (ids, _) = mgr
        .lookup_range_first_k_page(101, &seek, None, Some(&id20_b), 10, false)
        .await
        .unwrap();
    assert_eq!(
        ids,
        vec![id20_a, id10],
        "DESC tie-breaker must return only ties strictly below after_id"
    );
}

#[tokio::test]
async fn lookup_first_k_zero_returns_empty() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let id = RecordId::new();
    let rec = record_with_int(201, 42);
    mgr.on_record_created(&id, &rec, 1).await.unwrap();
    let got = mgr.lookup_first_k(101, 0).await.unwrap();
    assert!(got.is_empty());
}

#[tokio::test]
async fn lookup_max_returns_largest() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let mut max_id_for_score = None;
    for score in [50, 10, 30, 5, 20] {
        let id = RecordId::new();
        let rec = record_with_int(201, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
        if score == 50 {
            max_id_for_score = Some(id);
        }
    }
    let got = mgr.lookup_max(101).await.unwrap();
    assert_eq!(got, max_id_for_score);
}

#[tokio::test]
async fn lookup_max_empty_index_returns_none() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let got = mgr.lookup_max(101).await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn lookup_last_k_in_value_desc_order() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let mut score_to_id = Vec::new();
    for score in [50, 10, 30, 5, 20, 40] {
        let id = RecordId::new();
        let rec = record_with_int(201, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
        score_to_id.push((score, id));
    }
    let got = mgr.lookup_last_k(101, 3).await.unwrap();
    assert_eq!(got.len(), 3);
    // Expect the three records with scores 50, 40, 30 — in that order.
    let mut expected = score_to_id.clone();
    expected.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    let expected_ids: Vec<RecordId> = expected.iter().take(3).map(|(_, id)| *id).collect();
    assert_eq!(got, expected_ids);
}

#[tokio::test]
async fn lookup_last_k_zero_returns_empty() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let id = RecordId::new();
    let rec = record_with_int(201, 42);
    mgr.on_record_created(&id, &rec, 1).await.unwrap();
    let got = mgr.lookup_last_k(101, 0).await.unwrap();
    assert!(got.is_empty());
}

#[tokio::test]
async fn lookup_last_k_more_than_present_returns_all_in_desc() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let mut ids_by_score = Vec::new();
    for score in [3, 1, 2] {
        let id = RecordId::new();
        let rec = record_with_int(201, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
        ids_by_score.push((score, id));
    }
    let got = mgr.lookup_last_k(101, 100).await.unwrap();
    assert_eq!(got.len(), 3);
    let mut expected = ids_by_score.clone();
    expected.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    let expected_ids: Vec<RecordId> = expected.iter().map(|(_, id)| *id).collect();
    assert_eq!(got, expected_ids);
}

#[tokio::test]
async fn on_record_updated_moves_the_entry() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let id = RecordId::new();
    let old = record_with_int(201, 10);
    let new = record_with_int(201, 100);
    mgr.on_record_created(&id, &old, 1).await.unwrap();
    mgr.on_record_updated(&id, &old, &new, 2).await.unwrap();

    // Old slot is empty; new slot contains the id.
    let r_old = mgr
        .lookup_range(101, Some(&enc_i64(10)), Some(&enc_i64(10)))
        .await
        .unwrap();
    assert!(r_old.is_empty(), "old entry must have been removed");
    let r_new = mgr
        .lookup_range(101, Some(&enc_i64(100)), Some(&enc_i64(100)))
        .await
        .unwrap();
    assert!(r_new.contains(&id), "new entry must be present");
}

#[tokio::test]
async fn on_record_updated_with_same_value_is_noop() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let id = RecordId::new();
    let rec = record_with_int(201, 10);
    mgr.on_record_created(&id, &rec, 1).await.unwrap();
    // Identical "old" and "new" — same encoded value, skip.
    mgr.on_record_updated(&id, &rec, &rec, 2).await.unwrap();

    let r = mgr
        .lookup_range(101, Some(&enc_i64(10)), Some(&enc_i64(10)))
        .await
        .unwrap();
    assert_eq!(r.len(), 1, "entry must still be there once");
}

#[tokio::test]
async fn on_record_deleted_removes_entry() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let id = RecordId::new();
    let rec = record_with_int(201, 42);
    mgr.on_record_created(&id, &rec, 1).await.unwrap();
    mgr.on_record_deleted(&id, &rec).await.unwrap();
    let r = mgr
        .lookup_range(101, Some(&enc_i64(42)), Some(&enc_i64(42)))
        .await
        .unwrap();
    assert!(r.is_empty(), "entry must have been removed on delete");
}

#[tokio::test]
async fn missing_field_is_skipped_silently() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let id = RecordId::new();
    // Record has field 202, not the indexed field 201.
    let mut m = new_map();
    m.insert(InternerKey::new(202), InnerValue::Int(99));
    let rec = InnerValue::Map(m);
    // Must not error; just no entry written.
    mgr.on_record_created(&id, &rec, 1).await.unwrap();
    let r = mgr.lookup_range(101, None, None).await.unwrap();
    assert!(
        r.is_empty(),
        "no entry for record missing the indexed field"
    );
}

#[tokio::test]
async fn drop_index_removes_definition_and_entries() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    for score in 1..=5i64 {
        let id = RecordId::new();
        let rec = record_with_int(201, score);
        mgr.on_record_created(&id, &rec, 1).await.unwrap();
    }
    let dropped = mgr.drop_index(101).await.unwrap();
    assert!(dropped);
    assert!(!mgr.has_indexes());
    // Re-register the same name — should start empty.
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let r = mgr.lookup_range(101, None, None).await.unwrap();
    assert!(r.is_empty(), "no entries after drop+re-register");
}

#[tokio::test]
async fn drop_index_nonexistent_returns_false() {
    let (_, mgr) = fresh_mgr().await;
    assert!(!mgr.drop_index(9999).await.unwrap());
}

#[tokio::test]
async fn definitions_reload_on_fresh_manager() {
    // Create, register, then build a NEW manager backed by the same
    // info_store — definitions should reload.
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    {
        let mgr = SortedIndexManager::new(Arc::clone(&info_store))
            .await
            .unwrap();
        mgr.register(SortedIndexDefinition::new(101, vec![201, 202]))
            .await
            .unwrap();
        mgr.register(SortedIndexDefinition::new(102, vec![300]))
            .await
            .unwrap();
    }

    let mgr2 = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    assert!(mgr2.has_indexes());
    assert!(mgr2.find_by_field(&[201, 202]).is_some());
    assert!(mgr2.find_by_field(&[300]).is_some());
}

#[tokio::test]
async fn empty_manager_lookup_range_is_empty() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();
    let r = mgr.lookup_range(101, None, None).await.unwrap();
    assert!(r.is_empty());
}

/// Regression: an `Eq` query against a string column whose value is
/// a prefix of another column's value must not match the longer one.
/// Pre-fix the encoder appended raw UTF-8 with no terminator, so
/// `prefix||"a"||rid_X` could sort within the bounds we built for
/// `"aa"` and vice versa — the range scan returned wrong records.
#[tokio::test]
async fn string_prefix_does_not_match_longer_value() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(101, vec![201]))
        .await
        .unwrap();

    // Insert two records: one with value "a", one with value "aa".
    let rid_a = RecordId::new();
    let rid_aa = RecordId::new();
    mgr.on_record_created(&rid_a, &record_with_str(201, "a"), 1)
        .await
        .unwrap();
    mgr.on_record_created(&rid_aa, &record_with_str(201, "aa"), 1)
        .await
        .unwrap();

    // Range "a"..="a" (i.e. Eq("a")) must return only rid_a, NOT rid_aa.
    let bound_a = enc_str("a");
    let r = mgr
        .lookup_range(101, Some(&bound_a), Some(&bound_a))
        .await
        .unwrap();
    assert!(r.contains(&rid_a), "Eq(\"a\") missed rid_a");
    assert!(
        !r.contains(&rid_aa),
        "Eq(\"a\") incorrectly matched rid_aa — sorted index leaked across string boundary"
    );
    assert_eq!(
        r.len(),
        1,
        "Eq(\"a\") returned {} records, expected 1",
        r.len()
    );

    // Range "aa"..="aa" must return only rid_aa.
    let bound_aa = enc_str("aa");
    let r = mgr
        .lookup_range(101, Some(&bound_aa), Some(&bound_aa))
        .await
        .unwrap();
    assert_eq!(r.len(), 1);
    assert!(r.contains(&rid_aa));
}
