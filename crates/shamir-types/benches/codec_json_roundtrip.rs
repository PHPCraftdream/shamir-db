//! Round-trip vs direct conversion bench for the interned JSON codec.
//!
//! `apply_group_by` in shamir-engine had to feed `json::Value` back into
//! filter evaluation, which expects `InnerValue`. The path used was
//! `serde_json::to_vec(json_obj)` → `json_to_inner(interner, &bytes)`
//! — i.e. serialise to JSON bytes, then parse those bytes back into
//! `json::Value`, then walk that tree.
//!
//! The same module already exposes `json_value_to_inner` which walks the
//! existing `json::Value` directly into `InnerValue`, with no bytes in
//! the middle. This bench measures the difference.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use serde_json as json;

use shamir_types::codecs::interned::{json_to_inner, json_value_to_inner};
use shamir_types::core::interner::Interner;

fn make_record(idx: u32) -> json::Value {
    json::json!({
        "id": idx,
        "name": format!("user-{}", idx),
        "age": (idx % 100),
        "score": idx as f64 * 1.5,
        "active": idx.is_multiple_of(2),
        "email": format!("u{}@example.com", idx),
        "tags": ["alpha", "beta", "gamma", "delta", "epsilon"],
        "address": {
            "city": "Jerusalem",
            "zip": "9100000",
            "country": "IL",
        },
        "created_at": 1_700_000_000u64 + idx as u64,
        "balance": idx as f64 * 12.34,
    })
}

fn bench(c: &mut Criterion) {
    let interner = Interner::new();
    let records: Vec<json::Value> = (0..1000).map(make_record).collect();

    let mut group = c.benchmark_group("codec_json_to_inner");
    group.throughput(Throughput::Elements(records.len() as u64));

    // Baseline: serialise json::Value -> bytes -> parse -> InnerValue.
    // This is what `apply_group_by`'s HAVING retain closure used to do.
    group.bench_function("roundtrip_to_vec_then_parse", |b| {
        b.iter(|| {
            for r in &records {
                let bytes = serde_json::to_vec(r).unwrap();
                black_box(json_to_inner(&interner, &bytes).unwrap());
            }
        })
    });

    // Direct: walk json::Value into InnerValue in one pass.
    group.bench_function("direct_value_to_inner", |b| {
        b.iter(|| {
            for r in &records {
                black_box(json_value_to_inner(r, &interner).unwrap());
            }
        })
    });

    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
