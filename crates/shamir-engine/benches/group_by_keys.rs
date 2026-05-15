//! GROUP BY hot loop bench. `apply_group_by` builds a key per record
//! to look up its group bucket. The baseline path is:
//!   resolve_field -> inner_to_json_value -> json::Value::to_string
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
use shamir_query_types::read::GroupBy;
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
            ["Jerusalem", "Tel-Aviv", "Haifa", "Beer-Sheva", "Eilat",
             "Tiberias", "Nazareth", "Acre", "Ashdod", "Netanya"][(idx as usize) % 10]
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
        fields: vec![
            vec!["city".to_string()],
            vec!["age".to_string()],
        ],
        having: None,
    };

    let select = Select {
        items: vec![SelectItem::CountAll {
            alias: Some("count".to_string()),
        }],
        distinct: false,
    };

    let mut group = c.benchmark_group("apply_group_by");
    group.throughput(Throughput::Elements(records.len() as u64));

    group.bench_function("by_int_100_groups", |b| {
        b.iter(|| black_box(apply_group_by(&records, &group_by_age, &select, &interner, &ctx)));
    });
    group.bench_function("by_str_10_groups", |b| {
        b.iter(|| black_box(apply_group_by(&records, &group_by_city, &select, &interner, &ctx)));
    });
    group.bench_function("by_str_int_composite", |b| {
        b.iter(|| black_box(apply_group_by(&records, &group_by_two, &select, &interner, &ctx)));
    });

    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
