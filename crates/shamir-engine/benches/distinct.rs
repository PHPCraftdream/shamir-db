//! DISTINCT hot loop bench.
//!
//! `apply_distinct` serialises every json::Value to its canonical
//! string (`record.to_string()`) to drive an `IndexSet<String>`. For
//! a result with N records of M fields that's N×O(M) JSON
//! serialisations plus the per-record String allocation, just to
//! decide which rows are duplicates. The walk-and-hash path skips
//! the serialisation entirely.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use serde_json as json;

use shamir_engine::query::read::exec::apply_distinct;

fn make_record(idx: u32) -> json::Value {
    json::json!({
        "id": idx,
        "name": format!("user-{}", idx),
        "age": (idx % 100),
        "score": idx as f64 * 1.5,
        "active": idx % 2 == 0,
        "email": format!("u{}@example.com", idx),
        "tags": ["alpha", "beta", "gamma"],
        "address": {
            "city": "Jerusalem",
            "zip": "9100000",
            "country": "IL",
        },
    })
}

fn bench(c: &mut Criterion) {
    // All-unique input — pathological: every record goes through the
    // full key-build path. Closest to what the actual query pipeline
    // hands DISTINCT in practice.
    let unique: Vec<json::Value> = (0..1000).map(make_record).collect();

    // Half-duplicate input — every key appears twice, so half the
    // records hit the IndexSet's existing entry path.
    let mut dup = Vec::with_capacity(1000);
    for i in 0..500 {
        let r = make_record(i);
        dup.push(r.clone());
        dup.push(r);
    }

    let mut group = c.benchmark_group("apply_distinct");
    group.throughput(Throughput::Elements(1000));

    group.bench_function("1000_all_unique", |b| {
        b.iter(|| black_box(apply_distinct(unique.clone())));
    });
    group.bench_function("1000_half_dup", |b| {
        b.iter(|| black_box(apply_distinct(dup.clone())));
    });

    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
