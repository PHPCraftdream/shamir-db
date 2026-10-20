use crate::completion_tracker::{CompletionTracker, State};

#[test]
fn contiguous_advance() {
    let ct = CompletionTracker::new();
    ct.mark(1, State::Materialized);
    ct.mark(2, State::Materialized);
    ct.mark(3, State::Materialized);
    assert_eq!(ct.watermark(), 3);
}

#[test]
fn hole_blocks_advance() {
    let ct = CompletionTracker::new();
    ct.mark(1, State::Materialized);
    ct.mark(3, State::Materialized);
    assert_eq!(ct.watermark(), 1);

    ct.mark(2, State::Materialized);
    assert_eq!(ct.watermark(), 3);
}

#[test]
fn aborted_passes_through() {
    let ct = CompletionTracker::new();
    ct.mark(1, State::Materialized);
    ct.mark(2, State::Aborted);
    ct.mark(3, State::Materialized);
    assert_eq!(ct.watermark(), 3);
}

#[tokio::test]
async fn concurrent_mark_random_order() {
    use std::sync::Arc;

    let ct = Arc::new(CompletionTracker::new());
    let n = 100u64;

    let mut handles = Vec::new();
    // Mark versions 1..=N in random order via shuffled spawns.
    let mut versions: Vec<u64> = (1..=n).collect();
    // Simple deterministic shuffle.
    versions.reverse();
    for chunk in versions.chunks(10) {
        let ct2 = Arc::clone(&ct);
        let chunk = chunk.to_vec();
        handles.push(tokio::spawn(async move {
            for v in chunk {
                ct2.mark(v, State::Materialized);
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(ct.watermark(), n);
}

/// P1b: proves that a batch of versions marked Aborted (simulating WAL
/// begin_many Err or PanicGuard Drop) advances the watermark past them,
/// preventing stall.
#[test]
fn abort_batch_advances_watermark() {
    let ct = CompletionTracker::new();
    // Simulate: versions 1..=5 assigned; version 1 materialized normally,
    // then a batch failure aborts 2..=5 (like begin_many Err path).
    ct.mark(1, State::Materialized);
    assert_eq!(ct.watermark(), 1);

    // Abort the batch in arbitrary order (as PanicGuard Drop would).
    ct.mark(4, State::Aborted);
    ct.mark(2, State::Aborted);
    ct.mark(5, State::Aborted);
    ct.mark(3, State::Aborted);
    assert_eq!(ct.watermark(), 5);
}

/// P1b: a single aborted version in the middle doesn't block subsequent
/// Materialized versions from advancing the watermark.
#[test]
fn single_abort_unblocks_watermark() {
    let ct = CompletionTracker::new();
    ct.mark(1, State::Materialized);
    // Version 2 hits WAL begin Err → Aborted.
    ct.mark(3, State::Materialized);
    assert_eq!(ct.watermark(), 1); // blocked on 2

    ct.mark(2, State::Aborted);
    assert_eq!(ct.watermark(), 3); // unblocked
}

#[test]
fn watermark_monotonic() {
    let ct = CompletionTracker::new();
    ct.mark(1, State::Materialized);
    ct.mark(2, State::Materialized);
    assert_eq!(ct.watermark(), 2);

    // Marking an already-passed version is a no-op.
    ct.mark(1, State::Aborted);
    assert_eq!(ct.watermark(), 2);
}
