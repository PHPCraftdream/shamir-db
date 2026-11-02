use crate::common::time::ns;
use crate::server::lockout::{
    FailureOutcome, FailureState, InMemoryLockoutStore, LockoutSnapshot, LockoutState,
    LockoutStore, PairKey, Subnet, BACKOFF_RESET_NS, LOCKOUT_BASE_QUOTA, LOCKOUT_PAIR_CAPACITY,
    LOCKOUT_PARENT_QUOTA,
};
use std::sync::{Arc, Barrier};

const NOW: u64 = 10 * ns::HOUR;

fn key4(parent: u8, base: u8, user: u8) -> PairKey {
    (Subnet::V4([10, parent, base]), [user; 16])
}
fn key6(parent: u8, base: u8, user: u8) -> PairKey {
    (Subnet::V6([0x20, 1, 0, parent, 0, 0, 0, base]), [user; 16])
}
fn key(family: u8, parent: u8, base: u8, user: u8) -> PairKey {
    if family == 4 {
        key4(parent, base, user)
    } else {
        key6(parent, base, user)
    }
}
fn same_parent(a: Subnet, b: Subnet) -> bool {
    match (a, b) {
        (Subnet::V4(a), Subnet::V4(b)) => a[..2] == b[..2],
        (Subnet::V6(a), Subnet::V6(b)) => a[..6] == b[..6],
        _ => false,
    }
}
fn failure(count: u32, at: u64) -> FailureState {
    FailureState {
        count,
        last_fail_at_ns: at,
    }
}
fn active_lock() -> LockoutState {
    LockoutState {
        triggered_at_ns: NOW - 1,
        duration_ns: 2 * crate::common::time::ns::HOUR,
    }
}
fn snapshot(
    failures: Vec<(PairKey, FailureState)>,
    lockouts: Vec<(PairKey, LockoutState)>,
) -> LockoutSnapshot {
    LockoutSnapshot {
        failures,
        lockouts,
        total_lockouts: 0,
        captured_at_ns: NOW,
    }
}
fn seeded(
    failures: Vec<(PairKey, FailureState)>,
    lockouts: Vec<(PairKey, LockoutState)>,
) -> InMemoryLockoutStore {
    InMemoryLockoutStore::with_snapshot(snapshot(failures, lockouts))
}
fn recorded(store: &InMemoryLockoutStore) -> Vec<(PairKey, FailureState)> {
    store.snapshot().failures
}

#[test]
fn exhausted_base_evicts_lowest_then_oldest_for_v4_and_v6() {
    for family in [4, 6] {
        let mut fails = (0..64)
            .map(|user| (key(family, 1, 1, user), failure(2, NOW - 20)))
            .collect::<Vec<_>>();
        fails[3].1 = failure(1, NOW - 1);
        fails[4].1 = failure(1, NOW - 10);
        let store = seeded(fails, vec![]);
        store.register_failure(key(family, 1, 1, 64), NOW);
        let pairs = recorded(&store);
        assert_eq!(pairs.len(), 64);
        assert!(!pairs.iter().any(|(k, _)| *k == key(family, 1, 1, 4)));
        assert!(pairs.iter().any(|(k, _)| *k == key(family, 1, 1, 3)));
        assert!(pairs.iter().any(|(k, _)| *k == key(family, 1, 1, 64)));
    }
}

#[test]
fn exhausted_parent_evicts_only_within_parent_for_both_families() {
    for family in [4, 6] {
        let mut fails = Vec::new();
        for base in 0..8 {
            for user in 0..64 {
                fails.push((key(family, 1, base, user), failure(2, NOW - 5)));
            }
        }
        let other = key(family, 2, 0, 0);
        fails.push((other, failure(1, NOW - 100)));
        let store = seeded(fails, vec![]);
        let new_key = key(family, 1, 8, 0);
        store.register_failure(new_key, NOW);
        let pairs = recorded(&store);
        assert_eq!(pairs.len(), 513);
        assert_eq!(
            pairs
                .iter()
                .filter(|(k, _)| same_parent(k.0, key(family, 1, 0, 0).0))
                .count(),
            512
        );
        assert!(pairs.iter().any(|(k, _)| *k == other));
        assert!(pairs.iter().any(|(k, _)| *k == new_key));
    }
}

#[test]
fn exhausted_base_and_parent_candidate_must_satisfy_both_owners() {
    let mut fails = Vec::new();
    for user in 0..64 {
        fails.push((key4(1, 1, user), failure(1, NOW - 100)));
    }
    for base in 2..9 {
        for user in 0..64 {
            fails.push((key4(1, base, user), failure(1, NOW - 100)));
        }
    }
    for base in 0..8 {
        for user in 0..64 {
            fails.push((key4(2, base, user), failure(1, NOW - 100)));
        }
    }
    let store = seeded(fails, vec![]);
    let target_base = key4(1, 1, 64).0;
    let new_key = key4(1, 1, 64);
    store.register_failure(new_key, NOW);
    let pairs = recorded(&store);
    assert_eq!(pairs.len(), 1024);
    assert_eq!(pairs.iter().filter(|(k, _)| k.0 == target_base).count(), 64);
    assert!(pairs.iter().any(|(k, _)| *k == new_key));
    assert_eq!(
        pairs.iter().filter(|(k, _)| k.0 == key4(1, 1, 0).0).count(),
        64
    );
    for user in 0..64 {
        assert!(pairs.iter().any(|(k, _)| *k == key4(1, 2, user)));
    }
    assert_eq!(
        pairs
            .iter()
            .filter(|(k, _)| same_parent(k.0, key4(2, 0, 0).0))
            .count(),
        512
    );
}

#[test]
fn active_pairs_are_never_evicted_and_all_active_drops_accounting_only() {
    let fails = (0..64)
        .map(|u| (key4(1, 1, u), failure(1, NOW - 100)))
        .collect();
    let locks = (0..64).map(|u| (key4(1, 1, u), active_lock())).collect();
    let store = seeded(fails, locks);
    assert_eq!(
        store.register_failure(key4(1, 1, 64), NOW),
        FailureOutcome::Backoff { delay_ms: 0 }
    );
    assert_eq!(store.failure_pair_count(), 64);
    assert_eq!(store.active_lockout_count(), 64);
    let pairs = recorded(&store);
    for user in 0..64 {
        assert!(pairs.iter().any(|(k, _)| *k == key4(1, 1, user)));
    }
    assert!(!pairs.iter().any(|(k, _)| *k == key4(1, 1, 64)));
}

#[test]
fn quota_eviction_uses_lowest_count_before_oldest_and_oldest_ties() {
    let fails = (0..64)
        .map(|u| {
            (
                key4(1, 1, u),
                failure(if u == 7 { 2 } else { 1 }, NOW - u as u64),
            )
        })
        .collect();
    let store = seeded(fails, vec![]);
    store.register_failure(key4(1, 1, 64), NOW);
    let pairs = recorded(&store);
    assert!(!pairs.iter().any(|(k, _)| *k == key4(1, 1, 63)));
    assert!(pairs.iter().any(|(k, _)| *k == key4(1, 1, 7)));
}

#[test]
fn snapshot_deduplicates_failure_and_lock_and_preserves_highest_duplicate_count() {
    let k = key4(1, 1, 2);
    let store = seeded(
        vec![(k, failure(4, NOW - 2)), (k, failure(8, NOW - 1))],
        vec![(k, active_lock())],
    );
    assert_eq!(store.failure_pair_count(), 1);
    assert_eq!(store.active_lockout_count(), 1);
    assert_eq!(recorded(&store)[0].1.count, 8);
}

#[test]
fn snapshot_caps_base_parent_and_global_with_active_first_and_highest_count() {
    let base_fails = (0..70)
        .map(|u| (key4(1, 1, u), failure(u as u32 + 1, NOW - 1)))
        .collect();
    let base = seeded(base_fails, vec![]);
    assert_eq!(recorded(&base).len(), 64);
    assert_eq!(
        base.current_backoff_ms(key4(1, 1, 69), NOW),
        failure(70, NOW - 1).backoff_ms()
    );

    let parent_fails = (0..520)
        .map(|i| {
            (
                key4(1, (i / 64) as u8, (i % 64) as u8),
                failure(i as u32 + 1, NOW - 1),
            )
        })
        .collect();
    let parent = seeded(parent_fails, vec![]);
    assert_eq!(recorded(&parent).len(), 512);
    assert!(recorded(&parent).iter().any(|(k, _)| *k == key4(1, 8, 7)));

    let mut global_fails = Vec::new();
    for p in 0..16 {
        for i in 0..512 {
            global_fails.push((
                key4(p, (i / 64) as u8, (i % 64) as u8),
                failure(i as u32 + 1, NOW - 1),
            ));
        }
    }
    let lock_key = global_fails[0].0;
    let global = seeded(global_fails, vec![(lock_key, active_lock())]);
    assert_eq!(recorded(&global).len(), LOCKOUT_PAIR_CAPACITY);
    assert_eq!(global.active_lockout_count(), 1);
    assert!(global.is_locked_out(lock_key, NOW));

    let mut overflow_fails = Vec::new();
    for p in 0..16 {
        for i in 0..512 {
            overflow_fails.push((
                key4(p, (i / 64) as u8, (i % 64) as u8),
                failure(10, NOW - 1),
            ));
        }
    }
    let high_key = key4(16, 0, 0);
    overflow_fails.push((high_key, failure(999, NOW - 1)));
    let overflow = seeded(overflow_fails, vec![]);
    let overflow_pairs = recorded(&overflow);
    assert_eq!(overflow_pairs.len(), LOCKOUT_PAIR_CAPACITY);
    assert!(overflow_pairs.iter().any(|(k, _)| *k == high_key));
    let source_parent_counts = (0..16)
        .map(|parent| {
            overflow_pairs
                .iter()
                .filter(|(k, _)| same_parent(k.0, key4(parent, 0, 0).0))
                .count()
        })
        .collect::<Vec<_>>();
    assert!(source_parent_counts
        .iter()
        .all(|&count| count <= LOCKOUT_PARENT_QUOTA));
    assert_eq!(source_parent_counts.iter().sum::<usize>(), 8191);

    let same_base_failures = (0..70)
        .map(|u| (key4(1, 1, u), failure(1, NOW - 1)))
        .collect();
    let lock_keys = (0..64).map(|u| key4(1, 1, u)).collect::<Vec<_>>();
    let mixed = seeded(
        same_base_failures,
        lock_keys.iter().map(|k| (*k, active_lock())).collect(),
    );
    let mixed_pairs = recorded(&mixed);
    assert_eq!(mixed_pairs.len(), 64);
    for k in &lock_keys {
        assert!(mixed_pairs.iter().any(|(found, _)| found == k));
    }
    assert_eq!(mixed.active_lockout_count(), 64);
}

#[test]
fn lock_only_pair_is_retained_and_stale_failures_gc_without_expiring_hour_lock() {
    let lock_key = key4(1, 1, 1);
    let stale = key4(1, 1, 2);
    let store = seeded(
        vec![(stale, failure(3, NOW - BACKOFF_RESET_NS - 1))],
        vec![(lock_key, active_lock())],
    );
    store.gc(NOW);
    assert_eq!(store.failure_pair_count(), 0);
    assert_eq!(store.active_lockout_count(), 1);
    assert!(store.is_locked_out(lock_key, NOW));
    assert_eq!(
        store.register_failure(key4(1, 1, 3), NOW),
        FailureOutcome::Backoff { delay_ms: 100 }
    );
    assert_eq!(store.failure_pair_count(), 1);

    let lock_only = seeded(vec![], vec![(lock_key, active_lock())]);
    assert_eq!(lock_only.current_backoff_ms(lock_key, NOW), 0);
    let failure_and_lock = seeded(
        vec![(lock_key, failure(4, NOW - 1))],
        vec![(lock_key, active_lock())],
    );
    failure_and_lock.gc(NOW + BACKOFF_RESET_NS);
    assert_eq!(failure_and_lock.failure_pair_count(), 0);
    assert!(failure_and_lock.is_locked_out(lock_key, NOW + BACKOFF_RESET_NS));
}

#[test]
fn full_global_table_evicts_lowest_oldest_and_existing_updates_survive_full_capacity() {
    let mut fails = Vec::new();
    for p in 0..16 {
        for i in 0..512 {
            fails.push((
                key4(p, (i / 64) as u8, (i % 64) as u8),
                failure(
                    if p == 0 && i == 0 { 1 } else { 2 },
                    NOW - if i == 0 { 100 } else { 1 },
                ),
            ));
        }
    }
    let store = seeded(fails, vec![]);
    store.register_failure(key4(16, 0, 0), NOW);
    assert_eq!(store.failure_pair_count(), LOCKOUT_PAIR_CAPACITY);
    assert!(!recorded(&store).iter().any(|(k, _)| *k == key4(0, 0, 0)));
    assert_eq!(
        store.register_failure(key4(16, 0, 0), NOW + 1),
        FailureOutcome::Backoff { delay_ms: 200 }
    );
    assert_eq!(store.failure_pair_count(), LOCKOUT_PAIR_CAPACITY);
}

#[test]
fn full_global_active_table_drops_new_pair_without_evicting_locks() {
    let locks = (0..16)
        .flat_map(|p| {
            (0..512).map(move |i| (key4(p, (i / 64) as u8, (i % 64) as u8), active_lock()))
        })
        .collect();
    let store = seeded(vec![], locks);
    let new_key = key4(16, 0, 0);
    assert_eq!(
        store.register_failure(new_key, NOW),
        FailureOutcome::Backoff { delay_ms: 0 }
    );
    assert_eq!(store.failure_pair_count(), 0);
    assert_eq!(store.active_lockout_count(), LOCKOUT_PAIR_CAPACITY);
    assert!(!recorded(&store).iter().any(|(k, _)| *k == new_key));
}

#[test]
fn mixed_subnet_families_have_independent_quotas_and_active_pairs_survive_eviction() {
    let mut fails = Vec::new();
    for user in 0..64 {
        fails.push((key4(1, 1, user), failure(2, NOW - 1)));
    }
    for user in 0..64 {
        fails.push((key6(1, 1, user), failure(2, NOW - 1)));
    }
    let store = seeded(fails, vec![(key4(1, 1, 0), active_lock())]);
    store.register_failure(key4(1, 1, 64), NOW);
    store.register_failure(key6(1, 1, 64), NOW);
    let pairs = recorded(&store);
    assert_eq!(pairs.len(), 128);
    assert_eq!(
        pairs
            .iter()
            .filter(|(k, _)| matches!(k.0, Subnet::V4(_)))
            .count(),
        64
    );
    assert_eq!(
        pairs
            .iter()
            .filter(|(k, _)| matches!(k.0, Subnet::V6(_)))
            .count(),
        64
    );
    assert!(pairs.iter().any(|(k, _)| *k == key4(1, 1, 0)));
    assert!(pairs.iter().any(|(k, _)| *k == key4(1, 1, 64)));
    assert!(pairs.iter().any(|(k, _)| *k == key6(1, 1, 64)));
}

#[test]
fn concurrent_inserts_obey_strict_base_and_parent_quotas() {
    for family in [4, 6] {
        let store = Arc::new(InMemoryLockoutStore::new());
        let barrier = Arc::new(Barrier::new(9));
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for worker in 0..8u8 {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                handles.push(scope.spawn(move || {
                    barrier.wait();
                    for user in 0..16u8 {
                        store.register_failure(key(family, 1, 1, worker * 16 + user), NOW);
                    }
                }));
            }
            barrier.wait();
            for handle in handles {
                handle.join().unwrap();
            }
        });
        assert_eq!(store.failure_pair_count(), 64);
        assert_eq!(
            recorded(&store)
                .iter()
                .filter(|(k, _)| k.0 == key(family, 1, 1, 0).0)
                .count(),
            64
        );

        let many = Arc::new(InMemoryLockoutStore::new());
        let gate = Arc::new(Barrier::new(11));
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for worker in 0..10u8 {
                let store = Arc::clone(&many);
                let gate = Arc::clone(&gate);
                handles.push(scope.spawn(move || {
                    gate.wait();
                    for user in 0..64u8 {
                        store.register_failure(key(family, 1, worker, user), NOW);
                    }
                }));
            }
            gate.wait();
            for handle in handles {
                handle.join().unwrap();
            }
        });
        assert_eq!(many.failure_pair_count(), LOCKOUT_PARENT_QUOTA);
        assert_eq!(
            recorded(&many)
                .iter()
                .filter(|(k, _)| same_parent(k.0, key(family, 1, 0, 0).0))
                .count(),
            LOCKOUT_PARENT_QUOTA
        );
        many.register_failure(key(family, 2, 0, 0), NOW);
        assert_eq!(many.failure_pair_count(), LOCKOUT_PARENT_QUOTA + 1);
    }
}

#[test]
fn fixed_table_constants_and_quota_relationships() {
    assert_eq!(LOCKOUT_PAIR_CAPACITY, 8192);
    assert_eq!(LOCKOUT_PARENT_QUOTA, 512);
    assert_eq!(LOCKOUT_PAIR_CAPACITY / LOCKOUT_PARENT_QUOTA, 16);
}
