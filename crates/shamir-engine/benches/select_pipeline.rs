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

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

use serde_json as json;
use shamir_bench_utils as bu;
use shamir_engine::query::filter::eval_context::FilterContext;
use shamir_engine::query::read::exec::{apply_order_by, apply_pagination, apply_select};
use shamir_engine::query::read::{
    OrderBy, OrderByItem, OrderDirection, Pagination, ReadQuery, Select, SelectItem,
};
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::TableConfig;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::core::interner::{Interner, TouchInd};
use shamir_types::types::common::{new_map, new_map_wc};
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

// ============================================================================
// Opt #3a — end-to-end LIMIT push-down bench.
//
// Setup: a real `TableManager` (in-memory) with N records, all matching
// the WHERE clause. Bench two shapes of the SAME page-of-10 query:
//
//   `pushdown_active` — plain `WHERE … LIMIT 10` (no ORDER BY / DISTINCT /
//       GROUP BY / aggregates). With Opt #3a the read pipeline projects
//       only the 10 page rows.
//   `pushdown_disabled` — same WHERE / same LIMIT 10, but with
//       `ORDER BY name ASC` added. ORDER BY disables the gate, so the
//       pipeline must project every match before sorting + truncating —
//       this is the "before" baseline for the push-down.
//
// Both shapes touch the same N records / same index plan; the delta is
// the projection cost of `N - 10` rows. The ratio shows the asymptotic
// win as N grows.
//
// Note: ORDER BY here is NOT served by a sorted index (none defined on
// `name`), so the dispatcher uses `read_collecting`, NOT the
// `order_limit_fast_path`. This isolates the projection cost the
// push-down skips.
// ============================================================================

async fn build_table_with_n(n: usize) -> shamir_engine::table::TableManager {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new(
        "bench".into(),
        BoxRepo::InMemory(repo),
        vec![TableConfig::new("rows".to_string())],
    );
    let table = instance.get_table("rows").await.unwrap();
    let interner = table.interner().get().await.unwrap();

    // Pre-intern the field names so we can build interned records directly.
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };
    let k_id = touch("id");
    let k_name = touch("name");
    let k_age = touch("age");
    let k_score = touch("score");
    let k_email = touch("email");
    let k_city = touch("city");
    let k_status = touch("status");
    let v_jerusalem = InnerValue::Str("Jerusalem".into());
    let v_active = InnerValue::Str("active".into());

    for i in 0..n {
        let mut m = new_map_wc(10);
        m.insert(k_id.clone(), InnerValue::Int(i as i64));
        m.insert(k_name.clone(), InnerValue::Str(format!("user-{}", i)));
        m.insert(k_age.clone(), InnerValue::Int((i % 100) as i64));
        m.insert(k_score.clone(), InnerValue::F64(i as f64 * 1.5));
        m.insert(
            k_email.clone(),
            InnerValue::Str(format!("u{}@example.com", i)),
        );
        m.insert(k_city.clone(), v_jerusalem.clone());
        // status = "active" for every row → WHERE matches all N
        m.insert(k_status.clone(), v_active.clone());
        table.insert(&InnerValue::Map(m)).await.unwrap();
    }

    table.create_index("status_idx", &["status"]).await.unwrap();
    table
}

fn bench_pushdown(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("read_limit_pushdown");
    group.sample_size(bu::sample_size(20));

    for &n in &[10_000usize, 100_000usize] {
        let table = rt.block_on(build_table_with_n(n));
        let interner = rt.block_on(table.interner().get()).unwrap();

        // (A) push-down active: WHERE + LIMIT 10, no ORDER BY.
        let q_pushdown: ReadQuery = serde_json::from_value(json::json!({
            "from": "rows",
            "where": {"op": "eq", "field": ["status"], "value": "active"},
            "select": {
                "items": [
                    {"type": "field", "path": ["name"]},
                    {"type": "field", "path": ["age"]},
                    {"type": "field", "path": ["score"]},
                    {"type": "field", "path": ["email"]}
                ],
                "distinct": false
            },
            "pagination": {"mode": "LimitOffset", "limit": 10}
        }))
        .unwrap();

        // (B) push-down disabled: same shape + ORDER BY name (no sorted
        //     index → falls to `read_collecting` which projects every
        //     match before sorting + truncating).
        let q_full: ReadQuery = serde_json::from_value(json::json!({
            "from": "rows",
            "where": {"op": "eq", "field": ["status"], "value": "active"},
            "select": {
                "items": [
                    {"type": "field", "path": ["name"]},
                    {"type": "field", "path": ["age"]},
                    {"type": "field", "path": ["score"]},
                    {"type": "field", "path": ["email"]}
                ],
                "distinct": false
            },
            "order_by": {"items": [{"field": ["name"], "direction": "asc"}]},
            "pagination": {"mode": "LimitOffset", "limit": 10}
        }))
        .unwrap();

        group.throughput(Throughput::Elements(10));

        let _ = interner; // interner refcount kept alive via table.

        group.bench_function(format!("pushdown_active_N={}", n), |b| {
            b.to_async(&rt).iter(|| {
                let table = table.clone();
                let q = q_pushdown.clone();
                async move {
                    let interner = table.interner().get().await.unwrap();
                    let refs = new_map();
                    let ctx = FilterContext::new(interner, &refs);
                    let r = table.read(&q, &ctx).await.unwrap();
                    black_box(r);
                }
            });
        });

        group.bench_function(format!("pushdown_disabled_N={}", n), |b| {
            b.to_async(&rt).iter(|| {
                let table = table.clone();
                let q = q_full.clone();
                async move {
                    let interner = table.interner().get().await.unwrap();
                    let refs = new_map();
                    let ctx = FilterContext::new(interner, &refs);
                    let r = table.read(&q, &ctx).await.unwrap();
                    black_box(r);
                }
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench, bench_pushdown);
criterion_main!(benches);
