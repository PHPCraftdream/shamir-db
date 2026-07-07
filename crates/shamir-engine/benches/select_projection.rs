//! SELECT projection / `apply_select_value` benchmark.
//!
//! Quantifies the cost of building `QueryValue` per record and serialising
//! to bytes. Replaces the old legacy benchmarks (apply_select /
//! apply_select_to_bytes) which were removed as part of J1 elimination.
//!
//! Scenarios:
//!   - select_all_100k          — `SELECT *` over 100k records (full Map clone)
//!   - select_few_fields_100k   — explicit projection of 2 of 6 fields
//!   - select_all_then_serialize_100k — project to QueryValue, then to_vec
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): setup
//! (interner, 100k records) is built ONCE outside the timed closures — the
//! same shape as the old `b.iter_batched(|| (), |_| ..., BatchSize::SmallInput)`,
//! which never actually rebuilt anything per iteration.

use std::hint::black_box;

use bench_scale_tool::Harness;
use shamir_engine::query::read::exec::apply_select_value;
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

fn main() {
    let mut h = Harness::new("select_projection", env!("CARGO_MANIFEST_DIR"));

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
    {
        let raw_records = raw_records.clone();
        let select_all = select_all.clone();
        let interner = interner.clone();
        h.bench("select_all/select_all_100k", move || {
            let projected = apply_select_value(&raw_records, &select_all, &interner);
            black_box(projected);
        });
    }

    // ── Scenario 2: explicit field list (skips most of the record) ─
    {
        let raw_records = raw_records.clone();
        let select_few = select_few.clone();
        let interner = interner.clone();
        h.bench("select_few_fields/select_2_of_6_fields_100k", move || {
            let projected = apply_select_value(&raw_records, &select_few, &interner);
            black_box(projected);
        });
    }

    // ── Scenario 3: project + serialize (matches the wire path) ────
    {
        let raw_records = raw_records.clone();
        let select_all = select_all.clone();
        let interner = interner.clone();
        h.bench(
            "select_then_serialize/select_all_then_serialize_100k",
            move || {
                let projected = apply_select_value(&raw_records, &select_all, &interner);
                let bytes = rmp_serde::to_vec_named(&projected).unwrap();
                black_box(bytes);
            },
        );
    }

    h.run();
}
