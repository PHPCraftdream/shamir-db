//! RV-INDEX-D3ED367D: posting-cache generation protocol regressions.
//!
//! Cache entries are tagged with the shared invalidation epoch; a HIT is
//! served only when the tag still matches the current epoch, and writers
//! bump the epoch BEFORE mutating the store. A reader that scanned the
//! pre-bump state can never publish into, or be served from, a cache entry
//! that outlives the invalidation.

use bytes::Bytes;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::index_keys::{build_index_key, build_posting_key};
use crate::base_index::infra::posting_cache::{POSTING_CACHE_BUDGET, POSTING_CACHE_CAP};
use crate::base_index::tests::index_manager_tests::helpers::{create_manager, create_test_value};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

#[tokio::test]
async fn rv_index_d3ed367d_scan_racing_writer_cannot_publish_stale_cache() {
    let (_, _, manager) = create_manager();
    manager
        .create_index(IndexDefinition::new(
            1001,
            vec![IndexInfoItem::new(vec![1])],
        ))
        .await
        .unwrap();

    let a = RecordId::new();
    manager
        .on_record_created(&a, &create_test_value(&[(1, InnerValue::Int(7))]))
        .await
        .unwrap();

    let hook = Arc::new(crate::base_index::backfill_pause_hook::BackfillPauseHook::new());
    manager.set_lookup_post_scan_pause_hook(Some(Arc::clone(&hook)));
    let reader_manager = manager.clone();
    let reader = tokio::spawn(async move {
        reader_manager
            .lookup_by_index(1001, &[InnerValue::Int(7)])
            .await
            .unwrap()
            .unwrap()
    });
    hook.wait_until_parked().await;

    // Exercise the production planner and apply_ops path while the scan is
    // parked after observing the old store contents.
    let b = RecordId::new();
    manager
        .on_record_created(&b, &create_test_value(&[(1, InnerValue::Int(7))]))
        .await
        .unwrap();
    hook.release();
    let raced = reader.await.unwrap();
    assert_eq!(
        raced.len(),
        1,
        "the paused scan observed the pre-write state"
    );

    manager.set_lookup_post_scan_pause_hook(None);
    let fresh = manager
        .lookup_by_index(1001, &[InnerValue::Int(7)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fresh.len(),
        2,
        "subsequent lookup must re-scan, not hit stale publication"
    );
    assert!(fresh.contains(&a));
    assert!(fresh.contains(&b));
}

/// Old-code failure: the unversioned cache served the pre-write Arc forever,
/// so a posting added between cache warm-up and the next lookup was silently
/// missing. New code: the writer's epoch bump retires the stale tag and the
/// miss path re-scans.
#[tokio::test]
async fn rv_index_d3ed367d_stale_cache_entry_not_served_after_generation_bump() {
    let (_, _, manager) = create_manager();
    manager
        .create_index(IndexDefinition::new(
            1001,
            vec![IndexInfoItem::new(vec![1])],
        ))
        .await
        .unwrap();

    let a = RecordId::new();
    manager
        .on_record_created(&a, &create_test_value(&[(1, InnerValue::Int(7))]))
        .await
        .unwrap();
    let warm = manager
        .lookup_by_index(1001, &[InnerValue::Int(7)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(warm.len(), 1, "cache must be warm with only `a`");

    // Scan-visible new posting written straight to info_store (bypasses the
    // write hooks, so no per-key invalidation fires on its own).
    let b = RecordId::new();
    let index_key = build_index_key(false, 1001, &[InnerValue::Int(7)])
        .expect("probe has identity")
        .to_bytes();
    let posting_key = build_posting_key(&index_key, &b);
    manager
        .info_store
        .set(posting_key.into(), Bytes::new())
        .await
        .unwrap();

    // The bump-before-mutation step every posting-write path now performs
    // (apply_ops, direct backfills, sweeps, external invalidation helpers).
    manager
        .posting_cache_generation
        .fetch_add(1, Ordering::AcqRel);

    let fresh = manager
        .lookup_by_index(1001, &[InnerValue::Int(7)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fresh.len(),
        2,
        "lookup after the epoch bump must reflect the store, not the stale tag"
    );
    assert!(fresh.contains(&b), "new posting must be visible");
    assert!(fresh.contains(&a), "original posting must remain visible");
}

/// generation counter is shared across manager clones: a bump through one
/// clone must invalidate entries observed through every other clone.
#[tokio::test]
async fn rv_index_d3ed367d_generation_is_shared_across_manager_clones() {
    let (_, _, manager) = create_manager();
    let clone = manager.clone();
    let before = clone.posting_cache_generation.load(Ordering::Acquire);
    manager
        .posting_cache_generation
        .fetch_add(1, Ordering::AcqRel);
    assert_eq!(
        clone.posting_cache_generation.load(Ordering::Acquire),
        before + 1,
        "clones must observe the same cache epoch"
    );
    assert!(Arc::ptr_eq(&manager.posting_cache, &clone.posting_cache));
}

#[test]
fn posting_cache_accounting_replacement_removal_and_eviction() {
    let (_, _, manager) = create_manager();
    let cache = &manager.posting_cache;
    let key = Bytes::from_static(b"one");
    cache.insert(key.clone(), 1, Arc::from(vec![RecordId::new(); 10]));
    let first = cache.accounted_bytes();
    assert!(first > 10 * std::mem::size_of::<RecordId>());
    cache.insert(key.clone(), 2, Arc::from(vec![RecordId::new(); 20]));
    assert_eq!(
        cache.accounted_bytes(),
        first + 10 * std::mem::size_of::<RecordId>()
    );
    cache.remove(&key);
    assert_eq!(cache.accounted_bytes(), 0);

    for i in 0..=POSTING_CACHE_CAP {
        cache.insert(Bytes::from(i.to_be_bytes().to_vec()), 0, Arc::from([]));
    }
    assert_eq!(cache.len(), POSTING_CACHE_CAP);
    assert!(cache.accounted_bytes() > 0);
    cache.retain_prefix_excluding(&[]);
    assert_eq!(cache.accounted_bytes(), 0);
}

#[test]
fn posting_cache_byte_pressure_evicts_below_entry_limit() {
    let (_, _, manager) = create_manager();
    let cache = &manager.posting_cache;
    let count = POSTING_CACHE_BUDGET / (2 * std::mem::size_of::<RecordId>());
    let values: Arc<[RecordId]> = Arc::from(vec![RecordId::new(); count]);
    cache.insert(Bytes::from_static(b"first"), 0, Arc::clone(&values));
    assert!(cache.get(&Bytes::from_static(b"first")).is_some());
    cache.insert(Bytes::from_static(b"second"), 0, values);
    assert!(cache.get(&Bytes::from_static(b"first")).is_none());
    assert!(cache.get(&Bytes::from_static(b"second")).is_some());
    assert_eq!(cache.len(), 1);
    assert!(cache.accounted_bytes() <= POSTING_CACHE_BUDGET);
}

#[test]
fn posting_cache_oversized_bypasses_admission() {
    let (_, _, manager) = create_manager();
    let count = POSTING_CACHE_BUDGET / std::mem::size_of::<RecordId>() + 1;
    let values: Arc<[RecordId]> = Arc::from(vec![RecordId::new(); count]);
    manager
        .posting_cache
        .insert(Bytes::from_static(b"large"), 0, Arc::clone(&values));
    assert!(manager
        .posting_cache
        .get(&Bytes::from_static(b"large"))
        .is_none());
    assert_eq!(manager.posting_cache.accounted_bytes(), 0);
    assert_eq!(values.len(), count);
}

#[test]
fn posting_cache_concurrent_admission_never_exceeds_budget() {
    let (_, _, manager) = create_manager();
    let cache = Arc::clone(&manager.posting_cache);
    std::thread::scope(|scope| {
        for worker in 0..8_u8 {
            let cache = Arc::clone(&cache);
            scope.spawn(move || {
                for n in 0..64_u8 {
                    let values: Arc<[RecordId]> = Arc::from(vec![RecordId::new(); 32768]);
                    cache.insert(Bytes::from(vec![worker, n]), 0, values);
                    assert!(cache.accounted_bytes() <= POSTING_CACHE_BUDGET);
                }
            });
        }
    });
    assert!(cache.accounted_bytes() <= POSTING_CACHE_BUDGET);
    cache.retain_prefix_excluding(&[]);
    assert_eq!(cache.accounted_bytes(), 0);
}

#[tokio::test]
async fn posting_cache_invalidation_paths_release_charge() {
    let (_, _, manager) = create_manager();
    let def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(def).await.unwrap();
    let value = create_test_value(&[(1, InnerValue::Int(7))]);
    let id = RecordId::new();
    manager.on_record_created(&id, &value).await.unwrap();
    let index_key = build_index_key(false, 1001, &[InnerValue::Int(7)])
        .expect("probe has identity")
        .to_bytes();
    let posting_key = build_posting_key(&index_key, &id);
    let _ = manager
        .lookup_by_index(1001, &[InnerValue::Int(7)])
        .await
        .unwrap();
    assert!(manager.posting_cache.accounted_bytes() > 0);
    manager.invalidate_posting_cache_for_keys([posting_key.as_ref()]);
    assert_eq!(manager.posting_cache.accounted_bytes(), 0);
    let _ = manager
        .lookup_by_index(1001, &[InnerValue::Int(7)])
        .await
        .unwrap();
    let ops = manager.plan_record_deleted(&id, &value).await.unwrap();
    manager.invalidate_posting_cache_for_ops(&ops);
    assert_eq!(manager.posting_cache.accounted_bytes(), 0);
}

#[tokio::test]
async fn posting_cache_drop_recreation_releases_resident_charge() {
    let (_, _, manager) = create_manager();
    let def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(def.clone()).await.unwrap();
    let old_id = RecordId::new();
    let value = create_test_value(&[(1, InnerValue::Int(7))]);
    manager.on_record_created(&old_id, &value).await.unwrap();
    let held = manager
        .lookup_by_index(1001, &[InnerValue::Int(7)])
        .await
        .unwrap()
        .unwrap();
    assert!(manager.posting_cache.accounted_bytes() > 0);
    manager.drop_index(1001, None, None).await.unwrap();
    assert_eq!(manager.posting_cache.accounted_bytes(), 0);
    assert!(held.contains(&old_id));
    manager.create_index(def).await.unwrap();
    let new_id = RecordId::new();
    manager.on_record_created(&new_id, &value).await.unwrap();
    let fresh = manager
        .lookup_by_index(1001, &[InnerValue::Int(7)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&*fresh, &[new_id]);
}
