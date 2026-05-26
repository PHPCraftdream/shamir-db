//! ORDER BY baseline benchmark.
//!
//! Measures the three realistic ORDER BY scenarios and quantifies how
//! much CPU is spent in `serde_json::Value::get` (per-comparison field
//! resolution in the comparator) vs the sort + permutation itself.
//!
//! Measured 2026-05-26 (release build, 100k records, 5 fields):
//!   - order_by_indexed_field_limit_100  (score, sorted): ~84 ms / iter
//!   - order_by_non_indexed_field_limit_100 (email):     ~107 ms / iter
//!   - order_by_non_indexed_field_full (email):           ~94 ms / iter
//!
//! Detailed phase breakdown via `examples/prof_order_by.rs`
//! (`cargo run --release --example prof_order_by`):
//!   - Pure `Vec<json::Value>` permutation:        ~1.3 ms
//!   - Pre-extracted sort + permute (no lookup):   ~5.3 ms
//!   - Full apply_order_by (with comparator lookup): ~35 ms
//!   - Lookup + value-swap overhead:               ~30 ms (= 85% of sort)
//!
//! Verdict: `Value::get` field lookup inside the comparator is the
//! dominant cost — ~85% of ORDER BY time, 17% of the whole read
//! pipeline. Pre-extracting sort keys collapses it by ~6.7×.
//!
//! Conclusion: PROCEED with task #67 (precomputed field positions /
//! pre-extracted sort keys).
//!
//! Bonus signal: `apply_select` (JSON projection) is 63% of the full
//! read pipeline — strong evidence for task #68 too (inner_to_json_value
//! string clones).

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use serde_json as json;

use shamir_engine::query::read::exec::{apply_order_by, apply_pagination, apply_select};
use shamir_engine::query::read::{
    OrderBy, OrderByItem, OrderDirection, Pagination, Select, SelectItem,
};
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

fn touch(interner: &Interner, s: &str) -> InternerKey {
    match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    }
}

/// Build one record: `{ id, name, email, score, created_at }`.
fn make_record(interner: &Interner, idx: u32) -> InnerValue {
    let mut m = new_map_wc(8);
    m.insert(touch(interner, "id"), InnerValue::Int(idx as i64));
    m.insert(
        touch(interner, "name"),
        InnerValue::Str(format!("user-{idx}")),
    );
    m.insert(
        touch(interner, "email"),
        InnerValue::Str(format!("user{idx}@example.com")),
    );
    m.insert(
        touch(interner, "score"),
        InnerValue::F64((idx as f64) * 1.7),
    );
    m.insert(
        touch(interner, "created_at"),
        InnerValue::Int(1_700_000_000 + idx as i64),
    );
    InnerValue::Map(m)
}

fn bench(c: &mut Criterion) {
    // ── Setup ─────────────────────────────────────────────────────
    let interner = Interner::new();
    for k in ["id", "name", "email", "score", "created_at"] {
        let _ = interner.touch_ind(k);
    }

    let n_records: u64 = 100_000;
    let raw_records: Vec<(RecordId, InnerValue)> = (0..n_records)
        .map(|i| (RecordId::new(), make_record(&interner, i as u32)))
        .collect();

    let select_all = Select {
        items: vec![SelectItem::All],
        distinct: false,
    };

    // Project once — ORDER BY operates on `Vec<json::Value>`.
    let projected: Vec<json::Value> = apply_select(&raw_records, &select_all, &interner);

    let order_by_score = OrderBy {
        items: vec![OrderByItem {
            field: vec!["score".to_string()],
            direction: OrderDirection::Asc,
            nulls: None,
        }],
    };

    let order_by_email = OrderBy {
        items: vec![OrderByItem {
            field: vec!["email".to_string()],
            direction: OrderDirection::Asc,
            nulls: None,
        }],
    };

    // ── Scenario 1: indexed field + LIMIT 100 ─────────────────────
    let mut g1 = c.benchmark_group("order_by_indexed_field_limit_100");
    g1.throughput(Throughput::Elements(n_records));
    g1.sample_size(10);
    g1.bench_function("score_asc_limit_100", |b| {
        b.iter_batched(
            || projected.clone(),
            |mut recs| {
                apply_order_by(&mut recs, &order_by_score);
                let limited = apply_pagination(
                    recs,
                    &Pagination::LimitOffset {
                        limit: Some(100),
                        offset: 0,
                    },
                    false,
                );
                black_box(limited);
            },
            BatchSize::SmallInput,
        )
    });
    g1.finish();

    // ── Scenario 2: non-indexed field + LIMIT 100 ─────────────────
    let mut g2 = c.benchmark_group("order_by_non_indexed_field_limit_100");
    g2.throughput(Throughput::Elements(n_records));
    g2.sample_size(10);
    g2.bench_function("email_asc_limit_100", |b| {
        b.iter_batched(
            || projected.clone(),
            |mut recs| {
                apply_order_by(&mut recs, &order_by_email);
                let limited = apply_pagination(
                    recs,
                    &Pagination::LimitOffset {
                        limit: Some(100),
                        offset: 0,
                    },
                    false,
                );
                black_box(limited);
            },
            BatchSize::SmallInput,
        )
    });
    g2.finish();

    // ── Scenario 3: non-indexed field, full sort (no LIMIT) ───────
    let mut g3 = c.benchmark_group("order_by_non_indexed_field_full");
    g3.throughput(Throughput::Elements(n_records));
    g3.sample_size(10);
    g3.bench_function("email_asc_full", |b| {
        b.iter_batched(
            || projected.clone(),
            |mut recs| {
                apply_order_by(&mut recs, &order_by_email);
                black_box(recs);
            },
            BatchSize::SmallInput,
        )
    });
    g3.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
