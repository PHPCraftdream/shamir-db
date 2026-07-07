//! DISTINCT hot loop bench.
//!
//! `apply_distinct_qv` uses a canonical hashable key (HashableQueryValue) for
//! deduplication. For a result with N records of M fields that's N×O(M)
//! walk-and-hash operations per record. The walk-and-hash path skips
//! per-record string serialisation entirely vs the legacy string path.
//!
//! N is capped at 200 (was 1000): each record carries a heavy nested
//! `QueryValue` (List + nested Map + strings), so the per-call clone +
//! walk-and-hash at N=1000 cost ~7ms — too close to the ~10ms/call budget
//! the fixed-iteration harness expects. N=200 keeps the same workload
//! shape at ~1.5ms/call.
//!
//! Note: J1 migration — apply_distinct (legacy value path) removed; bench now uses
//! apply_distinct_qv (QueryValue path) which is the production function.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): the two
//! fixtures (`unique`, `dup`) are built ONCE outside the timed closure and
//! `clone()`d per iteration — the same shape as the original Criterion
//! `b.iter(|| apply_distinct_qv(unique.clone()))`, since the function
//! consumes its input by value.

use std::hint::black_box;

use bench_scale_tool::Harness;
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

fn main() {
    let mut h = Harness::new("distinct", env!("CARGO_MANIFEST_DIR"));

    const N: u32 = 200;

    // All-unique input — pathological: every record goes through the
    // full key-build path. Closest to what the actual query pipeline
    // hands DISTINCT in practice.
    let unique: Vec<QueryValue> = (0..N).map(make_record).collect();

    // Half-duplicate input — every key appears twice, so half the
    // records hit the IndexMap's existing entry path.
    let mut dup = Vec::with_capacity(N as usize);
    for i in 0..N / 2 {
        let r = make_record(i);
        dup.push(r.clone());
        dup.push(r);
    }

    {
        let unique = unique.clone();
        h.bench("apply_distinct_qv/200_all_unique", move || {
            black_box(apply_distinct_qv(unique.clone()));
        });
    }
    {
        let dup = dup.clone();
        h.bench("apply_distinct_qv/200_half_dup", move || {
            black_box(apply_distinct_qv(dup.clone()));
        });
    }

    h.run();
}
