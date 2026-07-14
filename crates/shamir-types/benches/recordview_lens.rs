//! STAGE 0 (re-measured) + STAGE 1 benchmarks for the RecordView migration
//! (see `docs/dev-artifacts/perf/record-view-migration.md` §7, §9).
//!
//! Re-pointed at the STORAGE form: records are encoded via
//! `InnerValue::to_bytes()` (id-keyed msgpack), NOT `inner_to_msgpack`
//! (string-keyed client codec). Baseline = `InnerValue::from_bytes()` (full
//! tree decode) + `map.get(InternerKey)`. Lens variant = id-keyed scan:
//! encode the target field's `InternerKey` to its `bin` id-bytes, seek the
//! matching `bin` key, decode just that value.
//!
//! Three variants over the SAME encoded blob of ONE `make_record` record:
//!
//!   (a) `tree_read_age`  — BASELINE: `InnerValue::from_bytes()` (full tree
//!                          decode) + map lookup by InternerKey + read its Int.
//!   (b) `lens_read_age`  — RecordView lens: id-keyed scan over `to_bytes()`
//!                          blob, seek the bin key matching "age"'s interned
//!                          id, decode ONLY the matched Int. Zero tree.
//!   (c) `lens_match_name` — filter-eval proxy: scan to "name"'s interned id,
//!                          compare its raw string value bytes to a constant
//!                          literal on BYTES (no string construct, no typed
//!                          decode).
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): every
//! workload's setup (interner, records, encoded blobs) is built ONCE outside
//! the timed closure, exactly as it was under Criterion's `b.iter` — this is
//! plan 1 (shared setup) from the harness's methodology docs.

use std::hint::black_box;

use bench_scale_tool::Harness;
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::record_view::RecordView;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

// ---------------------------------------------------------------------------
// Record factory — identical to the prior bench so numbers are comparable.
// ---------------------------------------------------------------------------

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

fn main() {
    let mut h = Harness::new("recordview_lens", env!("CARGO_MANIFEST_DIR"));

    // --- Measurement 1: lens vs tree for a single-field read (STORAGE form) --
    let interner = Interner::new();
    let record = make_record(&interner, 0);
    let blob = record.to_bytes().expect("encode");
    let age_ik = intern(&interner, "age");
    let name_ik = intern(&interner, "name");

    // Sanity: the lens must agree with the tree on the "age" value, and the
    // string match must succeed — otherwise we'd be benchmarking a broken
    // prototype. Panic early at registration, not in the hot loop.
    {
        let tree_val = match &record {
            InnerValue::Map(m) => m.get(&age_ik).and_then(|v| match v {
                InnerValue::Int(i) => Some(*i),
                _ => None,
            }),
            _ => None,
        };
        let lens = RecordView::new(&blob).unwrap();
        let lens_val = lens.get_int(age_ik.clone());
        assert_eq!(tree_val, lens_val, "lens/tree disagree on age");

        let name_match = lens.match_str_eq(name_ik.clone(), b"user-0");
        assert!(name_match, "lens string-match failed");
    }

    // (a) BASELINE — full tree decode (from_bytes) + map lookup + Int read.
    {
        let blob = blob.clone();
        let age_key = age_ik.clone();
        h.bench("recordview_lens_single_field/tree_read_age", move || {
            let iv = InnerValue::from_bytes(black_box(&*blob)).expect("decode");
            let v = match &iv {
                InnerValue::Map(m) => m.get(black_box(&age_key)).and_then(|e| match e {
                    InnerValue::Int(i) => Some(*i),
                    _ => None,
                }),
                _ => None,
            };
            black_box(v);
        });
    }

    // (b) LENS — RecordView over to_bytes() blob, scan id-keyed + decode Int.
    {
        let blob = blob.clone();
        let age_key = age_ik.clone();
        h.bench("recordview_lens_single_field/lens_read_age", move || {
            let lens = RecordView::new(black_box(&*blob)).unwrap();
            let v = lens.get_int(black_box(age_key.clone()));
            black_box(v);
        });
    }

    // (c) LENS — filter-eval proxy: scan to "name" id-key + compare string
    //     value bytes (no decode).
    {
        let blob = blob.clone();
        let name_key = name_ik.clone();
        h.bench("recordview_lens_single_field/lens_match_name", move || {
            let lens = RecordView::new(black_box(&*blob)).unwrap();
            let v = lens.match_str_eq(black_box(name_key.clone()), black_box(b"user-0"));
            black_box(v);
        });
    }

    // --- Tier B cross-check: per-record encode/decode round-trip -----------
    // NOTE: small batch (100 records) so each timed call is a cheap unit
    // (~0.3-0.8 ms/call); the bench-scale-tool harness owns macro-iteration
    // count, so we avoid the Criterion-era 1000-record blob (~7 ms/call on
    // the decode path). The single-field workloads above already operate on
    // one record's blob and are untouched.
    let interner_b = Interner::new();
    let records: Vec<InnerValue> = (0..100).map(|i| make_record(&interner_b, i)).collect();
    let encoded: Vec<bytes::Bytes> = records
        .iter()
        .map(|r| r.to_bytes().expect("encode"))
        .collect();

    {
        let records = records.clone();
        h.bench("recordview_tier_b_tree_roundtrip/encode_100", move || {
            for r in black_box(&records) {
                black_box(r.to_bytes().expect("encode"));
            }
        });
    }
    {
        let encoded = encoded.clone();
        h.bench("recordview_tier_b_tree_roundtrip/decode_100", move || {
            for blob in black_box(&encoded) {
                black_box(InnerValue::from_bytes(&**blob).expect("decode"));
            }
        });
    }

    // --- Tier A supplementary: deep-clone cost of one InnerValue tree ------
    {
        let records = records.clone();
        h.bench("recordview_tier_a_clone_cost/clone_inner_100", move || {
            for r in black_box(&records) {
                let cloned = r.clone();
                black_box(cloned);
            }
        });
    }

    h.run();
}
