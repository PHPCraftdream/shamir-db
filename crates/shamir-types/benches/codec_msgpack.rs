//! Encode-path bench for the interned MessagePack codec.
//!
//! Measures `inner_to_msgpack(interner, &InnerValue)` over a
//! batch of "typical record" InnerValues (Map with 10 fields,
//! one nested Map, one List). This is the per-row hot path on
//! query result encoding.
//!
//! Baseline: current `inner_to_rmpv_value` -> `rmpv::encode::write_value`
//! (allocates a full rmpv::Value tree before encoding).

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

use shamir_types::codecs::interned::messagepack::{inner_to_msgpack, msgpack_to_inner};
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

fn intern(i: &Interner, s: &str) -> InternerKey {
    match i.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    }
}

fn make_record(interner: &Interner, idx: u32) -> InnerValue {
    let mut m = new_map_wc(10);
    m.insert(intern(interner, "id"), InnerValue::Int(idx as i64));
    m.insert(
        intern(interner, "name"),
        InnerValue::Str(format!("user-{}", idx)),
    );
    m.insert(intern(interner, "age"), InnerValue::Int((idx % 100) as i64));
    m.insert(intern(interner, "score"), InnerValue::F64(idx as f64 * 1.5));
    m.insert(intern(interner, "active"), InnerValue::Bool(idx % 2 == 0));
    m.insert(
        intern(interner, "email"),
        InnerValue::Str(format!("u{}@example.com", idx)),
    );
    m.insert(intern(interner, "tags"), {
        InnerValue::List(vec![
            InnerValue::Str("alpha".into()),
            InnerValue::Str("beta".into()),
            InnerValue::Str("gamma".into()),
            InnerValue::Str("delta".into()),
            InnerValue::Str("epsilon".into()),
        ])
    });
    m.insert(intern(interner, "address"), {
        let mut a = new_map_wc(3);
        a.insert(intern(interner, "city"), InnerValue::Str("Jerusalem".into()));
        a.insert(intern(interner, "zip"), InnerValue::Str("9100000".into()));
        a.insert(intern(interner, "country"), InnerValue::Str("IL".into()));
        InnerValue::Map(a)
    });
    m.insert(intern(interner, "created_at"), InnerValue::Int(1_700_000_000 + idx as i64));
    m.insert(intern(interner, "balance"), InnerValue::F64(idx as f64 * 12.34));
    InnerValue::Map(m)
}

fn bench_encode(c: &mut Criterion) {
    let interner = Interner::new();
    let records: Vec<InnerValue> = (0..1000).map(|i| make_record(&interner, i)).collect();

    let mut group = c.benchmark_group("codec_msgpack_encode");
    group.throughput(Throughput::Elements(records.len() as u64));
    group.bench_function("interned_1000_records", |b| {
        b.iter(|| {
            for r in &records {
                black_box(inner_to_msgpack(&interner, r).unwrap());
            }
        })
    });
    group.finish();
}

fn bench_decode(c: &mut Criterion) {
    let interner = Interner::new();
    let records: Vec<InnerValue> = (0..1000).map(|i| make_record(&interner, i)).collect();
    let encoded: Vec<Vec<u8>> = records
        .iter()
        .map(|r| inner_to_msgpack(&interner, r).unwrap())
        .collect();

    let mut group = c.benchmark_group("codec_msgpack_decode");
    group.throughput(Throughput::Elements(encoded.len() as u64));
    group.bench_function("interned_1000_records", |b| {
        b.iter(|| {
            for blob in &encoded {
                black_box(msgpack_to_inner(&interner, blob).unwrap());
            }
        })
    });
    group.finish();
}

criterion_group!(benches, bench_encode, bench_decode);
criterion_main!(benches);
