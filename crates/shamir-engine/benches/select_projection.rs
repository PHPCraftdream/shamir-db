//! SELECT projection / `apply_select` benchmark for task #110.
//!
//! Quantifies the cost of building the intermediate `serde_json::Value`
//! tree per record — the dominant overhead bench #107 identified
//! (apply_select = 62% of the read pipeline, 800k allocations / 100k
//! records, ~70 MB of churn). Task #110 plans a streaming alternative
//! that writes bytes directly without the Value tree; these scenarios
//! are the future before/after comparison.
//!
//! Baseline scenarios (run BEFORE #110 to record the current cost):
//!   - select_all_100k          — `SELECT *` over 100k records (full Map clone)
//!   - select_few_fields_100k   — explicit projection of 2 of 6 fields
//!   - select_then_serialize_100k — full pipeline: project to Value,
//!     then `serde_json::to_vec(&records)`. Matches what the wire codec
//!     does today.
//!
//! After #110 the streaming path should replace `select_then_serialize`
//! with `select_streaming` and beat the baseline. Expected speedup
//! ≥ 30% on the streaming scenario (bench #107 verdict).
//!
//! Run: `cargo bench --bench select_projection -- --quick`

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use serde_json as json;

use shamir_bench_utils as bu;
use shamir_engine::query::read::exec::{apply_select, apply_select_to_bytes};
use shamir_engine::query::read::{Select, SelectItem};
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

fn touch(interner: &Interner, s: &str) -> InternerKey {
    match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    }
}

/// Realistic 6-field record (matches the order_by_pipeline fixture so
/// numbers are directly comparable across benches).
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
        touch(interner, "active"),
        InnerValue::Bool(idx.is_multiple_of(2)),
    );
    m.insert(
        touch(interner, "created_at"),
        InnerValue::Int(1_700_000_000 + idx as i64),
    );
    InnerValue::Map(m)
}

fn bench(c: &mut Criterion) {
    let interner = Interner::new();
    for k in ["id", "name", "email", "score", "active", "created_at"] {
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

    let select_few = Select {
        items: vec![
            SelectItem::Field {
                path: vec!["email".to_string()],
                alias: None,
            },
            SelectItem::Field {
                path: vec!["score".to_string()],
                alias: None,
            },
        ],
        distinct: false,
    };

    // ── Scenario 1: SELECT * (full Map projection) ────────────────
    let mut g1 = c.benchmark_group("select_all");
    g1.throughput(Throughput::Elements(n_records));
    g1.sample_size(bu::sample_size(10));
    g1.bench_function("select_all_100k", |b| {
        b.iter_batched(
            || (),
            |_| {
                let projected = apply_select(&raw_records, &select_all, &interner);
                black_box(projected);
            },
            BatchSize::SmallInput,
        )
    });
    g1.finish();

    // ── Scenario 2: explicit field list (skips most of the record) ─
    let mut g2 = c.benchmark_group("select_few_fields");
    g2.throughput(Throughput::Elements(n_records));
    g2.sample_size(bu::sample_size(10));
    g2.bench_function("select_2_of_6_fields_100k", |b| {
        b.iter_batched(
            || (),
            |_| {
                let projected = apply_select(&raw_records, &select_few, &interner);
                black_box(projected);
            },
            BatchSize::SmallInput,
        )
    });
    g2.finish();

    // ── Scenario 3: project + serialize (matches the wire path) ────
    //
    // This is what the executor does today — build `Vec<json::Value>`,
    // then hand it to a serializer. Task #110 will replace the two
    // phases with a streaming serializer that writes bytes directly
    // from `InnerValue`. Compare against `select_all_100k` to see how
    // much extra work the `Value` tree imposes on top of the wire
    // serialization.
    let mut g3 = c.benchmark_group("select_then_serialize");
    g3.throughput(Throughput::Elements(n_records));
    g3.sample_size(bu::sample_size(10));
    g3.bench_function("select_all_then_serialize_100k", |b| {
        b.iter_batched(
            || (),
            |_| {
                let projected = apply_select(&raw_records, &select_all, &interner);
                let bytes = json::to_vec(&projected).unwrap();
                black_box(bytes);
            },
            BatchSize::SmallInput,
        )
    });
    g3.finish();

    // ── Scenario 4: streaming path (SELECT * only) ────────────────
    let mut g4 = c.benchmark_group("select_streaming");
    g4.throughput(Throughput::Elements(n_records));
    g4.sample_size(bu::sample_size(10));
    g4.bench_function("select_all_streaming_100k", |b| {
        b.iter_batched(
            || (),
            |_| {
                let bytes = apply_select_to_bytes(&raw_records, &select_all, &interner);
                black_box(bytes);
            },
            BatchSize::SmallInput,
        )
    });
    g4.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
