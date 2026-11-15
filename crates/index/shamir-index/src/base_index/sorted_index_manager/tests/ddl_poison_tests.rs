//! Poison-recovery tests for `SortedIndexManager`'s DDL guard mutexes
//! (`dropping_sorted`, `renaming_sorted`).
//!
//! A thread locks a guard, seeds guarded state, then panics while still
//! holding the lock — leaving the mutex poisoned AND the seeded state in
//! place. Each test then proves the DDL operations still succeed through the
//! poison-recovering `lock_ddl_guard` helper AND observe the state the
//! panicking thread left.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;

use super::super::SortedIndexManager;
use crate::base_index::sorted_index_definition::SortedIndexDefinition;

/// Locks `mutex` in a spawned thread, lets `seed` mutate the guarded data,
/// then panics while still holding the guard — leaving the mutex poisoned
/// and the seeded state in place.
fn poison_guard<T, S>(mutex: &Arc<Mutex<T>>, seed: S)
where
    S: FnOnce(&mut T) + Send + 'static,
    T: Send + 'static,
{
    let mutex = Arc::clone(mutex);
    let handle = std::thread::spawn(move || {
        let mut guard = mutex.lock().unwrap();
        seed(&mut guard);
        panic!("simulated panic while holding a DDL guard");
    });
    assert!(
        handle.join().is_err(),
        "seed thread must panic while holding the guard"
    );
}

/// Constructs a `SortedIndexManager` over an in-memory store.
fn create_manager() -> SortedIndexManager {
    let store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    futures::executor::block_on(SortedIndexManager::new(store)).unwrap()
}

#[tokio::test]
async fn dropping_sorted_poison_recovery_succeeds() {
    let manager = create_manager();

    poison_guard(&manager.dropping_sorted, |set| {
        set.insert(61);
    });

    // The CREATE guard still observes the seeded state through the poisoned
    // mutex: registering name 61 must be rejected with `Err` (NOT a panic).
    let def = SortedIndexDefinition::new(61, vec![1]);
    assert!(manager.register(def).await.is_err());

    // The DDL op succeeds through the poisoned guard.
    manager.add_to_dropping_sorted(62).await.unwrap();

    {
        let set = manager
            .dropping_sorted
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assert!(set.contains(&61), "seeded key 61 must survive the panic");
        assert!(set.contains(&62), "new key 62 must be added despite poison");
    }

    // Clearing the seeded entry also succeeds.
    manager.clear_from_dropping_sorted(61).await.unwrap();
    let set = manager
        .dropping_sorted
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert!(!set.contains(&61));
    assert!(set.contains(&62));
    drop(set);
}

#[tokio::test]
async fn renaming_sorted_poison_recovery_succeeds() {
    let manager = create_manager();

    poison_guard(&manager.renaming_sorted, |map| {
        map.insert(71, 81);
    });

    // The DDL op succeeds through the poisoned guard and lands beside the
    // panicking thread's seeded entry.
    manager.add_to_renaming_sorted(72, 82).await.unwrap();

    let map: BTreeMap<u64, u64> = manager
        .renaming_sorted
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(map.get(&71), Some(&81), "seeded 71→81 must survive");
    assert_eq!(
        map.get(&72),
        Some(&82),
        "new 72→82 must be added despite poison"
    );
    drop(map);

    // Clearing the seeded entry also succeeds.
    manager.clear_from_renaming_sorted(71).await.unwrap();
    let map = manager
        .renaming_sorted
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert!(!map.contains_key(&71));
    assert_eq!(map.get(&72), Some(&82));
    drop(map);
}
