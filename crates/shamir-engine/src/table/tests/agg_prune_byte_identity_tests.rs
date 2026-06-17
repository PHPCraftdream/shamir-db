//! AGG #54 — byte-identity tests for the per-row tree decode prune.
//!
//! The prune (in [`crate::table::read_exec`]) replaces the full
//! `InnerValue::from_bytes(record_bytes)` decode on the aggregate / GROUP BY
//! read path with a decode of ONLY the referenced top-level subtrees, gated so
//! it only fires when the referenced-id set is provably complete + concrete.
//!
//! These tests prove the observable contract: for every qualifying query, the
//! aggregate RESULT computed from the pruned tree equals the result computed
//! from the full-decode tree (byte-identical JSON). They also prove the GATE
//! falls back to `None` (→ full decode) for every non-qualifying shape, and
//! that the full-decode path still produces correct results for those shapes.
//!
//! The pruned and full-decode trees are fed through the SAME aggregate
//! pipeline (`apply_group_by` / `apply_aggregate_all`), which is itself
//! unchanged — so a matching result is direct evidence that `resolve_field_ref`,
//! `group_key_item`, Min/Max borrows and funclib all behave identically on the
//! pruned tree.

use serde_json::json;

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{apply_aggregate_all, apply_group_by, GroupBy, Select, SelectItem};
use crate::table::read_exec::{collect_referenced_top_ids, prune_to_inner};
use shamir_query_builder::select;
use shamir_query_types::read::{AggFunc, AggregateField};
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::record_view::RecordView;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

// ============================================================================
// Harness — wide records with mostly-unreferenced fields + a nested subtree.
// ============================================================================

/// Intern a string, returning its u64 id.
fn intern(interner: &Interner, s: &str) -> u64 {
    match interner.touch_ind(s) {
        Ok(TouchInd::New(k)) | Ok(TouchInd::Exists(k)) => k.id(),
        Err(e) => panic!("intern failed: {}", e),
    }
}

/// Build a nested-map value: `{ age: Int, city: Str }`.
fn profile_map(interner: &Interner, age: i64, city: &str) -> InnerValue {
    let mut m = new_map();
    m.insert(
        InternerKey::new(intern(interner, "age")),
        InnerValue::Int(age),
    );
    m.insert(
        InternerKey::new(intern(interner, "city")),
        InnerValue::Str(city.into()),
    );
    InnerValue::Map(m)
}

/// Build a WIDE record with 10 top-level fields. Aggregate queries below
/// reference only `dept`, `salary`, `profile.age`, `profile.city` — fields
/// `big1`..`big5` (large strings / ints) are unreferenced and must NEVER be
/// decoded on the pruned path.
fn make_wide_record(
    interner: &Interner,
    dept: &str,
    salary: i64,
    profile_age: i64,
    profile_city: &str,
) -> InnerValue {
    let mut m = new_map();
    m.insert(
        InternerKey::new(intern(interner, "dept")),
        InnerValue::Str(dept.into()),
    );
    m.insert(
        InternerKey::new(intern(interner, "salary")),
        InnerValue::Int(salary),
    );
    m.insert(
        InternerKey::new(intern(interner, "profile")),
        profile_map(interner, profile_age, profile_city),
    );
    // Wide unreferenced fields — large enough that decoding them is wasteful.
    for i in 1..=5 {
        let key = format!("big{}", i);
        m.insert(
            InternerKey::new(intern(interner, &key)),
            InnerValue::Str("x".repeat(200)),
        );
    }
    m.insert(
        InternerKey::new(intern(interner, "active")),
        InnerValue::Bool(true),
    );
    m.insert(
        InternerKey::new(intern(interner, "score")),
        InnerValue::F64(1.5),
    );
    InnerValue::Map(m)
}

/// Build the battery of wide records, each serialised to storage bytes.
/// Returns `Vec<(RecordId, full_inner, bytes)>` so each test can decode both
/// ways from the SAME bytes.
fn make_wide_battery(interner: &Interner) -> Vec<(RecordId, InnerValue, Vec<u8>)> {
    let rows = [
        ("eng", 100, 30, "NYC"),
        ("eng", 200, 35, "LA"),
        ("eng", 150, 25, "NYC"),
        ("sales", 90, 40, "LA"),
        ("sales", 110, 45, "NYC"),
    ];
    rows.iter()
        .map(|(d, s, a, c)| {
            let inner = make_wide_record(interner, d, *s, *a, c);
            let bytes = inner.to_bytes().expect("to_bytes").to_vec();
            (RecordId::new(), inner, bytes)
        })
        .collect()
}

/// Full-decode a battery into `Vec<(RecordId, InnerValue)>`.
fn decode_full(battery: &[(RecordId, InnerValue, Vec<u8>)]) -> Vec<(RecordId, InnerValue)> {
    battery
        .iter()
        .map(|(id, _, bytes)| {
            let inner = InnerValue::from_bytes(bytes.as_slice()).expect("full decode");
            (*id, inner)
        })
        .collect()
}

/// Pruned-decode a battery into `Vec<(RecordId, InnerValue)>` using the
/// referenced-id set for `select` + `group_by`. Panics if the gate fell back
/// (`None`) — callers must only invoke this on qualifying queries.
fn decode_pruned(
    battery: &[(RecordId, InnerValue, Vec<u8>)],
    select: &Select,
    group_by: Option<&GroupBy>,
    interner: &Interner,
) -> Vec<(RecordId, InnerValue)> {
    let ids = collect_referenced_top_ids(select, group_by, interner)
        .expect("gate must succeed for qualifying queries");
    battery
        .iter()
        .map(|(id, _, bytes)| {
            let view = RecordView::new(bytes.as_slice()).expect("RecordView");
            (*id, prune_to_inner(&view, &ids))
        })
        .collect()
}

fn ctx_for<'a>(
    interner: &'a Interner,
    refs: &'a shamir_types::types::common::TMap<String, crate::query::read::QueryResult>,
) -> FilterContext<'a> {
    FilterContext::new(interner, refs)
}

// ============================================================================
// Qualifying cases — pruned result MUST equal full-decode result.
// ============================================================================

#[test]
fn prune_group_by_dept_count_salary_sum() {
    let interner = Interner::default();
    let battery = make_wide_battery(&interner);
    let group_by = GroupBy::new(["dept"]);
    let select = Select {
        items: vec![
            select::field("dept"),
            select::count_all("cnt"),
            select::sum("salary", "total_salary"),
        ],
        distinct: false,
    };
    let refs = new_map();
    let ctx = ctx_for(&interner, &refs);

    let full = decode_full(&battery);
    let pruned = decode_pruned(&battery, &select, Some(&group_by), &interner);

    let res_full = apply_group_by(&full, &group_by, &select, &interner, &ctx);
    let res_pruned = apply_group_by(&pruned, &group_by, &select, &interner, &ctx);

    assert_eq!(
        serde_json::to_value(&res_full).unwrap(),
        serde_json::to_value(&res_pruned).unwrap(),
        "pruned result must equal full-decode result"
    );
    // Sanity: concrete expected values.
    assert_eq!(res_full.len(), 2);
}

#[test]
fn prune_group_by_min_max_avg() {
    let interner = Interner::default();
    let battery = make_wide_battery(&interner);
    let group_by = GroupBy::new(["dept"]);
    let select = Select {
        items: vec![
            select::field("dept"),
            select::min("salary", "min_sal"),
            select::max("salary", "max_sal"),
            select::avg("salary", "avg_sal"),
        ],
        distinct: false,
    };
    let refs = new_map();
    let ctx = ctx_for(&interner, &refs);

    let full = decode_full(&battery);
    let pruned = decode_pruned(&battery, &select, Some(&group_by), &interner);

    let res_full = apply_group_by(&full, &group_by, &select, &interner, &ctx);
    let res_pruned = apply_group_by(&pruned, &group_by, &select, &interner, &ctx);

    assert_eq!(
        serde_json::to_value(&res_full).unwrap(),
        serde_json::to_value(&res_pruned).unwrap(),
    );
    // Min/Max exercise the `&'a InnerValue` borrow path — confirm concrete values.
    let res_json: Vec<serde_json::Value> = res_full
        .iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect();
    let eng = res_json.iter().find(|o| o["dept"] == "eng").unwrap();
    assert_eq!(eng["min_sal"], 100);
    assert_eq!(eng["max_sal"], 200);
}

#[test]
fn prune_group_by_count_field() {
    let interner = Interner::default();
    let battery = make_wide_battery(&interner);
    let group_by = GroupBy::new(["dept"]);
    let select = Select {
        items: vec![select::field("dept"), select::count("salary", "n")],
        distinct: false,
    };
    let refs = new_map();
    let ctx = ctx_for(&interner, &refs);

    let full = decode_full(&battery);
    let pruned = decode_pruned(&battery, &select, Some(&group_by), &interner);

    let res_full = apply_group_by(&full, &group_by, &select, &interner, &ctx);
    let res_pruned = apply_group_by(&pruned, &group_by, &select, &interner, &ctx);

    assert_eq!(
        serde_json::to_value(&res_full).unwrap(),
        serde_json::to_value(&res_pruned).unwrap(),
    );
}

#[test]
fn prune_group_by_nested_path_profile_age() {
    // GROUP BY profile.age (nested path → keep top-level `profile` subtree).
    // NOTE: GroupBy::new does NOT split on ".", so a nested path must be
    // constructed directly as fields = vec![vec!["profile", "age"]].
    let interner = Interner::default();
    let battery = make_wide_battery(&interner);
    let group_by = GroupBy {
        fields: vec![vec!["profile".to_string(), "age".to_string()]],
        having: None,
    };
    let select = Select {
        items: vec![
            select::field(vec!["profile", "age"]),
            select::count_all("cnt"),
        ],
        distinct: false,
    };
    let refs = new_map();
    let ctx = ctx_for(&interner, &refs);

    let full = decode_full(&battery);
    let pruned = decode_pruned(&battery, &select, Some(&group_by), &interner);

    let res_full = apply_group_by(&full, &group_by, &select, &interner, &ctx);
    let res_pruned = apply_group_by(&pruned, &group_by, &select, &interner, &ctx);

    assert_eq!(
        serde_json::to_value(&res_full).unwrap(),
        serde_json::to_value(&res_pruned).unwrap(),
    );
}

#[test]
fn prune_group_by_nested_agg_profile_city_min_age() {
    // Mixed: GROUP BY top-level `dept`, aggregate over nested `profile.age`,
    // project nested `profile.city` — all resolve through the kept `profile`
    // subtree + `dept` top-level field.
    let interner = Interner::default();
    let battery = make_wide_battery(&interner);
    let group_by = GroupBy::new(["dept"]);
    let select = Select {
        items: vec![
            select::field("dept"),
            select::min(vec!["profile", "age"], "min_age"),
            select::field(vec!["profile", "city"]),
        ],
        distinct: false,
    };
    let refs = new_map();
    let ctx = ctx_for(&interner, &refs);

    let full = decode_full(&battery);
    let pruned = decode_pruned(&battery, &select, Some(&group_by), &interner);

    let res_full = apply_group_by(&full, &group_by, &select, &interner, &ctx);
    let res_pruned = apply_group_by(&pruned, &group_by, &select, &interner, &ctx);

    assert_eq!(
        serde_json::to_value(&res_full).unwrap(),
        serde_json::to_value(&res_pruned).unwrap(),
    );
}

#[test]
fn prune_aggregate_all_sum_avg_min_max() {
    // No GROUP BY — aggregate over the entire set.
    let interner = Interner::default();
    let battery = make_wide_battery(&interner);
    let select = Select {
        items: vec![
            select::sum("salary", "total"),
            select::avg("salary", "avg_sal"),
            select::min("salary", "min_sal"),
            select::max("salary", "max_sal"),
            select::count("salary", "n"),
        ],
        distinct: false,
    };

    let full = decode_full(&battery);
    let pruned = decode_pruned(&battery, &select, None, &interner);

    let res_full = apply_aggregate_all(&full, &select, &interner);
    let res_pruned = apply_aggregate_all(&pruned, &select, &interner);

    assert_eq!(
        serde_json::to_value(&res_full).unwrap(),
        serde_json::to_value(&res_pruned).unwrap(),
    );
    // Concrete sanity.
    let res0_json = serde_json::to_value(&res_full[0]).unwrap();
    assert_eq!(res0_json["total"], 650);
}

#[test]
fn prune_group_by_agg_fn_median() {
    // funclib aggregate (AggregateFn) over a concrete field — still prunable.
    let interner = Interner::default();
    let battery = make_wide_battery(&interner);
    let group_by = GroupBy::new(["dept"]);
    let select = Select {
        items: vec![
            select::field("dept"),
            select::agg_fn("median", "salary", "med_sal"),
        ],
        distinct: false,
    };
    let refs = new_map();
    let ctx = ctx_for(&interner, &refs);

    let full = decode_full(&battery);
    let pruned = decode_pruned(&battery, &select, Some(&group_by), &interner);

    let res_full = apply_group_by(&full, &group_by, &select, &interner, &ctx);
    let res_pruned = apply_group_by(&pruned, &group_by, &select, &interner, &ctx);

    assert_eq!(
        serde_json::to_value(&res_full).unwrap(),
        serde_json::to_value(&res_pruned).unwrap(),
    );
}

#[test]
fn prune_unreferenced_fields_absent_from_pruned_tree() {
    // Observable contract: the pruned tree for a qualifying query must NOT
    // contain the wide unreferenced fields (they were never decoded). We probe
    // one pruned record directly.
    let interner = Interner::default();
    let battery = make_wide_battery(&interner);
    let select = Select {
        items: vec![select::sum("salary", "total")],
        distinct: false,
    };

    let ids = collect_referenced_top_ids(&select, None, &interner).unwrap();
    // Only `salary` should be referenced.
    let salary_id = intern(&interner, "salary");
    assert_eq!(ids.len(), 1, "only salary should be referenced");
    assert!(ids.contains(&salary_id));

    let big1_id = intern(&interner, "big1");
    assert!(!ids.contains(&big1_id), "big1 must not be referenced");

    // Decode one record pruned and confirm big1 is absent but salary present.
    let bytes = &battery[0].2;
    let view = RecordView::new(bytes.as_slice()).unwrap();
    let pruned = prune_to_inner(&view, &ids);
    match &pruned {
        InnerValue::Map(m) => {
            assert!(m.contains_key(&InternerKey::new(salary_id)));
            assert!(!m.contains_key(&InternerKey::new(big1_id)));
            assert_eq!(m.len(), 1, "pruned tree should have exactly 1 field");
        }
        other => panic!("pruned must be a Map, got {:?}", other),
    }
}

// ============================================================================
// FALL-BACK cases — gate MUST return None (→ full decode) and the full-decode
// path still produces correct results.
// ============================================================================

#[test]
fn fallback_select_star_returns_none() {
    let interner = Interner::default();
    // Force at least one field to be interned so the gate has something to
    // consider before hitting SelectItem::All.
    let _ = intern(&interner, "dept");
    let select = Select {
        items: vec![select::all()],
        distinct: false,
    };
    assert!(collect_referenced_top_ids(&select, None, &interner).is_none());
}

#[test]
fn fallback_aggregate_field_all_returns_none() {
    let interner = Interner::default();
    let _ = intern(&interner, "dept");
    // SUM(*) — AggregateField::All under SelectItem::Aggregate.
    let select = Select {
        items: vec![SelectItem::Aggregate {
            func: AggFunc::Sum,
            field: AggregateField::All,
            alias: Some("total".into()),
            distinct: false,
        }],
        distinct: false,
    };
    assert!(collect_referenced_top_ids(&select, None, &interner).is_none());
}

#[test]
fn fallback_aggregate_fn_field_all_returns_none() {
    let interner = Interner::default();
    let _ = intern(&interner, "dept");
    // funclib agg over (*) — AggregateField::All under SelectItem::AggregateFn.
    let select = Select {
        items: vec![SelectItem::AggregateFn {
            name: "median".into(),
            field: AggregateField::All,
            alias: Some("med".into()),
            distinct: false,
        }],
        distinct: false,
    };
    assert!(collect_referenced_top_ids(&select, None, &interner).is_none());
}

#[test]
fn fallback_scalar_function_returns_none() {
    let interner = Interner::default();
    let _ = intern(&interner, "dept");
    // A $fn scalar in projection — may read arbitrary fields → fall back.
    let select = Select {
        items: vec![
            select::field("dept"),
            select::func("upper_name", "strings/upper", Vec::new()),
        ],
        distinct: false,
    };
    assert!(collect_referenced_top_ids(&select, None, &interner).is_none());
}

#[test]
fn fallback_count_all_alone_qualifies_for_prune() {
    // CountAll never touches records → contributes no field id and is safe to
    // keep alongside pruned items. With ONLY CountAll the referenced set is
    // empty but the gate returns Some(empty), not None. (The actual decode
    // loop still runs because needs_raw=true; the pruned tree is an empty Map,
    // which is fine — CountAll short-circuits to group_records.len().)
    let interner = Interner::default();
    let select = Select {
        items: vec![select::count_all("cnt")],
        distinct: false,
    };
    let ids = collect_referenced_top_ids(&select, None, &interner).unwrap();
    assert!(ids.is_empty());
}

#[test]
fn fallback_full_decode_still_correct_on_select_star() {
    // The fall-back path uses full decode — verify it still produces correct
    // aggregate results for a shape that triggers fall-back. We can't easily
    // run SELECT * through apply_group_by (it doesn't handle All in aggregate
    // context), so use a query that mixes a prunable agg with a fall-back item
    // and confirm the GATE falls back (None) while the full-decode result is
    // still correct for the prunable portion.
    let interner = Interner::default();
    let battery = make_wide_battery(&interner);
    let group_by = GroupBy::new(["dept"]);
    // Mix: prunable `sum(salary)` + fall-back `SelectItem::All`.
    let select = Select {
        items: vec![
            select::field("dept"),
            select::sum("salary", "total"),
            select::all(),
        ],
        distinct: false,
    };
    // Gate must fall back.
    assert!(collect_referenced_top_ids(&select, Some(&group_by), &interner).is_none());

    // Full-decode path still produces correct aggregate output for the
    // prunable items (build_aggregate_object ignores SelectItem::All in group
    // context — see aggregate.rs line 337).
    let refs = new_map();
    let ctx = ctx_for(&interner, &refs);
    let full = decode_full(&battery);
    let res = apply_group_by(&full, &group_by, &select, &interner, &ctx);
    let res_json: Vec<serde_json::Value> = res
        .iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect();
    let eng = res_json.iter().find(|o| o["dept"] == "eng").unwrap();
    assert_eq!(eng["total"], json!(450)); // 100+200+150
}

#[test]
fn prune_group_by_missing_field_id_decodes_consistently() {
    // A GROUP BY on a field name that was never interned → gate falls back
    // (None), so the full-decode path is used. The result is the same as if
    // pruning had been attempted, because resolve_field_ref returns None for
    // the missing key either way — but the GATE must not silently drop it.
    let interner = Interner::default();
    let battery = make_wide_battery(&interner);
    let group_by = GroupBy::new(["nonexistent_field"]);
    let select = Select {
        items: vec![select::count_all("cnt")],
        distinct: false,
    };
    // Gate falls back because the group-by field isn't interned.
    assert!(collect_referenced_top_ids(&select, Some(&group_by), &interner).is_none());

    // Full-decode path: all rows collapse into one group (missing key →
    // GroupKeyItem::Missing for every row).
    let refs = new_map();
    let ctx = ctx_for(&interner, &refs);
    let full = decode_full(&battery);
    let res = apply_group_by(&full, &group_by, &select, &interner, &ctx);
    assert_eq!(res.len(), 1);
    let res0_json = serde_json::to_value(&res[0]).unwrap();
    assert_eq!(res0_json["cnt"], json!(battery.len() as i64));
}
