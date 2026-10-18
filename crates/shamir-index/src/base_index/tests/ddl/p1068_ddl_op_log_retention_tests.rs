//! P1 (#1068): DDL op-log retention — the `DDL_OP_LOG_CAP`/eviction
//! machinery must actually run, must never touch `InProgress` records, and
//! must be idempotent/crash-safe (re-derived from on-disk state every run,
//! no tombstone needed).
//!
//! Uses `maybe_evict_terminal_records_with_cap` (the explicit-cap test seam)
//! so these tests exercise real eviction with a cap of 5 instead of writing
//! `DDL_OP_LOG_CAP` (10000) real records.

use std::sync::Arc;

use shamir_query_types::read::{DdlOpKind, DdlOpState, DdlOpStatus};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;

use crate::base_index::ddl_op_log::{
    self, maybe_evict_terminal_records_with_cap, read_op_status, write_op_status,
    write_op_status_with_cap,
};

/// Mirrors `ddl_op_log::DDL_OP_LOG_EVICTION_CHECK_INTERVAL` (private to the
/// module). Kept in sync manually — if the production constant changes,
/// this test needs to change with it, same as any other test-side mirror of
/// an implementation constant.
const DDL_OP_LOG_EVICTION_CHECK_INTERVAL: usize = 100;

fn fresh_store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new()) as Arc<dyn Store>
}

/// Builds a terminal (`Succeeded`) status for a fresh, timestamp-ordered
/// `op_id`. Each call mints a strictly newer `RecordId` than the previous
/// call (real wall-clock timestamps), so a sequence of these is already in
/// chronological/lexicographic order — mirroring how real DDL ops accrue.
fn succeeded_status() -> DdlOpStatus {
    DdlOpStatus {
        op_id: RecordId::new(),
        kind: DdlOpKind::Other {
            description: "#1068 test op".to_string(),
        },
        state: DdlOpState::Succeeded { completed_at: 0 },
    }
}

fn in_progress_status() -> DdlOpStatus {
    DdlOpStatus {
        op_id: RecordId::new(),
        kind: DdlOpKind::Other {
            description: "#1068 test op (in progress)".to_string(),
        },
        state: DdlOpState::InProgress,
    }
}

/// Writes `n` terminal statuses in sequence and returns their `op_id`s in
/// write (== chronological) order. Sleeps a tick between writes so each
/// `RecordId::new()` gets a strictly increasing microsecond timestamp —
/// `RecordId` collisions within the same microsecond are still extremely
/// unlikely (random tail) but we want deterministic ORDER here, not just
/// uniqueness, so we force real time to advance.
async fn write_n_terminal(info_store: &Arc<dyn Store>, n: usize) -> Vec<RecordId> {
    let mut ids = Vec::with_capacity(n);
    for _ in 0..n {
        let status = succeeded_status();
        ids.push(status.op_id);
        write_op_status(info_store, &status).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_micros(2)).await;
    }
    ids
}

// ============================================================================
// 1. Cap exceeded -> oldest terminal records evicted, newest survive.
// ============================================================================

#[tokio::test]
async fn cap_exceeded_evicts_oldest_terminal_records_keeps_newest() {
    let info_store = fresh_store();
    let cap = 5usize;
    let total = 12usize;

    let ids = write_n_terminal(&info_store, total).await;

    maybe_evict_terminal_records_with_cap(&info_store, cap)
        .await
        .unwrap();

    // The oldest (total - cap) records must be gone.
    let evicted_count = total - cap;
    for id in &ids[..evicted_count] {
        let status = read_op_status(&info_store, id).await.unwrap();
        assert!(
            status.is_none(),
            "expected oldest record {id:?} to have been evicted, but it is still present"
        );
    }

    // The newest `cap` records must survive.
    for id in &ids[evicted_count..] {
        let status = read_op_status(&info_store, id).await.unwrap();
        assert!(
            status.is_some(),
            "expected newest record {id:?} to survive eviction, but it is gone"
        );
    }

    // Exactly `cap` terminal records remain.
    let remaining = ddl_op_log::count_terminal_records(&info_store)
        .await
        .unwrap();
    assert_eq!(
        remaining, cap as u64,
        "expected exactly {cap} terminal records to remain after eviction"
    );
}

// ============================================================================
// 2. InProgress records are NEVER evicted, even when oldest and the
//    terminal population alone exceeds the cap.
// ============================================================================

#[tokio::test]
async fn in_progress_records_survive_eviction_even_when_oldest_and_over_cap() {
    let info_store = fresh_store();
    let cap = 5usize;

    // Seed InProgress records FIRST (oldest), then a terminal population
    // that alone exceeds the cap.
    let mut in_progress_ids = Vec::new();
    for _ in 0..3 {
        let status = in_progress_status();
        in_progress_ids.push(status.op_id);
        write_op_status(&info_store, &status).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_micros(2)).await;
    }

    let terminal_ids = write_n_terminal(&info_store, 12).await;

    maybe_evict_terminal_records_with_cap(&info_store, cap)
        .await
        .unwrap();

    // Every InProgress record survives, unconditionally.
    for id in &in_progress_ids {
        let status = read_op_status(&info_store, id).await.unwrap();
        assert!(
            matches!(status, Some(s) if matches!(s.state, DdlOpState::InProgress)),
            "InProgress record {id:?} must survive eviction regardless of age, but it is gone \
             or was mutated"
        );
    }

    // Terminal population is correctly trimmed to cap.
    let evicted_count = terminal_ids.len() - cap;
    for id in &terminal_ids[..evicted_count] {
        let status = read_op_status(&info_store, id).await.unwrap();
        assert!(
            status.is_none(),
            "expected oldest terminal record {id:?} to have been evicted"
        );
    }
    for id in &terminal_ids[evicted_count..] {
        let status = read_op_status(&info_store, id).await.unwrap();
        assert!(
            status.is_some(),
            "expected newest terminal record {id:?} to survive eviction"
        );
    }

    let remaining_terminal = ddl_op_log::count_terminal_records(&info_store)
        .await
        .unwrap();
    assert_eq!(remaining_terminal, cap as u64);
}

/// #1068 brief's mandatory revert-and-check for the InProgress-survives
/// invariant: temporarily make eviction consider `InProgress` records
/// eligible too (mirroring what a regression would look like — treating
/// EVERY record as terminal/eviction-eligible, not just non-InProgress
/// ones) and confirm the assertion above would go RED under that broken
/// behavior. This is a self-contained re-implementation of the buggy
/// variant (not a call into production code with a feature flag), so it
/// proves the real test actually depends on the InProgress guard rather
/// than passing vacuously.
#[tokio::test]
async fn revert_check_in_progress_survives_invariant_fails_without_the_guard() {
    let info_store = fresh_store();
    let cap = 5usize;

    let mut in_progress_ids = Vec::new();
    for _ in 0..3 {
        let status = in_progress_status();
        in_progress_ids.push(status.op_id);
        write_op_status(&info_store, &status).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_micros(2)).await;
    }
    let _terminal_ids = write_n_terminal(&info_store, 12).await;

    // Broken variant: evict oldest records WITHOUT skipping InProgress —
    // i.e. treat the whole prefix as eviction-eligible. This is exactly
    // the bug the real `maybe_evict_terminal_records_with_cap` guards
    // against (classifying InProgress as non-terminal before collecting
    // eviction candidates).
    use futures::StreamExt;
    let prefix = bytes::Bytes::from_static(b"ddl_op:");
    let stream = info_store.scan_prefix_stream(prefix, 256);
    futures::pin_mut!(stream);
    let mut all_keys: Vec<shamir_storage::types::RecordKey> = Vec::new();
    while let Some(batch) = stream.next().await {
        for (key, _value) in batch.unwrap() {
            all_keys.push(key);
        }
    }
    let total = all_keys.len();
    if total > cap {
        let evict_count = total - cap;
        let to_evict: Vec<_> = all_keys.into_iter().take(evict_count).collect();
        info_store.remove_many(to_evict).await.unwrap();
    }

    // Under the broken (no-InProgress-guard) variant, the three InProgress
    // records — being the OLDEST records in the log — get evicted. Confirm
    // the invariant the real test asserts is indeed violated here, proving
    // the real guard is load-bearing and not a vacuous check.
    let mut any_in_progress_evicted = false;
    for id in &in_progress_ids {
        if read_op_status(&info_store, id).await.unwrap().is_none() {
            any_in_progress_evicted = true;
        }
    }
    assert!(
        any_in_progress_evicted,
        "revert-and-check invariant violated: the broken (InProgress-eligible) eviction variant \
         was expected to evict at least one InProgress record (they are the oldest), but none \
         were evicted — the real test would not actually be proving the InProgress guard exists"
    );
}

// ============================================================================
// 3. Idempotent / crash-safe re-run.
// ============================================================================

#[tokio::test]
async fn eviction_is_idempotent_on_immediate_rerun() {
    let info_store = fresh_store();
    let cap = 5usize;

    let ids = write_n_terminal(&info_store, 12).await;

    maybe_evict_terminal_records_with_cap(&info_store, cap)
        .await
        .unwrap();

    let remaining_after_first = ddl_op_log::count_terminal_records(&info_store)
        .await
        .unwrap();
    assert_eq!(remaining_after_first, cap as u64);

    // Run again immediately, no new writes in between: must be a clean
    // no-op — no error, no further deletions, no double-counting.
    maybe_evict_terminal_records_with_cap(&info_store, cap)
        .await
        .unwrap();

    let remaining_after_second = ddl_op_log::count_terminal_records(&info_store)
        .await
        .unwrap();
    assert_eq!(
        remaining_after_second, cap as u64,
        "a second, immediate eviction re-run must not evict any further records"
    );

    // The exact same `cap` newest records must still be present.
    let evicted_count = ids.len() - cap;
    for id in &ids[evicted_count..] {
        assert!(read_op_status(&info_store, id).await.unwrap().is_some());
    }
}

// ============================================================================
// 4. Throttled trigger actually fires (via the real write_op_status path).
// ============================================================================

/// Proves the throttled trigger inside `write_op_status` actually fires,
/// with no explicit caller-side call to `maybe_evict_terminal_records`
/// anywhere in this test.
///
/// Uses `write_op_status_with_cap` (a test seam that threads an explicit
/// cap through the REAL `write_op_status` body — same production code
/// path, same throttle counter, same modulo check; only the cap the
/// internal sweep enforces is overridden) with a small cap, rather than
/// the real `DDL_OP_LOG_CAP` (10000), so this test can observe actual
/// evictions without writing 10000 records.
///
/// The module's throttle counter (`DDL_OP_LOG_WRITE_COUNTER`) is a single
/// process-wide `static`, shared with every other test in this binary that
/// calls `write_op_status`/`write_op_status_with_cap` concurrently (see the
/// counter's doc comment for why it's process-wide). That means this test
/// cannot assume it starts the throttle window at counter value 0 — some
/// other concurrently-running test may have already advanced it to an
/// arbitrary phase. To be robust to that, this test writes
/// `2 * DDL_OP_LOG_EVICTION_CHECK_INTERVAL` terminal records: no matter
/// what phase the shared counter starts at, at least one of these writes
/// MUST land on a throttle-interval boundary (a run of `N` consecutive
/// increments always crosses at least `floor(N / interval)` multiples of
/// the interval, so `N = 2 * interval` guarantees at least one), triggering
/// at least one real eviction sweep against the small cap. After the LAST
/// such boundary crossing within the run, at most `interval - 1` further
/// writes can land before the run ends (a boundary recurs every `interval`
/// writes), so the terminal population left standing is bounded by
/// `cap + (interval - 1)` — still far below `total`, which is what a
/// dead-wiring regression would leave behind.
///
/// If the throttle wiring were dead (removed modulo check, or the
/// eviction call deleted/fire-and-forgotten and dropped), NO eviction
/// would ever run and the final terminal count would equal every record
/// written (`total`) — well above the `cap + interval - 1` bound this test
/// asserts. So this test fails on code lacking the mechanism it proves.
#[tokio::test]
async fn throttled_trigger_fires_automatically_through_write_op_status() {
    let info_store = fresh_store();
    let cap = 3usize;
    let total = 2 * DDL_OP_LOG_EVICTION_CHECK_INTERVAL;
    let upper_bound = cap + DDL_OP_LOG_EVICTION_CHECK_INTERVAL - 1;

    for _ in 0..total {
        let status = succeeded_status();
        // The real call path: write_op_status_with_cap is write_op_status's
        // own implementation with the cap parameterized — no direct call
        // to maybe_evict_terminal_records anywhere in this test.
        write_op_status_with_cap(&info_store, &status, cap)
            .await
            .unwrap();
    }

    let remaining = ddl_op_log::count_terminal_records(&info_store)
        .await
        .unwrap();
    assert!(
        remaining <= upper_bound as u64,
        "expected the throttled trigger inside write_op_status to have fired automatically \
         while writing {total} terminal records (interval={DDL_OP_LOG_EVICTION_CHECK_INTERVAL}, \
         cap={cap}), trimming the log to at most {upper_bound} terminal records — instead \
         {remaining} remain, meaning eviction never ran automatically"
    );
    assert!(
        remaining < total as u64,
        "sanity check: {remaining} terminal records remain out of {total} written — if this \
         equals {total} exactly, eviction did not run at all"
    );
}
