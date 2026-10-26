//! RV-STORAGE-E02C8A80 round-3 regression tests: the charge-ONCE RAII
//! reservation lifecycle (no double-charge on retry, refund on
//! cancellation, CAS-based oversized exclusivity).

#![allow(deprecated)]

use crate::storage_cached::budget_waits_for_tests;
use crate::storage_cached::reset_retry_count_for_tests;
use crate::storage_cached::retry_count_for_tests;
use crate::storage_cached::CachedStore;
use crate::storage_cached::COMMAND_CHANNEL_CAPACITY;
use crate::storage_cached::QUEUED_PAYLOAD_BUDGET_BYTES;
use crate::storage_in_memory::InMemoryStore;
use crate::types::{RecordKey, Store};
use bytes::Bytes;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const MIB: usize = 1024 * 1024;

/// RV-STORAGE-E02C8A80: a full-channel retry must NOT charge the byte
/// counter again. A caller whose `set` pends against a saturated queue
/// holds exactly ONE reservation across every retry; a per-attempt
/// `fetch_add` (the defective revert) would multiply the charge by the
/// number of attempts.
///
/// Counterfactual: reverting `admit`'s `Full` arm to re-charge per
/// attempt (or dropping the round-4 retry-count oracle) makes the ≥ 10
/// counted full-channel attempts below either fail to appear or add
/// another charge each, failing the retry-count or the "still exactly
/// one charge" assertion.
#[tokio::test]
async fn rv_storage_e02c8a80_retry_leak_does_not_double_charge() {
    reset_retry_count_for_tests();
    let inner = Arc::new(InMemoryStore::new());
    let (store, gate) = CachedStore::new_sync_paused_owner_for_tests(inner)
        .await
        .unwrap();
    assert_eq!(
        store.fill_command_channel_for_tests(),
        COMMAND_CHANNEL_CAPACITY,
        "the count bound must be the binding constraint for fillers"
    );

    let store = Arc::new(store);
    let key: RecordKey = Bytes::from_static(b"e02c8a80-retry-key").into();
    let value = Bytes::from(vec![0u8; MIB]);
    let expected = MIB + key.as_ref().len();
    let writer = {
        let store = store.clone();
        let key = key.clone();
        tokio::spawn(async move { store.set(key, value).await })
    };

    // Deterministically wait until the writer holds its ONE reservation.
    let mut spins = 0;
    while store.queued_bytes_for_tests() != expected {
        spins += 1;
        assert!(spins < 100_000, "writer never acquired its reservation");
        tokio::task::yield_now().await;
    }

    // Bounded-spin until the writer has performed at least TEN real
    // full-channel attempts (counted by the round-4 retry oracle): the
    // channel stays full until the gate opens, so every `try_send` in
    // this window hits `Full`.
    let mut spins = 0;
    while retry_count_for_tests() < 10 {
        spins += 1;
        assert!(
            spins < 100_000,
            "writer never performed ten full-channel retry attempts"
        );
        tokio::task::yield_now().await;
    }
    assert_eq!(
        store.queued_bytes_for_tests(),
        expected,
        "retries must not re-charge the byte counter"
    );

    gate.notify_one();
    assert!(writer.await.unwrap().unwrap(), "set must complete");
    assert_eq!(
        store.queued_bytes_for_tests(),
        0,
        "the owner's dequeue release must return the counter to zero"
    );
}

/// RV-STORAGE-E02C8A80 round 4 (owner-death safety): the byte-budget
/// reservation travels INSIDE the queued command, so an aborted/panicked
/// owner that drops queued commands WITHOUT dequeuing them still refunds
/// every charge, and a caller waiting for budget is unblocked (its
/// `admit` loop re-checks closure and sticky health every iteration).
///
/// Counterfactual: reverting to round 3 alone (charge released only by
/// the owner's manual dequeue subtraction + an `admit` budget-wait loop
/// that never re-checks closure/health) makes this test fail: the
/// aborted owner drops the queued commands un-refunded, the counter
/// stays at the ~63 MiB fill, and the waiting `set` never returns — the
/// `recv_timeout` fires and the `queued_bytes == 0` assertion fails.
#[test]
fn rv_storage_e02c8a80_owner_death_refunds_and_unblocks_admission() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = Arc::new(rt.block_on(async {
        let inner = Arc::new(InMemoryStore::new());
        CachedStore::new_sync_paused_owner_for_tests(inner)
            .await
            .unwrap()
            .0
    }));

    // Fill the byte budget with fixed-width 1 MiB sets (the last one
    // legitimately fails: 64 × (1 MiB + key_len) exceeds the budget).
    let value = Bytes::from(vec![0u8; MIB]);
    let mut admitted = 0usize;
    for i in 0..64 {
        let key: RecordKey = Bytes::from(format!("e02c8a80-death-{i:03}")).into();
        if !store.try_admit_set_for_tests(key, value.clone()) {
            break;
        }
        admitted += 1;
    }
    let key_len = "e02c8a80-death-000".len();
    assert_eq!(
        store.queued_bytes_for_tests(),
        admitted * (MIB + key_len),
        "the fill must charge exactly the admitted sets"
    );
    assert!(
        store.queued_bytes_for_tests() + MIB + key_len > QUEUED_PAYLOAD_BUDGET_BYTES,
        "the fill must leave the budget genuinely exhausted"
    );

    // A caller in another runtime attempts a set; its admission pends
    // waiting for budget.
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = {
        let store = store.clone();
        std::thread::spawn(move || {
            let rt2 = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let key2: RecordKey = Bytes::from_static(b"e02c8a80-death-waiter").into();
            let value2 = Bytes::from(vec![0u8; MIB]);
            let _ = done_tx.send(rt2.block_on(store.set(key2, value2)));
        })
    };

    // Bounded-spin until a caller is genuinely stuck waiting for budget.
    rt.block_on(async {
        let mut spins = 0;
        while budget_waits_for_tests() == 0 {
            spins += 1;
            assert!(spins < 100_000, "no admission ever pended on the budget");
            tokio::task::yield_now().await;
        }
    });

    // Kill the owner: it drops `rx`, dropping every queued
    // `QueuedCommand` — their reservations refund automatically.
    store.abort_owner_for_tests();
    rt.block_on(async {
        let mut spins = 0;
        while store.queued_bytes_for_tests() != 0 {
            spins += 1;
            assert!(
                spins < 100_000,
                "queued charges were never refunded after owner death"
            );
            tokio::task::yield_now().await;
        }
    });

    // The waiter must observe the owner death (channel closure or the
    // sticky owner-health failure) as an ERROR, not hang forever.
    let result = done_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("waiting admission must return after owner death");
    assert!(
        result.is_err(),
        "the waiting admission must observe owner death as an error"
    );
    worker.join().unwrap();
}

/// RV-STORAGE-E02C8A80: cancelling an admission (dropping the `admit`
/// future while it pends on a full channel) must refund the reservation
/// via `Drop`. A no-refund revert leaves the 1 MiB charge stuck on the
/// counter forever.
///
/// Counterfactual: reverting `Reservation::drop` to a no-op (or the old
/// reserve-then-send code that never refunds a refused command) leaves
/// the counter at 1 MiB + key_len after the timeout cancels the future,
/// failing the `== 0` assertion below.
#[tokio::test]
async fn rv_storage_e02c8a80_cancelling_admission_refunds_reservation() {
    let inner = Arc::new(InMemoryStore::new());
    let (store, _gate) = CachedStore::new_sync_paused_owner_for_tests(inner)
        .await
        .unwrap();
    assert_eq!(
        store.fill_command_channel_for_tests(),
        COMMAND_CHANNEL_CAPACITY,
        "the channel must be saturated before the cancelled admission"
    );

    let key: RecordKey = Bytes::from_static(b"e02c8a80-cancel-key").into();
    let value = Bytes::from(vec![0u8; MIB]);
    // The 1 ms timeout is only the cancellation trigger: the future is
    // polled at least once (reserving) before the timer fires, which is
    // deterministic against a saturated channel.
    let attempted = tokio::time::timeout(Duration::from_millis(1), store.set(key, value)).await;
    assert!(
        attempted.is_err(),
        "the set must have been cancelled while the channel was full"
    );
    assert_eq!(
        store.queued_bytes_for_tests(),
        0,
        "cancelling the admission must refund the reservation via Drop"
    );
}

/// RV-STORAGE-E02C8A80: oversized admission is exclusive. The oversized
/// reservation is taken ONLY from an exactly-zero counter (a single
/// CAS), so of N concurrent oversized producers at most ONE can win —
/// regardless of interleaving. The defective revert checked
/// `tx.capacity()` before sending (check-then-act), which two concurrent
/// oversized producers could both pass, double-admitting and
/// over-charging.
///
/// Counterfactual: reverting to the `tx.capacity() == capacity`-then-send
/// exclusivity lets two of the 8 threads win the race when they interleave
/// between the capacity check and the send; the "exactly 1 success" and
/// "exactly one oversized charge" assertions below then fail. NOTE: that
/// revert is race-dependent — it may need repeated CI runs to reproduce.
///
/// Counterfactual (exclusivity mechanism itself): replacing the oversized
/// `compare_exchange(0, size)` with a plain non-zero-checking CAS or a
/// `fetch_add` lets multiple threads observe the counter as reservable
/// simultaneously, so more than one succeeds and the exact-1 assertion
/// fails deterministically under the barrier-synchronized start.
#[test]
fn rv_storage_e02c8a80_oversized_admission_is_exclusive_under_concurrency() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (store, _gate) = rt.block_on(async {
        let inner = Arc::new(InMemoryStore::new());
        CachedStore::new_sync_paused_owner_for_tests(inner)
            .await
            .unwrap()
    });
    let store = Arc::new(store);

    // ONE oversized payload, created once (clones are refcounted).
    let value = Bytes::from(vec![0u8; QUEUED_PAYLOAD_BUDGET_BYTES + 1]);
    let keys: Vec<RecordKey> = (0..8)
        .map(|i| Bytes::from(format!("e02c8a80-exclusive-{i}")).into())
        .collect();

    let successes = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(std::sync::Barrier::new(8));
    std::thread::scope(|scope| {
        for key in &keys {
            let store = store.clone();
            let value = value.clone();
            let key = key.clone();
            let successes = successes.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                if store.try_admit_set_for_tests(key, value) {
                    successes.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });

    assert_eq!(
        successes.load(Ordering::Relaxed),
        1,
        "exactly ONE oversized producer may win the exclusive reservation"
    );
    let key_len = keys[0].as_ref().len();
    assert_eq!(
        store.queued_bytes_for_tests(),
        QUEUED_PAYLOAD_BUDGET_BYTES + 1 + key_len,
        "the total oversized charge must equal exactly ONE command"
    );
}
