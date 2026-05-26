//! JSON encode bench — InnerValue -> Vec<u8>.
//!
//! Mirror of codec_msgpack: the baseline `inner_to_json` builds a full
//! `serde_json::Value` tree (recursive allocations per Array/Object,
//! key/value clones) and then runs `serde_json::to_vec` over that
//! tree. The intended optimisation is the same: an `InternedRef`
//! wrapper with a direct `Serialize` impl, so `serde_json::to_vec`
//! walks the InnerValue once and writes JSON bytes straight to the
//! output buffer.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

use shamir_types::codecs::interned::inner_to_json;
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
    m.insert(
        intern(interner, "active"),
        InnerValue::Bool(idx.is_multiple_of(2)),
    );
    m.insert(
        intern(interner, "email"),
        InnerValue::Str(format!("u{}@example.com", idx)),
    );
    m.insert(
        intern(interner, "tags"),
        InnerValue::List(vec![
            InnerValue::Str("alpha".into()),
            InnerValue::Str("beta".into()),
            InnerValue::Str("gamma".into()),
            InnerValue::Str("delta".into()),
            InnerValue::Str("epsilon".into()),
        ]),
    );
    m.insert(intern(interner, "address"), {
        let mut a = new_map_wc(3);
        a.insert(
            intern(interner, "city"),
            InnerValue::Str("Jerusalem".into()),
        );
        a.insert(intern(interner, "zip"), InnerValue::Str("9100000".into()));
        a.insert(intern(interner, "country"), InnerValue::Str("IL".into()));
        InnerValue::Map(a)
    });
    m.insert(
        intern(interner, "created_at"),
        InnerValue::Int(1_700_000_000 + idx as i64),
    );
    m.insert(
        intern(interner, "balance"),
        InnerValue::F64(idx as f64 * 12.34),
    );
    InnerValue::Map(m)
}

fn bench(c: &mut Criterion) {
    let interner = Interner::new();
    let records: Vec<InnerValue> = (0..1000).map(|i| make_record(&interner, i)).collect();

    let mut group = c.benchmark_group("codec_json_encode");
    group.throughput(Throughput::Elements(records.len() as u64));
    group.bench_function("interned_1000_records", |b| {
        b.iter(|| {
            for r in &records {
                black_box(inner_to_json(&interner, r).unwrap());
            }
        })
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
