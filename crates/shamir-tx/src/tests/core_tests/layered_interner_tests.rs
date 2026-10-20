use crate::layered_interner::{commit_interner_overlay, LayeredInterner, OVERLAY_ID_BASE};
use scc::HashMap as SccHashMap;
use shamir_collections::THasher;
use shamir_types::core::interner::Interner;
use std::sync::atomic::{AtomicU64, Ordering};

/// Returns a `LayeredInterner` in Layered mode.
/// The caller must keep `overlay` and `next` alive for the lifetime
/// of `layered_interner`.
fn make_layered<'a>(
    base: &'a Interner,
    overlay: &'a SccHashMap<String, u64, THasher>,
    next: &'a AtomicU64,
) -> LayeredInterner<'a> {
    LayeredInterner::Layered {
        base,
        overlay,
        next_overlay_id: next,
    }
}

#[test]
fn touch_sync_same_as_async() {
    let base = Interner::new();
    let overlay = SccHashMap::with_hasher(THasher::default());
    let next = AtomicU64::new(OVERLAY_ID_BASE);
    let li = make_layered(&base, &overlay, &next);

    let id = li.touch_sync("sync_key");
    assert!(id >= OVERLAY_ID_BASE);

    // Same key returns same id
    let id2 = li.touch_sync("sync_key");
    assert_eq!(id, id2);
}

#[test]
fn touch_sync_direct_returns_base_id() {
    let base = Interner::new();
    let li = LayeredInterner::Direct(&base);
    let id = li.touch_sync("hello");
    assert!(id < OVERLAY_ID_BASE);
    let got = base.get_ind("hello").expect("should exist in base");
    assert_eq!(got.id(), id);
}

#[tokio::test]
async fn touch_and_touch_sync_equivalent_on_shared_overlay() {
    // H6 DEADLOCK FIX guard. The async `touch` now delegates to the
    // SYNCHRONOUS `touch_sync` (both acquire the per-`TxContext` overlay
    // map's bucket lock via the synchronous `entry_sync`, NOT
    // `entry_async`'s lock-HANDOFF path — see the DEADLOCK FIX note on
    // `touch`). This is a behavioral-equivalence guard on a SHARED overlay,
    // NOT a concurrency stress test: the overlay map is per-`TxContext` and
    // single-task-at-a-time today, so there is no realistic cross-task
    // interleaving to exercise. The invariant pinned here is that the two
    // entry points agree: whichever allocates a key first wins, the other
    // observes the occupied entry — i.e. `touch` is now a faithful wrapper
    // around `touch_sync`, so they can never diverge on id allocation.
    let base = Interner::new();
    let overlay = SccHashMap::with_hasher(THasher::default());
    let next = AtomicU64::new(OVERLAY_ID_BASE);
    let li = make_layered(&base, &overlay, &next);

    // `touch` allocates, `touch_sync` then observes the occupied entry.
    let id_async_first = li.touch("k1").await;
    let id_sync_then = li.touch_sync("k1");
    assert_eq!(id_async_first, id_sync_then);

    // Reverse order: `touch_sync` allocates, `touch` then observes it.
    let id_sync_first = li.touch_sync("k2");
    let id_async_then = li.touch("k2").await;
    assert_eq!(id_sync_first, id_async_then);

    // Both new allocations are overlay ids; they are distinct keys.
    assert!(id_async_first >= OVERLAY_ID_BASE);
    assert!(id_sync_first >= OVERLAY_ID_BASE);
    assert_ne!(id_async_first, id_sync_first);
}

#[tokio::test]
async fn direct_mode_no_overhead() {
    let base = Interner::new();
    let li = LayeredInterner::Direct(&base);

    let id = li.touch("hello").await;
    assert!(id < OVERLAY_ID_BASE);

    let got = base.get_ind("hello").expect("should exist in base");
    assert_eq!(got.id(), id);
}

#[tokio::test]
async fn layered_touch_new_goes_to_overlay() {
    let base = Interner::new();
    let overlay = SccHashMap::with_hasher(THasher::default());
    let next = AtomicU64::new(OVERLAY_ID_BASE);
    let li = make_layered(&base, &overlay, &next);

    let id = li.touch("brand_new_key").await;
    assert!(
        id >= OVERLAY_ID_BASE,
        "overlay id must be >= OVERLAY_ID_BASE"
    );

    assert!(
        base.get_ind("brand_new_key").is_none(),
        "base must not know the key yet"
    );
    assert!(
        overlay.read_async("brand_new_key", |_, v| *v).await == Some(id),
        "overlay should contain the key"
    );
}

#[tokio::test]
async fn layered_touch_existing_in_base_returns_base_id() {
    let base = Interner::new();
    let base_id = base
        .touch_ind("foo")
        .expect("touch_ind succeeds")
        .key()
        .id();

    let overlay = SccHashMap::with_hasher(THasher::default());
    let next = AtomicU64::new(OVERLAY_ID_BASE);
    let li = make_layered(&base, &overlay, &next);
    let id = li.touch("foo").await;
    assert_eq!(id, base_id);
    assert!(id < OVERLAY_ID_BASE);
}

#[tokio::test]
async fn layered_touch_repeat_returns_same_overlay_id() {
    let base = Interner::new();
    let overlay = SccHashMap::with_hasher(THasher::default());
    let next = AtomicU64::new(OVERLAY_ID_BASE);
    let li = make_layered(&base, &overlay, &next);

    let id1 = li.touch("bar").await;
    let id2 = li.touch("bar").await;
    assert_eq!(id1, id2);
    assert!(id1 >= OVERLAY_ID_BASE);
}

#[tokio::test]
async fn get_id_does_not_allocate() {
    let base = Interner::new();
    let overlay = SccHashMap::with_hasher(THasher::default());
    let next = AtomicU64::new(OVERLAY_ID_BASE);
    let li = make_layered(&base, &overlay, &next);

    assert!(li.get_id("unknown").await.is_none());

    assert!(base.get_ind("unknown").is_none());
    assert!(overlay.is_empty());
}

#[tokio::test]
async fn get_str_reads_base_and_overlay() {
    let base = Interner::new();
    let base_id = base
        .touch_ind("foo")
        .expect("touch_ind succeeds")
        .key()
        .id();

    let overlay = SccHashMap::with_hasher(THasher::default());
    let next = AtomicU64::new(OVERLAY_ID_BASE);
    let li = make_layered(&base, &overlay, &next);
    let overlay_id = li.touch("bar").await;

    assert_eq!(li.get_str(base_id), Some("foo".to_string()));
    assert_eq!(li.get_str(overlay_id), Some("bar".to_string()));
}

#[tokio::test]
async fn commit_overlay_merges_into_base() {
    let base = Interner::new();
    let overlay = SccHashMap::with_hasher(THasher::default());
    let next = AtomicU64::new(OVERLAY_ID_BASE);

    let overlay_a = next.fetch_add(1, Ordering::SeqCst);
    overlay
        .insert_async("a".to_string(), overlay_a)
        .await
        .unwrap();
    let overlay_b = next.fetch_add(1, Ordering::SeqCst);
    overlay
        .insert_async("b".to_string(), overlay_b)
        .await
        .unwrap();

    let result = commit_interner_overlay(&base, &overlay).await.unwrap();
    assert_eq!(result.remap.len(), 2);

    let final_a = base.get_ind("a").expect("a should be in base").id();
    let final_b = base.get_ind("b").expect("b should be in base").id();
    assert_eq!(result.remap[&overlay_a], final_a);
    assert_eq!(result.remap[&overlay_b], final_b);
    // Both entries are new to base.
    assert_eq!(result.delta.len(), 2);
}

#[tokio::test]
async fn commit_overlay_with_race_uses_existing_base_id() {
    let base = Interner::new();
    let existing = base
        .touch_ind("foo")
        .expect("touch_ind succeeds")
        .key()
        .id();

    let overlay = SccHashMap::with_hasher(THasher::default());
    let overlay_id: u64 = OVERLAY_ID_BASE + 99;
    overlay
        .insert_async("foo".to_string(), overlay_id)
        .await
        .unwrap();

    let result = commit_interner_overlay(&base, &overlay).await.unwrap();
    assert_eq!(result.remap[&overlay_id], existing);
    // "foo" already existed in base — delta should be empty.
    assert!(result.delta.is_empty());
}

#[tokio::test]
async fn commit_overlay_empty_is_noop() {
    let base = Interner::new();
    let overlay = SccHashMap::with_hasher(THasher::default());
    let base_len = base.len();

    let result = commit_interner_overlay(&base, &overlay).await.unwrap();
    assert!(result.remap.is_empty());
    assert!(result.delta.is_empty());
    assert_eq!(base.len(), base_len);
}
