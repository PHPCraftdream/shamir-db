//! `/arrays::distinct` cold-path hot-loop bench.
//!
//! Pathology under test: the legacy `distinct` implementation did an
//! O(N²) full-`PartialEq` linear scan (`out.iter().any(|kept| kept == e)`)
//! for every element. The new implementation uses an order-preserving
//! hash-set dedup so membership testing is O(1) amortised — total O(N).
//!
//! The bench compares:
//! * `distinct_*` — the registered scalar fn (current O(N) impl).
//! * `distinct_naive_*` — an inlined O(N²) reference implementation
//!   matching the legacy code, so the baseline-vs-after ratio shows in
//!   a single binary run (mirrors the `interner_cold_growth` old/new
//!   side-by-side pattern).
//!
//! Two duplicate ratios at 1k and 10k scale:
//! * `all_unique` — the worst case (every element pays the full membership
//!   cost; legacy does ~N²/2 comparisons).
//! * `half_dup` — every key appears twice (realistic).
//!
//! N is capped at 10_000 because the legacy O(N²) reference is
//! intentionally slow by design — at 10k it does ~50M value comparisons
//! per call, already well over the per-call budget. The harness's
//! `--scale` knob shrinks N if a faster sweep is needed.

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_funclib::arrays;
use shamir_funclib::registry::{v_list, ScalarRegistry};
use shamir_types::types::value::QueryValue;

/// Build a `QueryValue::List` of N unique `Int` elements.
fn make_unique(n: usize) -> QueryValue {
    let v: Vec<QueryValue> = (0..n).map(|i| QueryValue::Int(i as i64)).collect();
    v_list(v)
}

/// Build a `QueryValue::List` of N elements where every value appears
/// twice (N must be even). Returns a Vec with N/2 unique values.
fn make_half_dup(n: usize) -> QueryValue {
    debug_assert!(n.is_multiple_of(2));
    let half = n / 2;
    let mut v = Vec::with_capacity(n);
    for i in 0..half {
        let qv = QueryValue::Int(i as i64);
        v.push(qv.clone());
        v.push(qv);
    }
    v_list(v)
}

/// Legacy O(N²) reference — exact code of the pre-fix `distinct`.
/// Used as the BASELINE arm so the ratio shows in one run.
fn distinct_naive(arr: &[QueryValue]) -> Vec<QueryValue> {
    let mut out: Vec<QueryValue> = Vec::with_capacity(arr.len());
    for e in arr {
        if !out.iter().any(|kept| kept == e) {
            out.push(e.clone());
        }
    }
    out
}

fn main() {
    let mut h = Harness::new("distinct_arrays", env!("CARGO_MANIFEST_DIR"));

    // 1k and 10k — the audit's called-out scale. `distinct_naive_10000`
    // is O(N²) by design (~50M comparisons/call) and is the documented
    // exception to the per-call budget (mirrors `interner_cold_growth`'s
    // `old_full_blob` exception).
    for &n in &[1000usize, 10_000] {
        let unique = make_unique(n);
        let half = make_half_dup(n);

        // Build a fresh registry per workload and wrap in Arc so the timed
        // closure can call it (`ScalarRegistry::call` is `&self`, so sharing
        // by reference is fine; the registry is cheap to build and immutable
        // after `register`).
        let mk_reg = || {
            let mut r = ScalarRegistry::new();
            arrays::register(&mut r);
            Arc::new(r)
        };

        // Current (O(N)) registered fn — all-unique input.
        let r1 = mk_reg();
        let input = unique.clone();
        h.bench(&format!("distinct_{n}_all_unique"), move || {
            let out = r1
                .call("distinct", std::slice::from_ref(&input))
                .expect("distinct");
            black_box(out);
        });

        // Current (O(N)) registered fn — half-duplicate input.
        let r2 = mk_reg();
        let input = half.clone();
        h.bench(&format!("distinct_{n}_half_dup"), move || {
            let out = r2
                .call("distinct", std::slice::from_ref(&input))
                .expect("distinct");
            black_box(out);
        });

        // Legacy O(N²) reference — all-unique input (BASELINE).
        let input = unique.clone();
        h.bench(&format!("distinct_naive_{n}_all_unique"), move || {
            let arr = match &input {
                QueryValue::List(l) => l,
                _ => unreachable!(),
            };
            let out = distinct_naive(arr);
            black_box(out);
        });

        // Legacy O(N²) reference — half-duplicate input (BASELINE).
        let input = half.clone();
        h.bench(&format!("distinct_naive_{n}_half_dup"), move || {
            let arr = match &input {
                QueryValue::List(l) => l,
                _ => unreachable!(),
            };
            let out = distinct_naive(arr);
            black_box(out);
        });
    }

    h.run();
}
