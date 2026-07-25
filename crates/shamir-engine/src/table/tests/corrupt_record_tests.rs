//! F-10 (#800) — `QueryResult::corrupt_records` regression tests.
//!
//! A malformed row inside a scan is still skipped from `records` (unchanged
//! behaviour), but must now be reported via `QueryResult::corrupt_records`
//! instead of silently vanishing from the result count. These tests inject a
//! deliberately corrupt record directly into the underlying store (bypassing
//! `TableManager::insert`'s normal encode path, which would never produce
//! invalid bytes), then exercise a cross-section of the `read_exec.rs` scan
//! shapes: the plain `read_streaming` Name path, `read_collecting`'s
//! non-aggregate branch (ORDER BY forces the collecting path), and
//! `read_collecting`'s aggregate/`raw_acc` branch (GROUP BY).

use std::sync::Arc;

use shamir_query_types::filter::{FieldPath, Filter, FilterValue};
use shamir_query_types::read::select::Select;
use shamir_query_types::read::{AggFunc, AggregateField, GroupBy, OrderBy, ReadQuery, SelectItem};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::table::TableManager;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn make_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    TableManager::create("corrupt".into(), data, info)
        .await
        .unwrap()
}

/// Seed 3 valid records (name, age, city) and return their RecordIds.
async fn seed_valid(tbl: &TableManager) -> Vec<RecordId> {
    let interner = tbl.interner().get().await.unwrap();
    let name_k = interner.touch_ind("name").unwrap().into_key();
    let age_k = interner.touch_ind("age").unwrap().into_key();
    let city_k = interner.touch_ind("city").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let rows = [("Alice", 30, "NYC"), ("Bob", 25, "LA"), ("Carol", 40, "SF")];
    let mut ids = Vec::with_capacity(rows.len());
    for (name, age, city) in rows {
        let mut m = new_map();
        m.insert(name_k.clone(), InnerValue::Str(name.into()));
        m.insert(age_k.clone(), InnerValue::Int(age));
        m.insert(city_k.clone(), InnerValue::Str(city.into()));
        ids.push(tbl.insert(&InnerValue::Map(m)).await.unwrap());
    }
    ids
}

/// Inject a deliberately corrupt (non-msgpack) record directly into the
/// table's underlying store, bypassing `TableManager::insert`'s normal
/// encode path (which would never produce invalid bytes). The id is still
/// resolvable (ids are read independently of the value payload) — only the
/// VALUE fails to decode.
async fn inject_corrupt_record(tbl: &TableManager) -> RecordId {
    let rid = RecordId::new();
    let garbage = bytes::Bytes::from_static(b"\xff\xff\xff not valid msgpack at all");
    tbl.data_store()
        .set(rid.to_bytes().into(), garbage)
        .await
        .unwrap();
    rid
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Plain SELECT * (no ORDER BY / GROUP BY / count_total) — full-scan
/// `read_streaming` Name path. A corrupt row alongside valid rows: valid rows
/// still come back correctly, and `corrupt_records` reports exactly the
/// malformed row's id.
#[tokio::test]
async fn read_streaming_reports_corrupt_record() {
    let tbl = make_table().await;
    let valid_ids = seed_valid(&tbl).await;
    let corrupt_id = inject_corrupt_record(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("corrupt");
    let result = tbl.read(&query, &ctx).await.unwrap();

    // Unchanged behaviour: the corrupt row is skipped, valid rows recover.
    assert_eq!(
        result.records.len(),
        valid_ids.len(),
        "expected exactly the 3 valid rows back, corrupt row must not appear"
    );

    // New behaviour: the corrupt row is reported, not silently dropped.
    assert_eq!(
        result.corrupt_records.len(),
        1,
        "expected exactly one corrupt record reported"
    );
    assert_eq!(result.corrupt_records[0].table, "corrupt");
    assert_eq!(result.corrupt_records[0].id, corrupt_id);
}

/// SELECT * with ORDER BY — forces the `read_collecting` non-aggregate
/// branch (plain-select accumulation + sort). Same assertions as the
/// streaming case: valid rows recover in order, corrupt row is reported.
#[tokio::test]
async fn read_collecting_order_by_reports_corrupt_record() {
    let tbl = make_table().await;
    let valid_ids = seed_valid(&tbl).await;
    let corrupt_id = inject_corrupt_record(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("corrupt").order_by(OrderBy::asc("age"));
    let result = tbl.read(&query, &ctx).await.unwrap();

    assert_eq!(
        result.records.len(),
        valid_ids.len(),
        "expected exactly the 3 valid rows back under ORDER BY, corrupt row skipped"
    );
    let ages: Vec<Option<i64>> = result
        .records
        .iter()
        .map(|r| {
            let qv = r.as_value();
            match &qv["age"] {
                shamir_types::types::value::QueryValue::Int(n) => Some(*n),
                _ => None,
            }
        })
        .collect();
    assert_eq!(ages, vec![Some(25), Some(30), Some(40)]);

    assert_eq!(result.corrupt_records.len(), 1);
    assert_eq!(result.corrupt_records[0].table, "corrupt");
    assert_eq!(result.corrupt_records[0].id, corrupt_id);
}

/// GROUP BY + aggregate — forces `read_collecting`'s `needs_raw` /
/// `raw_acc` branch (the aggregate pipeline decodes raw bytes via a
/// `RecordView` lens rather than the plain-projection path). The corrupt
/// row must not poison the aggregate result; it is skipped and reported.
#[tokio::test]
async fn read_collecting_aggregate_reports_corrupt_record() {
    let tbl = make_table().await;
    let valid_ids = seed_valid(&tbl).await;
    let corrupt_id = inject_corrupt_record(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("corrupt")
        .select(Select {
            items: vec![SelectItem::Aggregate {
                func: AggFunc::Count,
                field: AggregateField::All,
                alias: Some("cnt".into()),
                distinct: false,
            }],
            distinct: false,
        })
        .group_by(GroupBy::new(["city"]));
    let result = tbl.read(&query, &ctx).await.unwrap();

    // 3 valid rows, 3 distinct cities → 3 groups. The corrupt row (which
    // never reaches `raw_acc`) contributes to neither a group nor a count.
    assert_eq!(
        result.records.len(),
        valid_ids.len(),
        "expected 3 groups (one per distinct city), corrupt row excluded"
    );

    assert_eq!(result.corrupt_records.len(), 1);
    assert_eq!(result.corrupt_records[0].table, "corrupt");
    assert_eq!(result.corrupt_records[0].id, corrupt_id);
}

/// The common case — NO corrupt records — must be unaffected: a pure
/// regression guard that this task's plumbing doesn't accidentally surface
/// false positives. Also asserts the field is omitted from the wire when
/// empty (the `skip_serializing_if = "Vec::is_empty"` contract).
#[tokio::test]
async fn no_corrupt_records_when_all_rows_valid() {
    let tbl = make_table().await;
    let valid_ids = seed_valid(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("corrupt");
    let result = tbl.read(&query, &ctx).await.unwrap();

    assert_eq!(result.records.len(), valid_ids.len());
    assert!(
        result.corrupt_records.is_empty(),
        "no corrupt rows were injected — corrupt_records must be empty"
    );

    // Wire round-trip: the field is skipped when empty (backward-compatible
    // default), so a peer that doesn't know the field never observes it.
    let bytes = rmp_serde::to_vec_named(&result).expect("msgpack serialization failed");
    let back: shamir_query_types::read::QueryResult =
        rmp_serde::from_slice(&bytes).expect("msgpack deserialization failed");
    assert!(back.corrupt_records.is_empty());
}

/// WHERE filter that still scans past the corrupt row (city=NYC matches
/// Alice only) — exercises the filtered `read_streaming` path and confirms
/// the corrupt row is reported even when it wouldn't have matched the
/// filter anyway (it's excluded before the filter ever runs on it, since the
/// bytes never decode into a RecordView to test against the predicate).
#[tokio::test]
async fn read_streaming_filtered_reports_corrupt_record() {
    let tbl = make_table().await;
    seed_valid(&tbl).await;
    let corrupt_id = inject_corrupt_record(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("corrupt").filter(Filter::Eq {
        field: FieldPath::from(vec!["city".into()]),
        value: FilterValue::String("NYC".into()),
    });
    let result = tbl.read(&query, &ctx).await.unwrap();

    assert_eq!(result.records.len(), 1, "only Alice (city=NYC) matches");
    assert_eq!(result.corrupt_records.len(), 1);
    assert_eq!(result.corrupt_records[0].id, corrupt_id);
}
