//! RV-STORAGE-98f6650d regression tests: striped-revision validation of
//! tagged cache entries in `MemBufferStore`.
//!
//! Each test below documents exactly which production properties it pins
//! down; the shared properties exercised across the first three tests are:
//! 1. cache hits are accepted ONLY when the stripe stamp is unchanged
//!    across the read (pre-dirty `s0` == post-lookup `s1`), the packed
//!    active-writer count is 0, the entry's `{rev, Slot}` tag matches the
//!    current revision, and the stripe is not exhausted;
//! 2. clean fills are tagged with the per-key stamp captured BEFORE the
//!    authoritative dirty re-probe / backing lookup (so a stale physical
//!    entry is harmless);
//! 3. reads probe the authoritative dirty overlay directly FIRST —
//!    `dirty_count` is never correctness evidence;
//! 4. writers never republish into the moka cache (dirty is the authority).
//!
//! The revision-primitive units at the bottom pin the `begin`/`end` CAS
//! epoch behavior (both-edge revision bump, RAII, exhaustion latch)
//! separately.

use crate::error::DbResult;
use crate::storage_in_memory::InMemoryStore;
use crate::storage_membuffer::{FillPauseHook, MemBufferStore};
use crate::types::{RecordKey, Store};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use super::common::{no_flush_config, rk};

/// Delegates everything to an inner `InMemoryStore` EXCEPT `get`/`get_many`:
/// the FIRST backing read snapshots the inner result FIRST, then takes the
/// ONE-SHOT gate out of the setup-only `Mutex<Option<_>>` slot, signals the
/// test through the `entered` oneshot, and parks on the `release` oneshot
/// (no sleeps, no yield loops). The frozen (stale) result therefore can't
/// be affected by anything the writer or the background flusher do while
/// the reader is parked. Subsequent backing reads find the slot empty and
/// delegate straight to the inner store — the gate can only ever park ONE
/// read, so post-release reads can never hang on a missing permit.
struct GatedReadStore {
    inner: Arc<InMemoryStore>,
    /// One-use gate: `(entered_tx, release_rx)`. Taken by the first backing
    /// read; the test holds the complementary `entered_rx`/`release_tx`.
    gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

impl GatedReadStore {
    /// Build the store and hand back its single gate's halves: `entered_rx`
    /// fires when the first backing read has frozen its snapshot and
    /// parked; the test unblocks it via `release_tx`.
    fn new(inner: Arc<InMemoryStore>) -> (Arc<Self>, oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let store = Arc::new(Self {
            inner,
            gate: Mutex::new(Some((entered_tx, release_rx))),
        });
        (store, entered_rx, release_tx)
    }

    /// One-use park: consume the gate (if still present), signal `entered`,
    /// wait for `release`. Every later call is a no-op passthrough. The
    /// `MutexGuard` is dropped in a scoped block BEFORE the `.await` so the
    /// future stays `Send`.
    async fn park_after_snapshot(&self) {
        let gate = {
            let mut slot = self.gate.lock().expect("gate mutex poisoned");
            slot.take()
        };
        if let Some((entered_tx, release_rx)) = gate {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }
}

#[async_trait]
impl Store for GatedReadStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        // Snapshot FIRST, then park — the frozen (stale) result must not be
        // affected by anything that happens while the reader is parked.
        let val = self.inner.get(key).await;
        self.park_after_snapshot().await;
        val
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }

    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        let vals = self.inner.get_many(keys).await;
        self.park_after_snapshot().await;
        vals
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, crate::error::DbError>> + Send>>
    {
        self.inner.iter_stream(batch_size)
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, crate::error::DbError>> + Send>>
    {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

/// Spawn `fut`, wait deterministically (oneshot) until the gated backing
/// read has frozen its snapshot and parked, then return the join handle.
/// `poll!`/timeout-free: the `entered` oneshot is the scheduling evidence.
async fn spawn_parked_reader<F, T>(
    fut: F,
    entered_rx: oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handle = tokio::spawn(fut);
    let _ = entered_rx.await;
    handle
}

// ============================================================================
// 1. Stale None fill vs acknowledged set (single AND vectored): a reader
// parked inside the backing read with a frozen EMPTY snapshot must not
// poison the cache after the writer's acknowledged set completes and the
// reader resumes. Exact properties pinned here: the immediate post-release
// read must answer from dirty-first (3) — the stale Tombstone fill must not
// mask the acknowledged value — and after drain + explicit eviction the
// validated tag check (1) with pre-backing-lookup fill tagging (2) must
// reject the stale entry so the fresh backing fill is served.
// ============================================================================
#[tokio::test]
async fn rv_storage_98f6650d_stale_none_fill_rejected_after_acked_set_single_and_vectored() {
    // ---- single-key variant ----
    let (mock, entered_rx, release_tx) = GatedReadStore::new(Arc::new(InMemoryStore::new()));
    let store = Arc::new(MemBufferStore::new(mock, no_flush_config()));

    let key = rk(b"k");
    let reader = spawn_parked_reader(
        {
            let store = store.clone();
            let key = key.clone();
            async move { store.get(key).await }
        },
        entered_rx,
    )
    .await;
    // Reader is parked inside the backing get with a frozen NotFound.

    // Acknowledged writer — completes while the reader is parked.
    store
        .set(key.clone(), Bytes::from_static(b"v2"))
        .await
        .unwrap();

    let _ = release_tx.send(());
    let _ = reader.await.unwrap(); // the reader's own stale NotFound is fine

    // The stale Tombstone fill must never be served: dirty-first returns
    // the writer's value, and even after a drain (dirty emptied, backing
    // now holds v2) the stale tagged entry is rejected.
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"v2"),
        "stale None fill must not mask the acknowledged set"
    );
    store.flush().await.unwrap();
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"v2"),
        "stale tagged Tombstone must be rejected after drain"
    );
    // After explicit cache eviction, repeated reads still return the
    // writer's bytes (fresh backing fill).
    store.evict_from_cache_for_test(&key).await;
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"v2")
    );
    let got_many = store.get_many(vec![key.clone()]).await.unwrap();
    assert_eq!(got_many, vec![Some(Bytes::from_static(b"v2"))]);

    // ---- vectored variant: same interleaving through get_many ----
    let (mock2, entered2_rx, release2_tx) = GatedReadStore::new(Arc::new(InMemoryStore::new()));
    let store2 = Arc::new(MemBufferStore::new(mock2, no_flush_config()));
    let key2 = rk(b"k");
    let reader2 = spawn_parked_reader(
        {
            let store = store2.clone();
            let key = key2.clone();
            async move { store.get_many(vec![key]).await }
        },
        entered2_rx,
    )
    .await;
    store2
        .set(key2.clone(), Bytes::from_static(b"v2"))
        .await
        .unwrap();
    let _ = release2_tx.send(());
    let _ = reader2.await.unwrap();
    assert_eq!(
        store2.get_many(vec![key2.clone()]).await.unwrap(),
        vec![Some(Bytes::from_static(b"v2"))]
    );
    store2.flush().await.unwrap();
    store2.evict_from_cache_for_test(&key2).await;
    assert_eq!(
        store2.get_many(vec![key2]).await.unwrap(),
        vec![Some(Bytes::from_static(b"v2"))]
    );
}

// ============================================================================
// 2. Stale positive fill vs acknowledged remove: a reader parked inside the
// backing read with a frozen Some(v1) snapshot must not resurrect a removed
// key. Exact properties pinned here: the immediate post-release read is
// answered by the dirty tombstone (dirty-first, 3); after drain + explicit
// eviction the stale tagged Live fill is rejected by the tag/stamp
// validation (1) and the fresh backing (negative) fill is served.
// ============================================================================
#[tokio::test]
async fn rv_storage_98f6650d_stale_positive_fill_rejected_after_acked_remove() {
    let inner = Arc::new(InMemoryStore::new());
    let (mock, entered_rx, release_tx) = GatedReadStore::new(inner.clone());
    let store = Arc::new(MemBufferStore::new(mock, no_flush_config()));

    let key = rk(b"k");
    inner
        .set(key.clone(), Bytes::from_static(b"v1"))
        .await
        .unwrap();

    let reader = spawn_parked_reader(
        {
            let store = store.clone();
            let key = key.clone();
            async move { store.get_many(vec![key]).await }
        },
        entered_rx,
    )
    .await;
    // Reader parked with frozen Some(v1).

    // Acknowledged remove completes while the reader is parked.
    // (Best-effort `bool` is false here — the key was only in backing — so
    // it is not asserted.)
    let _ = store.remove(key.clone()).await.unwrap();

    let _ = release_tx.send(());
    let reader_out = reader.await.unwrap().unwrap();
    // The reader's own return value may fleetingly be Some(v1) (its frozen
    // snapshot predates the remove) — accepted; what matters is that the
    // stale fill is never served afterwards.
    let _ = reader_out;

    // Dirty tombstone answers authoritatively.
    assert!(
        matches!(
            store.get(key.clone()).await,
            Err(crate::error::DbError::NotFound(_))
        ),
        "stale positive fill must not resurrect the removed key"
    );
    store.flush().await.unwrap();
    assert!(
        matches!(
            store.get(key.clone()).await,
            Err(crate::error::DbError::NotFound(_))
        ),
        "stale tagged Live fill must be rejected after drain"
    );
    store.evict_from_cache_for_test(&key).await;
    assert!(matches!(
        store.get(key.clone()).await,
        Err(crate::error::DbError::NotFound(_))
    ));
    let got_many = store.get_many(vec![key]).await.unwrap();
    assert_eq!(got_many, vec![None]);
}

// ============================================================================
// 3. Delayed OLD insertion at the final publication boundary: an old clean
// fill is parked at the one-shot FillPauseHook immediately BEFORE the
// physical moka insertion. While it is parked the writer's acknowledged set
// completes, the dirty buffer drains to backing, AND a fresh fill ACTUALLY
// completes (the gate is one-shot: the fresh fill passes the consumed hook
// and its moka insertion finishes) — only THEN is the old fill released, so
// its insertion lands on top of a fully-published correct entry. Exact
// property pinned here: correctness comes from TAG VALIDATION of the fill's
// pre-backing-lookup stamp (2) on the next hit (1), NOT from insertion
// ordering — this specifically defeats a check-then-insert "fix" that only
// guards insertion order. Also pins that writers never republish into the
// cache (4): the fresh entry comes from a clean fill, not the writer.
// ============================================================================

// ============================================================================
// 3 (final). Delayed OLD insertion at the final publication boundary —
// see the doc comment above.
// ============================================================================
#[tokio::test]
async fn rv_storage_98f6650d_delayed_old_fill_tag_rejected_after_writer_and_fresh_fill() {
    // Non-gated backing: the pause point we need is the FillPauseHook at the
    // final publication boundary, not the backing read.
    let inner = Arc::new(InMemoryStore::new());
    let store = Arc::new(MemBufferStore::new(inner, no_flush_config()));
    let (hook, entered_rx, release_tx) = FillPauseHook::new();
    store.set_fill_pause_hook(Some(hook));
    store.disable_background_flusher_for_test();

    let key = rk(b"k");
    store
        .set(key.clone(), Bytes::from_static(b"v_old"))
        .await
        .unwrap();
    store.flush().await.unwrap();

    // Old clean fill parks immediately BEFORE the physical cache insertion.
    let old_fill = {
        let store = store.clone();
        let key = key.clone();
        tokio::spawn(async move { store.get(key).await })
    };
    let _ = entered_rx.await;

    // Acknowledged writer completes while the old fill is parked.
    store
        .set(key.clone(), Bytes::from_static(b"v2"))
        .await
        .unwrap();
    // Drain so the writer's value reaches backing and dirty is empty.
    store.flush().await.unwrap();
    // A FRESH fill runs while the old fill is still parked — the hook is
    // one-shot and already consumed, so the fresh fill passes straight
    // through and its post-writer-tagged moka insertion FULLY completes
    // before we release the old insertion (the assertion below is the
    // completion evidence; release comes strictly after it).
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"v2")
    );

    // Resume the OLD insertion — it must not overwrite observably.
    let _ = release_tx.send(());
    let _ = old_fill.await.unwrap().unwrap();

    // The old tagged entry (pre-writer revision) must be rejected on every
    // subsequent read, with and without a drain/eviction in between.
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"v2"),
        "delayed old insertion must not be served (tag validation)"
    );
    store.flush().await.unwrap();
    store.evict_from_cache_for_test(&key).await;
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"v2")
    );
    let got_many = store.get_many(vec![key]).await.unwrap();
    assert_eq!(got_many, vec![Some(Bytes::from_static(b"v2"))]);
}

// ============================================================================
// 4. Striped revision primitives: packed {revision, active} CAS epochs.
// Reverting the begin/end CAS pair (revision bump on EITHER edge or
// active-count accounting), the RAII Drop on panic/cancel, or the
// fail-closed exhaustion latch makes these fail:
//  * while any writer epoch is open the packed active count is > 0 and NO
//    cache entry validates (acceptable() == false);
//  * the revision bumps on BOTH edges: a stamp captured DURING a writer
//    (mid-epoch) must still be rejected after the writer's guard drops —
//    an `end` that leaves the revision unchanged makes that stamp
//    validate (exact revert caught by the during/after test);
//  * a cancelled writer's guard Drop still bumps the revision and
//    decrements (no leaked active);
//  * at the revision boundary REV_MAX-1, begin moves to REV_MAX (still
//    usable) and end saturates there while latching exhaustion — actual
//    near-exhaustion boundary, not just the forced latch;
//  * at the active-count boundary the guard is uncounted (no overflow,
//    no underflow) and the stripe latches exhausted;
//  * an exhausted stripe never validates again, even for a perfectly
//    matching stamp/revision.
// ============================================================================
mod rv_storage_98f6650d_revision_units {
    use crate::storage_membuffer::StripedRevisions;
    use crate::types::RecordKey;
    use std::sync::Arc;

    fn rk(b: &[u8]) -> RecordKey {
        RecordKey::from_slice(b)
    }

    /// Two DISTINCT keys hashing to the SAME stripe (workspace THasher).
    fn colliding_pair(r: &StripedRevisions) -> (RecordKey, RecordKey) {
        let a = rk(b"rv98f6650d-collide-a");
        for i in 0..100_000u32 {
            let b = rk(format!("rv98f6650d-collide-b-{i}").as_bytes());
            if r.stripe_of(&b) == r.stripe_of(&a) {
                return (a, b);
            }
        }
        panic!("no colliding key found");
    }

    fn active_of(packed: u64) -> u64 {
        packed & 0xFFFF_FFFF
    }

    fn rev_of(packed: u64) -> u64 {
        packed >> 32
    }

    #[test]
    fn rv_storage_98f6650d_overlapping_writers_share_stripe_active_count_never_zero() {
        let r = StripedRevisions::new();
        let (a, b) = colliding_pair(&r);
        assert_ne!(a, b);

        // Before any writer: revision 0, active 0.
        assert_eq!(r.packed_for_test(&a), 0);

        let g1 = r.begin(&a);
        let p1 = r.packed_for_test(&a);
        assert_eq!(active_of(p1), 1, "begin must set active=1");
        assert_eq!(rev_of(p1), 1, "begin must bump the revision");

        // A second, DISTINCT colliding key in the SAME stripe opens a second
        // epoch — the packed count accumulates.
        let g2 = r.begin(&b);
        let p2 = r.packed_for_test(&a);
        assert_eq!(active_of(p2), 2, "overlapping writers must accumulate");
        assert_eq!(rev_of(p2), 2, "each begin bumps the revision once");

        // While a writer epoch is open, NO cache entry validates —
        // acceptable() must be false regardless of the tag.
        let s1 = r.stamp(&a);
        assert!(
            !r.acceptable(s1, s1, rev_of(p2)),
            "in-flight entry must never validate while active > 0"
        );

        // End the first epoch: count stays NONZERO (second still open), and
        // the revision bumps AGAIN (both edges bump).
        drop(g1);
        let p3 = r.packed_for_test(&a);
        assert_eq!(active_of(p3), 1, "count must stay nonzero until all end");
        assert_eq!(rev_of(p3), 3, "end must bump the revision once more");

        drop(g2);
        let p4 = r.packed_for_test(&a);
        assert_eq!(active_of(p4), 0, "count returns to zero only when all end");
        assert_eq!(rev_of(p4), 4, "each end bumps the revision once");

        // With active == 0 and a matching tag, a clean entry validates.
        let s = r.stamp(&a);
        assert!(r.acceptable(s, s, rev_of(p4)));
        // A stale tag (pre-write revision) never validates.
        assert!(!r.acceptable(s, s, rev_of(p4) - 1));
    }

    #[tokio::test]
    async fn rv_storage_98f6650d_cancelled_writer_epoch_releases_via_raii_drop() {
        let r = Arc::new(StripedRevisions::new());
        let a = rk(b"rv98f6650d-cancel");
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());

        let entered2 = Arc::clone(&entered);
        let release2 = Arc::clone(&release);
        let r_arc = Arc::clone(&r);
        let a2 = a.clone();
        let task = tokio::spawn(async move {
            // The guard lives across the await; cancelling the task must
            // drop it (RAII end-CAS).
            let g = r_arc.begin(&a2);
            entered2.notify_one();
            release2.notified().await;
            drop(g);
        });
        // Wait for the guard reference to exist: the epoch is open once the
        // task signals entered (guard created BEFORE the notify).
        entered.notified().await;
        assert_eq!(active_of(r.packed_for_test(&a)), 1);

        // Cancel mid-`.await` — the guard Drop must still run the end-CAS.
        task.abort();
        let _ = task.await;
        assert_eq!(
            active_of(r.packed_for_test(&a)),
            0,
            "cancelled writer must not leak an active count (RAII Drop)"
        );
        assert_eq!(
            rev_of(r.packed_for_test(&a)),
            2,
            "cancelled guard's end still bumps the revision"
        );

        // The stripe remains fully usable afterwards.
        let s = r.stamp(&a);
        assert!(r.acceptable(s, s, rev_of(r.packed_for_test(&a))));
    }

    #[test]
    fn rv_storage_98f6650d_stamp_captured_during_writer_rejected_after_drop() {
        let r = StripedRevisions::new();
        let a = rk(b"rv98f6650d-during");

        let g = r.begin(&a);
        // Stamp captured DURING the writer epoch (active > 0).
        let mid_packed = r.packed_for_test(&a);
        let mid = r.stamp(&a);
        assert_eq!(active_of(mid_packed), 1);
        drop(g);

        let after_packed = r.packed_for_test(&a);
        let after = r.stamp(&a);
        assert_eq!(active_of(after_packed), 0);
        // Exact-revert catcher: an `end` that leaves the revision unchanged
        // would make mid.raw == after.raw and the mid-writer fill valid.
        assert_ne!(
            mid.raw, after.raw,
            "end must bump the revision (stamp captured during writer must differ)"
        );
        assert_eq!(rev_of(after_packed), rev_of(mid_packed) + 1);
        // Exact-revert catcher for the MID TAG (not the active count): both
        // stamps are the post-drop `after` (identical, active == 0), so the
        // ONLY thing that can reject the entry is its stale mid-writer
        // revision tag. Passing `mid` as both stamps instead would reject
        // via `active > 0` and prove nothing about the tag.
        assert!(
            !r.acceptable(after, after, rev_of(mid_packed)),
            "fill tagged with the mid-writer revision must be rejected after the guard drops"
        );
        // A fresh entry tagged with the post-writer revision still validates.
        let fresh = r.stamp(&a);
        assert!(r.acceptable(fresh, fresh, rev_of(r.packed_for_test(&a))));
    }

    #[test]
    fn rv_storage_98f6650d_revision_exhaustion_boundary_begin_and_end() {
        let r = StripedRevisions::new();
        let a = rk(b"rv98f6650d-boundary");
        const REV_MAX: u64 = 0xFFFF_FFFF;
        // Place the stripe one revision short of exhaustion: the ACTUAL
        // begin/end boundary, not the forced latch.
        r.set_packed_for_test(&a, (REV_MAX - 1) << 32);
        assert!(!r.is_exhausted_for_test(&a));

        // begin at REV_MAX-1 -> REV_MAX: valid, NOT exhausted.
        let g = r.begin(&a);
        let p = r.packed_for_test(&a);
        assert_eq!(rev_of(p), REV_MAX);
        assert_eq!(active_of(p), 1);
        assert!(!r.is_exhausted_for_test(&a));
        drop(g);

        // end at REV_MAX: revision SATURATES (no wrapping) and the
        // exhaustion latch fires.
        let p2 = r.packed_for_test(&a);
        assert_eq!(rev_of(p2), REV_MAX, "revision must saturate, never wrap");
        assert_eq!(active_of(p2), 0);
        assert!(r.is_exhausted_for_test(&a));

        // Writers keep working (checked CAS, balanced count), but caching
        // stays permanently disabled for the stripe.
        let g2 = r.begin(&a);
        let p3 = r.packed_for_test(&a);
        assert_eq!(rev_of(p3), REV_MAX, "saturated revision never wraps");
        assert_eq!(active_of(p3), 1);
        drop(g2);
        let p4 = r.packed_for_test(&a);
        assert_eq!(rev_of(p4), REV_MAX);
        assert_eq!(active_of(p4), 0);
        let s = r.stamp(&a);
        assert!(!r.acceptable(s, s, rev_of(r.packed_for_test(&a))));
    }

    #[test]
    fn rv_storage_98f6650d_active_overflow_uncounted_guard_latches_exhausted() {
        let r = StripedRevisions::new();
        let a = rk(b"rv98f6650d-activemax");
        const ACTIVE_MAX: u64 = 0xFFFF_FFFF;
        // Active field at its maximum: begin must NOT overflow.
        r.set_packed_for_test(&a, (0xFFFF_FFFDu64 << 32) | ACTIVE_MAX);

        let g = r.begin(&a);
        let p = r.packed_for_test(&a);
        assert!(
            r.is_exhausted_for_test(&a),
            "active overflow latches exhausted"
        );
        assert_eq!(
            active_of(p),
            ACTIVE_MAX,
            "uncounted guard must not overflow the active field"
        );
        drop(g);
        let p2 = r.packed_for_test(&a);
        assert_eq!(
            active_of(p2),
            ACTIVE_MAX,
            "uncounted guard's end must not decrement (no underflow)"
        );
        assert_eq!(
            rev_of(p2),
            0xFFFF_FFFF,
            "both edges still bump the revision (begin +1, end +1)"
        );
        let s = r.stamp(&a);
        assert!(!r.acceptable(s, s, rev_of(r.packed_for_test(&a))));
    }

    #[test]
    fn rv_storage_98f6650d_revision_exhaustion_fails_closed() {
        let r = StripedRevisions::new();
        let a = rk(b"rv98f6650d-exhaust");
        // Force the stripe to the exhaustion boundary (fail-closed latch),
        // as if 2^32 revisions had been consumed.
        r.force_exhaust_for_test(&a);

        // No entry validates on an exhausted stripe — even a perfectly
        // matching stamp and revision.
        let s = r.stamp(&a);
        let p = r.packed_for_test(&a);
        assert!(
            !r.acceptable(s, s, rev_of(p)),
            "exhausted stripe must fail closed"
        );

        // Writers keep working: begin/end still CAS (revision saturates at
        // REV_MAX, never wraps), the guard RAII still balances, and the
        // stripe stays exhausted.
        let g = r.begin(&a);
        let p2 = r.packed_for_test(&a);
        assert_eq!(active_of(p2), 1);
        assert_eq!(rev_of(p2), 0xFFFF_FFFF, "saturated revision never wraps");
        drop(g);
        assert_eq!(active_of(r.packed_for_test(&a)), 0);
        assert_eq!(rev_of(r.packed_for_test(&a)), 0xFFFF_FFFF);
        let s2 = r.stamp(&a);
        assert!(
            !r.acceptable(s2, s2, rev_of(r.packed_for_test(&a))),
            "exhaustion latches permanently — wrapped stamps never validate"
        );
    }
}
