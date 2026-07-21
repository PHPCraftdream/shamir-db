//! Differential test — the load-bearing safety net for F5 (item a).
//!
//! For every representative `TMap<String, QueryResult>` shape named in the
//! F5 brief, run BOTH the OLD path (`rmp_serde::to_vec_named` →
//! `from_slice::<QueryValue>` — the exact code that lived in
//! `query_runner.rs` pre-F5) AND the NEW direct conversion
//! (`to_query_value`) on the SAME input, and assert the resulting
//! `QueryValue`s are `PartialEq`-identical.
//!
//! Strategy A (custom `serde::Serializer`) handles every variant by driving
//! the existing `Serialize` impls verbatim — so this test confirms the
//! serializer correctly mirrors the msgpack round-trip's wire shape,
//! including the non-obvious cases:
//! - `Inserted` with a sorted-`_id` interleaving (keys both before and after
//!   `"_id"` alphabetically, plus the both-ends cases),
//! - `IdBytes` → `QueryValue::Bin` (NOT `Null`, as `as_value()` returns),
//! - `Dec`/`Big` → `Str` and `Set` → `List` (value-level coercions the
//!   round-trip applies),
//! - `skip_serializing_if` semantics for `stats`/`pagination`/`value`/
//!   `explain` (`None` omitted) and `skipped` (`false` omitted).

use shamir_query_types::read::{
    ExplainPlan, PaginationInfo, PlanType, QueryRecord, QueryResult, QueryStats,
};
use shamir_query_types::write::{ByteBuf, InsertedRecord};
use shamir_types::mpack;
use shamir_types::types::common::{new_map, new_set, TMap};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::query::batch::query_value_serializer::to_query_value;

// ── helpers ─────────────────────────────────────────────────────────────────

/// The EXACT old conversion (pre-F5 `query_runner.rs`): encode to msgpack
/// bytes, immediately re-parse as `QueryValue`.
fn old_round_trip(results: &TMap<String, QueryResult>) -> Option<QueryValue> {
    rmp_serde::to_vec_named(results)
        .ok()
        .and_then(|b| rmp_serde::from_slice::<QueryValue>(&b).ok())
}

/// Run BOTH paths on the same input and assert `PartialEq` parity.
fn assert_parity(label: &str, results: &TMap<String, QueryResult>) {
    let old = old_round_trip(results);
    let new = to_query_value(results).ok();
    assert_eq!(
        new, old,
        "{label}: new to_query_value must match old msgpack round-trip"
    );
}

/// Build a `TMap<String, QueryResult>` from alias/result pairs.
fn results_map(
    pairs: impl IntoIterator<Item = (&'static str, QueryResult)>,
) -> TMap<String, QueryResult> {
    let mut m: TMap<String, QueryResult> = new_map();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    m
}

/// Minimal `QueryResult` (only `records`, everything else default/absent).
fn qr(records: Vec<QueryRecord>) -> QueryResult {
    QueryResult {
        records,
        stats: None,
        pagination: None,
        value: None,
        explain: None,
        skipped: false,
        versions: None,
    }
}

// ── edge cases: empty map, multiple aliases ─────────────────────────────────

#[test]
fn empty_results_map() {
    let m: TMap<String, QueryResult> = new_map();
    assert_parity("empty map", &m);
}

#[test]
fn two_aliases() {
    let m = results_map([
        (
            "users",
            qr(vec![QueryRecord::Direct(
                mpack!({ "id": 1, "name": "alice" }),
            )]),
        ),
        ("count", qr(vec![QueryRecord::Direct(mpack!(7))])),
    ]);
    assert_parity("two aliases", &m);
}

// ── record variants: Direct / Inserted / IdBytes ────────────────────────────

#[test]
fn direct_record_nested_shapes() {
    // Nested Map, List, and scalar shapes inside Direct records.
    let records = vec![
        QueryRecord::Direct(mpack!({
            "id": 1,
            "name": "alice",
            "active": true,
            "nested": { "city": "NYC", "tags": ["a", "b", null] }
        })),
        QueryRecord::Direct(mpack!([1, "two", true, null, [9, 8, 7]])),
        QueryRecord::Direct(mpack!(42)),
        QueryRecord::Direct(mpack!(null)),
        QueryRecord::Direct(mpack!("bare string")),
        QueryRecord::Direct(mpack!(1.5)),
    ];
    let m = results_map([("reads", qr(records))]);
    assert_parity("direct nested", &m);
}

#[test]
fn inserted_record_with_id_midpoint_interleave() {
    // Field keys straddle "_id" alphabetically: "_created" < "_id" < "name"
    // < "qty". Exercises the sorted-`_id` midpoint-insert path.
    let mut fields = new_map();
    fields.insert("name".to_string(), QueryValue::Str("widget".into()));
    fields.insert("qty".to_string(), QueryValue::Int(42));
    fields.insert("_created".to_string(), QueryValue::Bool(true));
    let rec = QueryRecord::Inserted(InsertedRecord {
        id: Some(RecordId::system("rec-0001")),
        fields: QueryValue::Map(fields),
    });
    let m = results_map([("writes", qr(vec![rec]))]);
    assert_parity("inserted midpoint", &m);
}

#[test]
fn inserted_record_with_id_both_ends() {
    // All field keys AFTER "_id" → _id emitted FIRST (front-end insert).
    let front = QueryRecord::Inserted(InsertedRecord {
        id: Some(RecordId::system("rec-0002")),
        fields: mpack!({ "zzz": 1, "name": "x" }),
    });
    // All field keys BEFORE "_id" (uppercase / underscore-prefixed sorts that
    // precede "_id": "Aaa" < "_id" < ... and "_aaa" < "_id") → _id emitted
    // LAST (back-end insert).
    let back = QueryRecord::Inserted(InsertedRecord {
        id: Some(RecordId::system("rec-0003")),
        fields: mpack!({ "Aaa": 1, "_aaa": 2 }),
    });
    let m = results_map([("writes", qr(vec![front, back]))]);
    assert_parity("inserted both ends", &m);
}

#[test]
fn inserted_record_without_id() {
    // No _id injected — just sorted field pairs (UPDATE-RETURNING shape).
    let rec = QueryRecord::Inserted(InsertedRecord {
        id: None,
        fields: mpack!({ "name": "widget", "qty": 42 }),
    });
    let m = results_map([("updates", qr(vec![rec]))]);
    assert_parity("inserted no id", &m);
}

#[test]
fn id_bytes_becomes_bin_not_null() {
    // IdBytes must round-trip to QueryValue::Bin — NOT Null (as
    // `QueryRecord::as_value()` wrongly returns). This is the central
    // correctness trap the F5 brief warns about.
    let payload = vec![0x82u8, 0x01, 0xa3, b'a', b'b', b'c'];
    let rec = QueryRecord::IdBytes(ByteBuf::from(payload.clone()));
    let m = results_map([("ids", qr(vec![rec]))]);

    let old = old_round_trip(&m).expect("old round-trip ok");
    let new = to_query_value(&m).expect("new conversion ok");
    assert_eq!(new, old, "id_bytes: parity");

    // Walk records[0] in BOTH outputs and confirm Bin (not Null).
    let old_rec = &old["ids"]["records"][0];
    let new_rec = &new["ids"]["records"][0];
    match (old_rec, new_rec) {
        (QueryValue::Bin(ob), QueryValue::Bin(nb)) => {
            assert_eq!(ob, &payload, "old: IdBytes bin payload");
            assert_eq!(nb, &payload, "new: IdBytes bin payload");
        }
        (o, n) => panic!("IdBytes must be Bin in both paths; old={o:?} new={n:?}"),
    }
}

// ── value-level coercions: Dec → Str, Big → Str, Set → List ──────────────────

#[test]
fn dec_big_set_coercions_match_round_trip() {
    // Value::Dec / Value::Big serialize as strings; Value::Set serializes as
    // a seq. The round-trip therefore lands them as Str / Str / List — the
    // new serializer must match (it drives the same Serialize impls).
    let mut tag_set = new_set::<QueryValue>();
    tag_set.insert(QueryValue::Int(1));
    tag_set.insert(QueryValue::Int(2));

    let mut price_map = new_map();
    price_map.insert(
        "price".to_string(),
        QueryValue::Dec(rust_decimal::Decimal::new(314, 2)),
    );
    price_map.insert(
        "big".to_string(),
        QueryValue::Big(num_bigint::BigInt::from(999_999_999_999_999_i64)),
    );
    price_map.insert("tags".to_string(), QueryValue::Set(tag_set));

    let rec = QueryRecord::Direct(QueryValue::Map(price_map));
    let m = results_map([("coerced", qr(vec![rec]))]);
    assert_parity("dec/big/set coercions", &m);
}

// ── skip_serializing_if: stats / pagination / value / explain / skipped ─────

#[test]
fn optional_fields_present() {
    // Every Option field present — confirms nested struct (stats / pagination
    // / explain) serialization and the `value` field.
    let records = vec![QueryRecord::Direct(mpack!({ "id": 1 }))];
    let res = QueryResult {
        records,
        stats: Some(QueryStats {
            index_used: Some("idx_users".to_string()),
            records_scanned: 100,
            records_returned: 10,
            execution_time_us: 500,
        }),
        pagination: Some(PaginationInfo {
            total_count: Some(100),
            total_pages: Some(10),
            current_page: Some(1),
            page_size: Some(10),
            has_next: true,
            has_prev: false,
        }),
        value: Some(mpack!({ "computed": [1, 2, 3] })),
        explain: Some(ExplainPlan {
            plan_type: PlanType::IndexScan,
            index_used: Some("idx_users".to_string()),
            estimated_rows: Some(100),
        }),
        skipped: false,
        versions: None,
    };
    let m = results_map([("full", res)]);
    assert_parity("all optionals present", &m);
}

#[test]
fn optional_fields_absent() {
    // All Option fields None — confirms `skip_serializing_if = "Option::is_none"`
    // OMITS them (a None field must NOT appear as a null-valued key).
    let res = QueryResult {
        records: vec![QueryRecord::Direct(mpack!(1))],
        stats: None,
        pagination: None,
        value: None,
        explain: None,
        skipped: false,
        versions: None,
    };
    let m = results_map([("bare", res)]);
    assert_parity("all optionals absent", &m);

    // Directly confirm the QueryResult-as-map has ONLY the "records" key
    // (skipped=false is omitted too).
    let new = to_query_value(&m).expect("new ok");
    match &new["bare"] {
        QueryValue::Map(inner) => {
            let keys: Vec<&String> = inner.keys().collect();
            assert_eq!(
                keys,
                vec!["records"],
                "None fields + skipped=false must be absent"
            );
        }
        other => panic!("expected Map for QueryResult, got {other:?}"),
    }
}

#[test]
fn skipped_true_emitted() {
    // skip_serializing_if = "std::ops::Not::not": skipped=false is OMITTED,
    // skipped=true is EMITTED. Confirms the new path matches, not just
    // "represents bool correctly".
    let res_false = QueryResult {
        records: Vec::new(),
        stats: None,
        pagination: None,
        value: None,
        explain: None,
        skipped: false,
        versions: None,
    };
    let res_true = QueryResult {
        records: Vec::new(),
        stats: None,
        pagination: None,
        value: None,
        explain: None,
        skipped: true,
        versions: None,
    };
    let m = results_map([("a", res_false), ("b", res_true)]);
    assert_parity("skipped true/false", &m);

    // Confirm "b" carries a skipped:true key, "a" does not.
    let new = to_query_value(&m).expect("new ok");
    assert!(
        !matches!(&new["a"], QueryValue::Map(m) if m.contains_key("skipped")),
        "skipped=false must be absent"
    );
    assert!(
        matches!(&new["b"], QueryValue::Map(m) if m.get("skipped") == Some(&QueryValue::Bool(true))),
        "skipped=true must be present as Bool(true)"
    );
}

// ── combined kitchen-sink shape ─────────────────────────────────────────────

#[test]
fn kitchen_sink() {
    // One alias with a mix of ALL record variants + all optional fields, plus
    // a second alias — a single input exercising every branch at once.
    let mut ins_fields = new_map();
    ins_fields.insert("qty".to_string(), QueryValue::Int(7));
    ins_fields.insert("_created".to_string(), QueryValue::Bool(false));

    let rich = QueryResult {
        records: vec![
            QueryRecord::Direct(mpack!({ "id": 1, "name": "alice", "tags": ["x", "y"] })),
            QueryRecord::Inserted(InsertedRecord {
                id: Some(RecordId::system("k-001")),
                fields: QueryValue::Map(ins_fields),
            }),
            QueryRecord::Inserted(InsertedRecord {
                id: None,
                fields: mpack!({ "name": "no-id" }),
            }),
            QueryRecord::IdBytes(ByteBuf::from(vec![0x99u8, 0xAA, 0xBB])),
            QueryRecord::Direct(mpack!(null)),
        ],
        stats: Some(QueryStats {
            index_used: None,
            records_scanned: 5,
            records_returned: 5,
            execution_time_us: 1234,
        }),
        pagination: Some(PaginationInfo {
            total_count: None,
            total_pages: None,
            current_page: None,
            page_size: Some(5),
            has_next: false,
            has_prev: true,
        }),
        value: None,
        explain: Some(ExplainPlan {
            plan_type: PlanType::FullScan,
            index_used: None,
            estimated_rows: None,
        }),
        skipped: false,
        versions: None,
    };
    let simple = qr(vec![QueryRecord::Direct(mpack!("ok"))]);
    let m = results_map([("rich", rich), ("simple", simple)]);
    assert_parity("kitchen sink", &m);
}
