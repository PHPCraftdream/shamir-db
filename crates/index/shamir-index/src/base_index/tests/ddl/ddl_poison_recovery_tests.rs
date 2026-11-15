//! Poison-recovery tests for `IndexManager`'s DDL guard mutexes.
//!
//! A thread locks a guard (`dropping_*` / `renaming_*` / `dirty_sets`),
//! seeds guarded state, then panics while still holding the lock — leaving
//! the mutex poisoned AND the seeded state in place. Each test then proves
//! the DDL/lookup operations still succeed through the poison-recovering
//! `lock_ddl_guard` helpers AND observe the state the panicking thread left.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::index_manager::{HashRenameTombstone, IndexManager};
use crate::IndexState;

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

/// Constructs an `IndexManager` over two in-memory stores (inline mirror of
/// `index_manager_tests::helpers::create_manager`, which is not visible here).
fn create_manager() -> IndexManager {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    futures::executor::block_on(IndexManager::new(data_store, info_store)).unwrap()
}

/// Builds an `InnerValue::Map` with the given `(field, value)` pairs.
fn test_value(pairs: &[(u64, InnerValue)]) -> InnerValue {
    let mut map = new_map();
    for (key, value) in pairs {
        map.insert(InternerKey::new(*key), value.clone());
    }
    InnerValue::Map(map)
}

#[tokio::test]
async fn dropping_regular_poison_recovery_succeeds() {
    let manager = create_manager();

    poison_guard(&manager.dropping_regular, |m| {
        m.insert(7, Some("op-7".to_string()));
    });

    // DDL op succeeds through the poisoned guard and lands beside the
    // panicking thread's seeded entry.
    manager.add_to_dropping(false, 9, None).await.unwrap();

    {
        let map = manager
            .dropping_regular
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assert!(map.contains_key(&7), "seeded key 7 must survive the panic");
        assert!(
            map.contains_key(&9),
            "new key 9 must be added despite poison"
        );
    }

    // Clearing the seeded entry also succeeds.
    manager.clear_from_dropping(false, 7).await.unwrap();
    {
        let map = manager
            .dropping_regular
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assert!(!map.contains_key(&7));
        assert!(map.contains_key(&9));
    }

    // The CREATE guard still observes the in-flight tombstone: a CREATE
    // reusing still-dropped name 9 must be rejected with `Err` (NOT a panic
    // on the poisoned mutex).
    let def = IndexDefinition::new(9, vec![IndexInfoItem::new(vec![1])]);
    assert!(manager.create_index(def).await.is_err());
}

#[tokio::test]
async fn dropping_unique_poison_recovery_succeeds() {
    let manager = create_manager();

    poison_guard(&manager.dropping_unique, |m| {
        m.insert(7, Some("op-7".to_string()));
    });

    manager.add_to_dropping(true, 9, None).await.unwrap();

    {
        let map = manager
            .dropping_unique
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assert!(map.contains_key(&7), "seeded key 7 must survive the panic");
        assert!(
            map.contains_key(&9),
            "new key 9 must be added despite poison"
        );
    }

    manager.clear_from_dropping(true, 7).await.unwrap();
    let map = manager
        .dropping_unique
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert!(!map.contains_key(&7));
    assert!(map.contains_key(&9));
    drop(map);
}

#[tokio::test]
async fn renaming_regular_poison_recovery_succeeds() {
    let manager = create_manager();

    poison_guard(&manager.renaming_regular, |m| {
        m.insert(
            7,
            HashRenameTombstone {
                old_name: "old7".to_string(),
                new_name: "new7".to_string(),
                paths: vec!["f".to_string()],
                op_id: Some("op-7".to_string()),
            },
        );
    });

    let tombstone_9 = HashRenameTombstone {
        old_name: "old9".to_string(),
        new_name: "new9".to_string(),
        paths: vec!["f".to_string()],
        op_id: None,
    };
    manager
        .add_to_renaming(false, 9, tombstone_9)
        .await
        .unwrap();

    {
        let map = manager
            .renaming_regular
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assert!(map.contains_key(&7), "seeded key 7 must survive the panic");
        assert!(
            map.contains_key(&9),
            "new key 9 must be added despite poison"
        );
    }

    manager.clear_from_renaming(false, 7).await.unwrap();
    let map = manager
        .renaming_regular
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert!(!map.contains_key(&7));
    assert!(map.contains_key(&9));
    drop(map);
}

#[tokio::test]
async fn renaming_unique_poison_recovery_succeeds() {
    let manager = create_manager();

    poison_guard(&manager.renaming_unique, |m| {
        m.insert(
            7,
            HashRenameTombstone {
                old_name: "old7".to_string(),
                new_name: "new7".to_string(),
                paths: vec!["f".to_string()],
                op_id: Some("op-7".to_string()),
            },
        );
    });

    let tombstone_9 = HashRenameTombstone {
        old_name: "old9".to_string(),
        new_name: "new9".to_string(),
        paths: vec!["f".to_string()],
        op_id: None,
    };
    manager.add_to_renaming(true, 9, tombstone_9).await.unwrap();

    {
        let map = manager
            .renaming_unique
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assert!(map.contains_key(&7), "seeded key 7 must survive the panic");
        assert!(
            map.contains_key(&9),
            "new key 9 must be added despite poison"
        );
    }

    manager.clear_from_renaming(true, 7).await.unwrap();
    let map = manager
        .renaming_unique
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert!(!map.contains_key(&7));
    assert!(map.contains_key(&9));
    drop(map);
}

#[tokio::test]
async fn dirty_sets_outer_poison_recovery_succeeds() {
    let manager = create_manager();
    manager.mark_build_in_flight(100);

    let seeded_rid = RecordId::new();
    poison_guard(&manager.dirty_sets, move |m| {
        m.insert(100, Arc::new(Mutex::new(BTreeSet::from([seeded_rid]))));
    });

    // Drain observes the seeded RecordId through recovered outer+inner locks.
    let drained = manager.drain_dirty_set(100);
    assert_eq!(drained, vec![seeded_rid]);

    // A second drain returns empty — the first drain emptied the set.
    assert!(manager.drain_dirty_set(100).is_empty());

    // get_or_create_dirty_set is `pub(super)` to `index_manager` and not
    // callable from this module; drive it through its production caller
    // (`plan_record_created`'s capture path) instead: register a Building
    // in-flight index 200 and prove the lazy dirty-set creation still works
    // through the recovered outer lock.
    let index_def = IndexDefinition::new(200, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def.clone()).await.unwrap();
    let mut building = index_def.clone();
    building.state = IndexState::Building;
    manager.indexes.add_index(building);
    manager.mark_build_in_flight(200);

    let rid_new = RecordId::new();
    let value = test_value(&[(1, InnerValue::Str("alice".into()))]);
    manager.plan_record_created(&rid_new, &value).await.unwrap();

    let drained = manager.drain_dirty_set(200);
    assert_eq!(drained, vec![rid_new]);
}

#[tokio::test]
async fn plan_capture_into_poisoned_dirty_set_succeeds() {
    let manager = create_manager();

    // Mirror p1058_in_flight_build_registry_tests.rs: register a regular
    // index, flip it to Building, mark the build in flight.
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def.clone()).await.unwrap();
    let mut building = index_def.clone();
    building.state = IndexState::Building;
    manager.indexes.add_index(building);
    manager.mark_build_in_flight(1001);

    // Poison the OUTER dirty_sets map, seeding index 1001's INNER set with
    // one RecordId left behind by the panicking thread.
    let seeded_rid = RecordId::new();
    poison_guard(&manager.dirty_sets, move |m| {
        m.insert(1001, Arc::new(Mutex::new(BTreeSet::from([seeded_rid]))));
    });

    // Lookup-path planning succeeds through the poisoned guard.
    let rid_new = RecordId::new();
    let value = test_value(&[(1, InnerValue::Str("alice".into()))]);
    manager.plan_record_created(&rid_new, &value).await.unwrap();

    // The drain returns BOTH the seeded rid and the newly captured one.
    let drained = manager.drain_dirty_set(1001);
    assert!(drained.contains(&seeded_rid), "seeded rid must be observed");
    assert!(drained.contains(&rid_new), "new rid must be captured");
    assert_eq!(drained.len(), 2);
}
