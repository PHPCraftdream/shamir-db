//! SELECT projection bench — no GROUP BY, no aggregates.
//!
//! `SelectProjection::project` is called once per record on every read
//! query. Hot loop allocates:
//!   - `resolve_field` clones the leaf (already optimised on the
//!     filter side via `resolve_field_ref` — projection still uses
//!     the owned variant);
//!   - `inner_to_json_value` walks the leaf into json::Value;
//!   - `key.to_string()` allocates the output map key per field
//!     per record (alias or last path segment).
//!
//! Bench drives `apply_select` over 1000 records, 5 selected fields.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

use serde_json as json;
use shamir_engine::query::read::exec::{apply_order_by, apply_pagination, apply_select};
use shamir_engine::query::read::{
    OrderBy, OrderByItem, OrderDirection, Pagination, Select, SelectItem,
};
use shamir_types::core::interner::{Interner, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

fn make_record(interner: &Interner, idx: u32) -> InnerValue {
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };
    let mut m = new_map_wc(10);
    m.insert(touch("id"), InnerValue::Int(idx as i64));
    m.insert(touch("name"), InnerValue::Str(format!("user-{}", idx)));
    m.insert(touch("age"), InnerValue::Int((idx % 100) as i64));
    m.insert(touch("score"), InnerValue::F64(idx as f64 * 1.5));
    m.insert(
        touch("email"),
        InnerValue::Str(format!("u{}@example.com", idx)),
    );
    m.insert(touch("city"), InnerValue::Str("Jerusalem".into()));
    m.insert(touch("active"), InnerValue::Bool(idx.is_multiple_of(2)));
    InnerValue::Map(m)
}

fn bench(c: &mut Criterion) {
    let interner = Interner::new();
    for k in ["id", "name", "age", "score", "email", "city", "active"] {
        let _ = interner.touch_ind(k);
    }
    let records: Vec<(RecordId, InnerValue)> = (0..1000)
        .map(|i| (RecordId::new(), make_record(&interner, i)))
        .collect();

    let select_5 = Select {
        items: vec![
            SelectItem::Field {
                path: vec!["id".to_string()],
                alias: None,
            },
            SelectItem::Field {
                path: vec!["name".to_string()],
                alias: None,
            },
            SelectItem::Field {
                path: vec!["age".to_string()],
                alias: None,
            },
            SelectItem::Field {
                path: vec!["score".to_string()],
                alias: None,
            },
            SelectItem::Field {
                path: vec!["email".to_string()],
                alias: None,
            },
        ],
        distinct: false,
    };

    let select_all = Select {
        items: vec![SelectItem::All],
        distinct: false,
    };

    let mut group = c.benchmark_group("apply_select");
    group.throughput(Throughput::Elements(1000));
    group.bench_function("5_fields_1000_records", |b| {
        b.iter(|| black_box(apply_select(&records, &select_5, &interner)))
    });
    group.bench_function("select_all_1000_records", |b| {
        b.iter(|| black_box(apply_select(&records, &select_all, &interner)))
    });
    group.finish();

    // Projected JSON for ORDER BY bench. Build once, clone per
    // iteration so the sort is the only measured work.
    let projected: Vec<json::Value> = apply_select(&records, &select_5, &interner);
    let order_by_single = OrderBy {
        items: vec![OrderByItem {
            field: vec!["age".to_string()],
            direction: OrderDirection::Asc,
            nulls: None,
        }],
    };
    let order_by_two = OrderBy {
        items: vec![
            OrderByItem {
                field: vec!["age".to_string()],
                direction: OrderDirection::Asc,
                nulls: None,
            },
            OrderByItem {
                field: vec!["name".to_string()],
                direction: OrderDirection::Asc,
                nulls: None,
            },
        ],
    };

    let mut g2 = c.benchmark_group("apply_order_by");
    g2.throughput(Throughput::Elements(1000));
    g2.bench_function("single_int_1000", |b| {
        b.iter_batched(
            || projected.clone(),
            |mut recs| {
                apply_order_by(&mut recs, &order_by_single);
                black_box(recs);
            },
            criterion::BatchSize::SmallInput,
        )
    });
    g2.bench_function("two_fields_1000", |b| {
        b.iter_batched(
            || projected.clone(),
            |mut recs| {
                apply_order_by(&mut recs, &order_by_two);
                black_box(recs);
            },
            criterion::BatchSize::SmallInput,
        )
    });
    g2.finish();

    // ── apply_pagination ────────────────────────────────────────
    let mut g3 = c.benchmark_group("apply_pagination");
    g3.throughput(Throughput::Elements(1000));
    g3.bench_function("skip_50_limit_100", |b| {
        b.iter_batched(
            || projected.clone(),
            |recs| {
                black_box(apply_pagination(
                    recs,
                    &Pagination::LimitOffset {
                        limit: Some(100),
                        offset: 50,
                    },
                    false,
                ));
            },
            criterion::BatchSize::SmallInput,
        )
    });
    g3.bench_function("limit_10_from_1000", |b| {
        b.iter_batched(
            || projected.clone(),
            |recs| {
                black_box(apply_pagination(
                    recs,
                    &Pagination::LimitOffset {
                        limit: Some(10),
                        offset: 0,
                    },
                    false,
                ));
            },
            criterion::BatchSize::SmallInput,
        )
    });
    g3.bench_function("count_total_1000", |b| {
        b.iter_batched(
            || projected.clone(),
            |recs| {
                black_box(apply_pagination(recs, &Pagination::None, true));
            },
            criterion::BatchSize::SmallInput,
        )
    });
    g3.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
