//! DISTINCT hot loop bench.
//!
//! `apply_distinct_qv` uses a canonical json key (HashableJson) for
//! deduplication. For a result with N records of M fields that's N×O(M)
//! walk-and-hash operations per record. The walk-and-hash path skips
//! per-record string serialisation entirely vs the legacy string path.
//!
//! Note: J1 migration — apply_distinct (JSON) removed; bench now uses
//! apply_distinct_qv (QueryValue path) which is the production function.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

use shamir_engine::query::read::exec::apply_distinct_qv;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::QueryValue;

fn make_record(idx: u32) -> QueryValue {
    let mut m = new_map_wc(8);
    m.insert("id".to_string(), QueryValue::Int(idx as i64));
    m.insert("name".to_string(), QueryValue::Str(format!("user-{}", idx)));
    m.insert("age".to_string(), QueryValue::Int((idx % 100) as i64));
    m.insert("score".to_string(), QueryValue::F64(idx as f64 * 1.5));
    m.insert(
        "active".to_string(),
        QueryValue::Bool(idx.is_multiple_of(2)),
    );
    m.insert(
        "email".to_string(),
        QueryValue::Str(format!("u{}@example.com", idx)),
    );
    m.insert(
        "tags".to_string(),
        QueryValue::List(vec![
            QueryValue::Str("alpha".into()),
            QueryValue::Str("beta".into()),
            QueryValue::Str("gamma".into()),
        ]),
    );
    let mut addr = new_map_wc(3);
    addr.insert("city".to_string(), QueryValue::Str("Jerusalem".into()));
    addr.insert("zip".to_string(), QueryValue::Str("9100000".into()));
    addr.insert("country".to_string(), QueryValue::Str("IL".into()));
    m.insert("address".to_string(), QueryValue::Map(addr));
    QueryValue::Map(m)
}

fn bench(c: &mut Criterion) {
    // All-unique input — pathological: every record goes through the
    // full key-build path. Closest to what the actual query pipeline
    // hands DISTINCT in practice.
    let unique: Vec<QueryValue> = (0..1000).map(make_record).collect();

    // Half-duplicate input — every key appears twice, so half the
    // records hit the IndexMap's existing entry path.
    let mut dup = Vec::with_capacity(1000);
    for i in 0..500 {
        let r = make_record(i);
        dup.push(r.clone());
        dup.push(r);
    }

    let mut group = c.benchmark_group("apply_distinct_qv");
    group.throughput(Throughput::Elements(1000));

    group.bench_function("1000_all_unique", |b| {
        b.iter(|| black_box(apply_distinct_qv(unique.clone())));
    });
    group.bench_function("1000_half_dup", |b| {
        b.iter(|| black_box(apply_distinct_qv(dup.clone())));
    });

    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
