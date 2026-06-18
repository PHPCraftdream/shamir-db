//! GROUP BY hot loop bench. `apply_group_by` builds a key per record
//! to look up its group bucket. The baseline path is:
//!   resolve_field -> inner_to_query_value -> canonical_str
//!   -> push to Vec<String> -> Vec::join("|")
//!
//! That's a per-record string allocation just to drive a BTreeMap
//! lookup. We measure the full pipeline through `apply_group_by`
//! so the win — switching the keying to a hashable typed enum —
//! shows up as a wall-clock delta on the same input.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

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

fn bench(c: &mut Criterion) {
    let interner = Interner::new();
    for k in ["id", "age", "city", "score", "count"] {
        let _ = interner.touch_ind(k);
    }
    let records: Vec<(RecordId, InnerValue)> = (0..1000)
        .map(|i| (RecordId::new(), make_record(&interner, i)))
        .collect();
    let empty_refs: TMap<String, _> = new_map_wc(0);
    let ctx = FilterContext::new(&interner, &empty_refs);

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
    // The previous compute_aggregate path re-walked the group per aggregate
    // (5 walks × Vec<Option<InnerValue>> allocations + Count{All} cloning
    // the whole record). With 10 groups × 100 rows × 5 aggregates the cost
    // is visible above bench noise.
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

    // S4: `apply_group_by` now consumes `&[(RecordId, Bytes)]` (lens feed) —
    // encode the InnerValue fixtures to storage bytes ONCE, outside the timed
    // loop, so the bench measures the aggregate hot path (not encoding).
    let records: Vec<(RecordId, bytes::Bytes)> = records
        .into_iter()
        .map(|(id, v)| (id, v.to_bytes().expect("encode bench record")))
        .collect();

    let mut group = c.benchmark_group("apply_group_by");
    group.throughput(Throughput::Elements(records.len() as u64));

    group.bench_function("by_int_100_groups", |b| {
        b.iter(|| {
            black_box(apply_group_by(
                &records,
                &group_by_age,
                &select,
                &interner,
                &ctx,
            ))
        });
    });
    group.bench_function("by_str_10_groups", |b| {
        b.iter(|| {
            black_box(apply_group_by(
                &records,
                &group_by_city,
                &select,
                &interner,
                &ctx,
            ))
        });
    });
    group.bench_function("by_str_int_composite", |b| {
        b.iter(|| {
            black_box(apply_group_by(
                &records,
                &group_by_two,
                &select,
                &interner,
                &ctx,
            ))
        });
    });

    // Multi-aggregate scenario: 10 groups (by city) × 100 rows/group,
    // 5 aggregates per group. Tests the per-aggregate re-walk + clone cost.
    group.bench_function("multi_aggregate_5_funcs", |b| {
        b.iter(|| {
            black_box(apply_group_by(
                &records,
                &group_by_city,
                &select_multi,
                &interner,
                &ctx,
            ))
        });
    });

    // Many groups (100 by age) × 10 rows/group with multi-aggregate. Stresses
    // the Count{All} record-clone path (one clone per row per aggregate
    // in the baseline).
    group.bench_function("multi_aggregate_5_funcs_many_groups", |b| {
        b.iter(|| {
            black_box(apply_group_by(
                &records,
                &group_by_age,
                &select_multi,
                &interner,
                &ctx,
            ))
        });
    });

    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
