//! GROUP BY hot loop bench. `apply_group_by` builds a key per record
//! to look up its group bucket. The baseline path is:
//!   resolve_field -> inner_to_query_value -> canonical_str
//!   -> push to Vec<String> -> Vec::join("|")
//!
//! That's a per-record string allocation just to drive a BTreeMap
//! lookup. We measure the full pipeline through `apply_group_by`
//! so the win — switching the keying to a hashable typed enum —
//! shows up as a wall-clock delta on the same input.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): setup
//! (interner, records, group-by/select shapes) is built ONCE outside the
//! timed closure — plan 1 (shared setup). `Interner` / `FilterContext`
//! have no `Clone` impl and the harness's closures require `'static`, so
//! they are leaked to `'static` via `Box::leak` (a bench binary's process
//! lifetime makes this a harmless, deliberate trade for closure ergonomics).

use std::hint::black_box;

use bench_scale_tool::Harness;
use shamir_engine::query::filter::FilterContext;
use shamir_engine::query::read::exec::apply_group_by;
use shamir_engine::query::read::{Select, SelectItem};
use shamir_query_types::read::{AggFunc, AggregateField, GroupBy};
use shamir_types::core::interner::{Interner, TouchInd};
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

fn make_record(interner: &Interner, idx: u32) -> InnerValue {
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };
    let mut m = new_map_wc(6);
    m.insert(touch("id"), InnerValue::Int(idx as i64));
    m.insert(touch("age"), InnerValue::Int((idx % 100) as i64));
    m.insert(
        touch("city"),
        InnerValue::Str(
            [
                "Jerusalem",
                "Tel-Aviv",
                "Haifa",
                "Beer-Sheva",
                "Eilat",
                "Tiberias",
                "Nazareth",
                "Acre",
                "Ashdod",
                "Netanya",
            ][(idx as usize) % 10]
                .to_string(),
        ),
    );
    m.insert(touch("score"), InnerValue::F64(idx as f64));
    InnerValue::Map(m)
}

fn main() {
    let mut h = Harness::new("group_by_keys", env!("CARGO_MANIFEST_DIR"));

    let interner: &'static Interner = Box::leak(Box::new(Interner::new()));
    for k in ["id", "age", "city", "score", "count"] {
        let _ = interner.touch_ind(k);
    }
    let records: Vec<(RecordId, InnerValue)> = (0..1000)
        .map(|i| (RecordId::new(), make_record(interner, i)))
        .collect();
    let empty_refs: &'static TMap<String, _> = Box::leak(Box::new(new_map_wc(0)));
    let ctx: &'static FilterContext<'static> =
        Box::leak(Box::new(FilterContext::new(interner, empty_refs)));

    let group_by_age = GroupBy {
        fields: vec![vec!["age".to_string()]],
        having: None,
    };
    let group_by_city = GroupBy {
        fields: vec![vec!["city".to_string()]],
        having: None,
    };
    let group_by_two = GroupBy {
        fields: vec![vec!["city".to_string()], vec!["age".to_string()]],
        having: None,
    };

    let select = Select {
        items: vec![SelectItem::CountAll {
            alias: Some("count".to_string()),
        }],
        distinct: false,
    };

    // Multi-aggregate select: count(*) + sum(age) + avg(age) + min(age) + max(age).
    let select_multi = Select {
        items: vec![
            SelectItem::CountAll {
                alias: Some("count".to_string()),
            },
            SelectItem::Aggregate {
                func: AggFunc::Sum,
                field: AggregateField::Field(vec!["age".to_string()]),
                alias: Some("sum_age".to_string()),
                distinct: false,
            },
            SelectItem::Aggregate {
                func: AggFunc::Avg,
                field: AggregateField::Field(vec!["age".to_string()]),
                alias: Some("avg_age".to_string()),
                distinct: false,
            },
            SelectItem::Aggregate {
                func: AggFunc::Min,
                field: AggregateField::Field(vec!["age".to_string()]),
                alias: Some("min_age".to_string()),
                distinct: false,
            },
            SelectItem::Aggregate {
                func: AggFunc::Max,
                field: AggregateField::Field(vec!["age".to_string()]),
                alias: Some("max_age".to_string()),
                distinct: false,
            },
        ],
        distinct: false,
    };

    // S4: `apply_group_by` consumes `&[(RecordId, Bytes)]` (lens feed) —
    // encode the InnerValue fixtures to storage bytes ONCE, outside the
    // timed loop, so the bench measures the aggregate hot path (not encoding).
    let records: &'static Vec<(RecordId, bytes::Bytes)> = Box::leak(Box::new(
        records
            .into_iter()
            .map(|(id, v)| (id, v.to_bytes().expect("encode bench record")))
            .collect(),
    ));

    {
        let group_by_age = group_by_age.clone();
        let select = select.clone();
        h.bench("apply_group_by/by_int_100_groups", move || {
            black_box(apply_group_by(
                records,
                &group_by_age,
                &select,
                interner,
                ctx,
            ));
        });
    }
    {
        let group_by_city = group_by_city.clone();
        let select = select.clone();
        h.bench("apply_group_by/by_str_10_groups", move || {
            black_box(apply_group_by(
                records,
                &group_by_city,
                &select,
                interner,
                ctx,
            ));
        });
    }
    {
        let group_by_two = group_by_two.clone();
        let select = select.clone();
        h.bench("apply_group_by/by_str_int_composite", move || {
            black_box(apply_group_by(
                records,
                &group_by_two,
                &select,
                interner,
                ctx,
            ));
        });
    }

    // Multi-aggregate scenario: 10 groups (by city) × 100 rows/group,
    // 5 aggregates per group. Tests the per-aggregate re-walk + clone cost.
    {
        let group_by_city = group_by_city.clone();
        let select_multi = select_multi.clone();
        h.bench("apply_group_by/multi_aggregate_5_funcs", move || {
            black_box(apply_group_by(
                records,
                &group_by_city,
                &select_multi,
                interner,
                ctx,
            ));
        });
    }

    // Many groups (100 by age) × 10 rows/group with multi-aggregate. Stresses
    // the Count{All} record-clone path (one clone per row per aggregate
    // in the baseline).
    h.bench(
        "apply_group_by/multi_aggregate_5_funcs_many_groups",
        move || {
            black_box(apply_group_by(
                records,
                &group_by_age,
                &select_multi,
                interner,
                ctx,
            ));
        },
    );

    h.run();
}
