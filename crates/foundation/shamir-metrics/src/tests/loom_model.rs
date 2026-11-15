//! Loom model of the two-counter seqlock snapshot (design §3.5): two writer
//! threads each observe one value (different buckets) while a reader
//! snapshots; a non-torn snapshot must be a consistent prefix of the
//! observation sequence.
//!
//! Precedent: `crates/engine/shamir-engine/src/table/support/guards/tests/loom_model.rs`.

use loom::sync::Arc;
use loom::thread;

use crate::Histogram;

/// Private crate-test-local bound set (two bounds) — keeps the model small.
const MODEL_BOUNDS: [u64; 2] = [100, 200];
const LOW: u64 = 50;
const HIGH: u64 = 300;

#[test]
fn snapshot_consistent_under_two_writers_loom_model() {
    // Preemption bound keeps the interleaving space enumerable.
    let mut builder = loom::model::Builder::new();
    builder.preemption_bound = Some(3);
    builder.check(|| {
        let h = Arc::new(Histogram::new(&MODEL_BOUNDS));
        let h0 = Arc::clone(&h);
        let w0 = thread::spawn(move || h0.observe(LOW));
        let h1 = Arc::clone(&h);
        let w1 = thread::spawn(move || h1.observe(HIGH));
        let snap = h.snapshot();
        w0.join().expect("writer 0");
        w1.join().expect("writer 1");
        if snap.torn {
            // Bounded retries exhausted under contention — the degradation
            // path, not a correctness violation.
            return;
        }
        assert_eq!(snap.bounds, &MODEL_BOUNDS);
        match snap.count {
            0 => {
                assert_eq!(snap.buckets, vec![0, 0, 0]);
                assert_eq!(snap.sum, 0);
            }
            1 => {
                // Exactly one writer landed; `sum` matches its bucket.
                if snap.buckets[0] == 1 {
                    assert_eq!(snap.buckets[1], 0);
                    assert_eq!(snap.sum, LOW);
                } else {
                    assert_eq!(snap.buckets[0], 0);
                    assert_eq!(snap.buckets[1], 1);
                    assert_eq!(snap.sum, HIGH);
                }
            }
            2 => {
                assert_eq!(snap.buckets, vec![1, 1, 0]);
                assert_eq!(snap.sum, LOW + HIGH);
            }
            other => panic!("count out of range: {other}"),
        }
    });
}
