//! Bench: `inner_to_json_value` String clone cost (#108)
//!
//! Measures whether the String clones in the borrow-based conversion
//! (`&InnerValue` → `serde_json::Value`) are a real bottleneck, or
//! just noise compared to an owned-move path.
//!
//! Scenarios:
//!   - convert_1k / 10k / 100k — current borrow-based `inner_to_json_value`
//!   - convert_100k_owned_move — local mock that takes `InnerValue` by value
//!     and moves Strings out instead of cloning
//!   - clone_only_100k — baseline: just `Clone::clone` each InnerValue
//!
//! Measured 2026-05-26 (release build):
//!   - convert_100k (borrow):       607 ms
//!   - convert_100k_owned_move:     819 ms  (includes 206 ms clone)
//!   - clone_only_100k:             206 ms
//!
//!   Owned-move conversion-only cost: 819 - 206 = 613 ms
//!   Borrow conversion-only cost:                       607 ms
//!
//!   Delta borrow → owned-move: (613 - 607) / 607 ≈ 1.0% (within noise)
//!
//! Verdict: < 5% improvement. String clone in `Value::Str` is NOT the
//! bottleneck. Both paths pay the same dominant costs: json::Map
//! allocation + insert, serde_json::Number construction, interner
//! lookup via get_str → UserKey(String) allocation. The `Value::Str`
//! clone is drowned out by structural overhead.
//!
//! Conclusion: close #68 as not-worth-it. If conversion speed matters,
//! the lever is avoiding the intermediate `serde_json::Value` tree
//! entirely (cf. InternedRef direct-serialize path in `inner_to_json`).

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use shamir_types::codecs::interned::inner_to_json_value;
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use serde_json as json;

// ── helpers ──────────────────────────────────────────────────────────

fn intern(i: &Interner, s: &str) -> InternerKey {
    match i.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    }
}

/// Realistic record: 10 string fields (20-30 chars each) + 2 int + 1 float.
fn make_record(interner: &Interner, idx: u32) -> InnerValue {
    let mut m = new_map_wc(13);

    // 10 string fields
    m.insert(
        intern(interner, "username"),
        InnerValue::Str(format!("user_surname_{:05}", idx)),
    );
    m.insert(
        intern(interner, "email"),
        InnerValue::Str(format!("first.last.{:04}@mail.com", idx)),
    );
    m.insert(
        intern(interner, "address_line"),
        InnerValue::Str(format!("{:03} Baker Street, London", idx % 999)),
    );
    m.insert(
        intern(interner, "company"),
        InnerValue::Str(format!("Acme_Corp_Department_{:02}", idx % 50)),
    );
    m.insert(
        intern(interner, "phone"),
        InnerValue::Str(format!("+1-555-{:04}-{:04}", idx % 9999, idx)),
    );
    m.insert(
        intern(interner, "country"),
        InnerValue::Str("United_Kingdom_of_Great_Britain".into()),
    );
    m.insert(
        intern(interner, "city"),
        InnerValue::Str("San_Francisco_de_Quito".into()),
    );
    m.insert(
        intern(interner, "postal_code"),
        InnerValue::Str(format!("EC{:02}A {:03}BX", idx % 99, idx % 999)),
    );
    m.insert(
        intern(interner, "department"),
        InnerValue::Str("Engineering_and_Infrastructure".into()),
    );
    m.insert(
        intern(interner, "notes"),
        InnerValue::Str(format!("Monthly_review_scheduled_for_Q{}", idx % 4 + 1)),
    );

    // 2 int fields
    m.insert(intern(interner, "id"), InnerValue::Int(idx as i64));
    m.insert(
        intern(interner, "timestamp"),
        InnerValue::Int(1_700_000_000 + idx as i64),
    );

    // 1 float field
    m.insert(intern(interner, "score"), InnerValue::F64(idx as f64 * 1.5));

    InnerValue::Map(m)
}

// ── owned-move mock ──────────────────────────────────────────────────
//
// Local mirror of `inner_to_json_value` that takes InnerValue **by value**
// and moves Strings out with `mem::take` / `std::mem::replace`.
// Only covers the subset used by `make_record` (Null, Bool, Int, F64, Str,
// Map, List).

fn owned_to_json_value(
    value: InnerValue,
    interner: &Interner,
) -> Result<json::Value, shamir_types::codecs::CodecError> {
    use shamir_types::codecs::CodecError;
    use shamir_types::types::value::Value;

    match value {
        Value::Null => Ok(json::Value::Null),
        Value::Bool(b) => Ok(json::Value::Bool(b)),
        Value::Int(i) => Ok(json::Value::Number(i.into())),
        Value::F64(f) => {
            if f.is_finite() {
                if let Some(n) = json::Number::from_f64(f) {
                    Ok(json::Value::Number(n))
                } else {
                    Ok(json::Value::String(f.to_string()))
                }
            } else {
                Ok(json::Value::String(f.to_string()))
            }
        }
        Value::Str(s) => Ok(json::Value::String(s)), // ← move, no clone
        Value::List(mut l) => {
            let mut arr = Vec::with_capacity(l.len());
            for v in l.drain(..) {
                arr.push(owned_to_json_value(v, interner)?);
            }
            Ok(json::Value::Array(arr))
        }
        Value::Map(mut m) => {
            let mut obj = json::Map::with_capacity(m.len());
            for (interned_key, val) in m.drain(..) {
                let user_key = interner.get_str(&interned_key).ok_or_else(|| {
                    CodecError::Decode(format!("Interned key not found: {:?}", interned_key))
                })?;
                // user_key.0 is a String — avoid double-alloc by moving it.
                obj.insert(user_key.0, owned_to_json_value(val, interner)?);
            }
            Ok(json::Value::Object(obj))
        }
        // Remaining variants fall back to borrow-based path
        v => inner_to_json_value(&v, interner),
    }
}

// ── bench ────────────────────────────────────────────────────────────

fn bench(c: &mut Criterion) {
    let interner = Interner::new();

    // Pre-build enough records for all scenarios.
    let records_100k: Vec<InnerValue> = (0..100_000).map(|i| make_record(&interner, i)).collect();

    let mut group = c.benchmark_group("json_clone");

    // ── convert_Nk (borrow-based) ────────────────────────────────────

    for &n in &[1_000, 10_000, 100_000] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("convert_borrow", n), &n, |b, &n| {
            let slice = &records_100k[..n];
            b.iter(|| {
                for r in slice {
                    black_box(inner_to_json_value(r, &interner).unwrap());
                }
            });
        });
    }

    // ── clone_only_100k (intrinsic clone cost baseline) ──────────────

    group.throughput(Throughput::Elements(100_000));
    group.bench_function("clone_only_100k", |b| {
        let slice = &records_100k;
        b.iter(|| {
            for r in slice {
                black_box(r.clone());
            }
        });
    });

    // ── convert_100k_owned_move ──────────────────────────────────────

    group.throughput(Throughput::Elements(100_000));
    group.bench_function("convert_100k_owned_move", |b| {
        b.iter(|| {
            // We need owned values each iter, so clone from the fixture.
            // The clone cost is accounted for in both paths so the *delta*
            // between convert_borrow and owned_move is still meaningful.
            for r in &records_100k {
                let owned = r.clone();
                black_box(owned_to_json_value(owned, &interner).unwrap());
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
