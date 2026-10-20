#![allow(deprecated)]

use crate::storage_in_memory::InMemoryRepo;
use crate::storage_membuffer::MemBufferStore;
use crate::types::{Repo, Store};
use bytes::Bytes;
use std::sync::Arc;

use super::common::{no_flush_config, rk};
// ============================================================================
// Task #535: `dirty_nonempty` clear-race — `drain_once` must not mask an
// already-ACKed write. The naive `is_empty()` → `store(false)` sequence has a
// check-then-act gap: a writer's `dirty.insert` (preceded by its own
// `store(true)`) can land between the check and the store, leaving a real entry
// in `dirty` while the sentinel wrongly reads `false`. Every subsequent `get()`
// then skips the dirty probe via that false sentinel and stale-misses the write.
//
// The regression is driven deterministically (NO sleep-based timing) via a
// `#[cfg(test)]` `ClearRaceHook` fired inside `drain_once` at the exact clear
// window (after the `is_empty()` observation, BEFORE the `store(false)` that
// follows it): the hook injects the racing writer insert into that gap.
// Pre-fix, the sentinel ends up `false` with the key still in `dirty` and NOT
// yet in `inner`, so the immediate `get()` stale-misses it (NotFound).
// Post-fix, the verify-after-clear re-check observes the raced-in entry and
// restores the sentinel to `true`, so the `get()` finds it. (A second,
// narrower gap in that same fix — a writer that publishes `store(true)` then
// stalls across an `.await` before its own `dirty.insert()` completes, e.g.
// mid-loop in `insert_many`/`set_many`/`remove_many` — is closed separately
// by the writer-side republish-after-insert; this test targets the
// insert-observable-by-the-re-check case specifically.)
// ============================================================================
mod clear_race_535 {
    use super::*;
    use crate::membuffer_clear_race_hook::ClearRaceHook;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, Weak};

    #[tokio::test]
    async fn drain_clear_race_does_not_mask_acked_write() {
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();

        // The racing key K: injected by the hook into `dirty` inside the clear
        // window. It is NEVER written to `inner` up front, so if the sentinel is
        // wrongly cleared and `get(K)` skips the dirty probe, `inner` also
        // misses → NotFound. That NotFound is exactly the masked write.
        let racing_key = rk(b"racing-key-535");
        let racing_val = Bytes::from_static(b"racing-value-535");

        // Late-bound slot so the hook can reach the store built AFTER the hook.
        let store_slot: Arc<Mutex<Weak<MemBufferStore>>> = Arc::new(Mutex::new(Weak::new()));
        // Fire exactly ONCE — the drain-loop is not used here (single
        // `drain_once` pass), but guard defensively so the injected K is not
        // re-inserted on any later drain.
        let fired = Arc::new(AtomicBool::new(false));

        let hook = {
            let store_slot = Arc::clone(&store_slot);
            let fired = Arc::clone(&fired);
            let racing_key = racing_key.clone();
            let racing_val = racing_val.clone();
            ClearRaceHook::install(Arc::new(move || {
                if fired.swap(true, Ordering::SeqCst) {
                    return;
                }
                // Simulate a writer whose `store(true)` + `dirty.insert` raced
                // into the clear gap: publish the sentinel, then land the entry.
                if let Some(store) = store_slot.lock().unwrap().upgrade() {
                    store.inject_racing_dirty_write(racing_key.clone(), racing_val.clone());
                }
            }))
        };

        let buffered = Arc::new(MemBufferStore::new_with_clear_race_hook(
            inner_store,
            no_flush_config(),
            hook,
        ));
        *store_slot.lock().unwrap() = Arc::downgrade(&buffered);

        // Prime `dirty` with an unrelated key J so the single `drain_once`
        // actually flushes something and reaches the clear window.
        let jkey = buffered
            .insert(Bytes::from_static(b"prime-j"))
            .await
            .unwrap();

        // ONE drain pass: flushes J → `dirty` observed empty → `store(false)` →
        // hook injects K into the gap → verify-after-clear must restore the
        // sentinel (post-fix).
        let drained = buffered.drain_once_for_test(usize::MAX).await.unwrap();
        assert_eq!(drained, 1, "the single drain should have flushed only J");

        // The racing key K is now in `dirty` and NOT in `inner`. The sentinel
        // must be TRUE — otherwise `get(K)` (and every overlay scan) masks it.
        assert!(
            buffered.dirty_nonempty_flag(),
            "clear-race masked an ACKed write: dirty holds K but dirty_nonempty=false"
        );

        // End-to-end: the ACKed write must be visible immediately. Pre-fix this
        // returns NotFound (fast-path skips the dirty probe on the false
        // sentinel, and inner never had K).
        let got = buffered.get(racing_key).await.unwrap();
        assert_eq!(
            got.as_ref(),
            b"racing-value-535",
            "racing write must be visible after the clear-race window"
        );

        // Sanity: J did reach inner (the drain flushed it).
        let jkey_owned = jkey;
        assert!(
            buffered.get(jkey_owned).await.is_ok(),
            "primed key J should have been flushed to inner"
        );
    }
}

// ============================================================================
// Task #535 round 2: the narrower stall-across-`.await` gap an `@fl`
// adversarial pass found in round 1's fix. A writer's `dirty_nonempty.
// store(true)` published BEFORE its `dirty.insert()` does not help if the
// writer stalls (an `.await` yield) between the two AND a concurrent
// `drain_once` runs its ENTIRE clear-and-verify sequence inside that stall —
// neither of `drain_once`'s two `is_empty()` checks observes the not-yet-
// landed entry, so round 1's verify-after-clear has nothing to restore, and
// the writer's later insert lands with the sentinel stuck `false`. Real,
// non-hypothetical for `insert_many`/`set_many`/`remove_many`, whose per-item
// loop yields at `cache.insert(...).await` every iteration while a single
// `store(true)` was hoisted once before the loop.
//
// Closed by republishing `dirty_nonempty.store(true, Release)` immediately
// after EACH per-item `dirty.insert()`, not just once before the loop (see
// `insert_many`'s loop body). This test drives the exact interleaving via a
// `BatchInsertPauseHook` parked between `insert_many`'s first and second
// iteration.
// ============================================================================
mod batch_insert_republish_535 {
    use super::*;
    use crate::membuffer_clear_race_hook::BatchInsertPauseHook;

    #[tokio::test]
    async fn insert_many_second_item_survives_drain_racing_between_iterations() {
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();

        let buffered = Arc::new(MemBufferStore::new(inner_store, no_flush_config()));
        // Pre-empt the background flusher BEFORE any `.await` — this test
        // spawns `insert_many` and genuinely suspends on a `Notify`, which
        // (unlike the round-1 test above) gives the runtime its first real
        // chance to schedule the separately-spawned flusher task. Without
        // this, the flusher can legitimately race the manually-driven
        // `drain_once_for_test` below and flush v1 to `inner` on its own,
        // making the test pass regardless of whether the round-2 fix is
        // present. See `disable_background_flusher_for_test`'s doc comment.
        buffered.disable_background_flusher_for_test();

        let hook = Arc::new(BatchInsertPauseHook::new());
        buffered.set_batch_pause_hook(Some(Arc::clone(&hook)));

        // Spawn insert_many([v0, v1]) — it will park after v0's iteration
        // (dirty.insert + round-2 republish + cache write all done for v0),
        // before v1's dirty.insert runs.
        let buffered_writer = Arc::clone(&buffered);
        let insert_task = tokio::spawn(async move {
            buffered_writer
                .insert_many(vec![
                    Bytes::from_static(b"v0-535"),
                    Bytes::from_static(b"v1-535"),
                ])
                .await
        });

        hook.wait_until_parked().await;

        // While the writer is parked (v1 NOT in dirty yet), run a real
        // drain_once: it flushes v0 (the only entry in dirty right now) to
        // inner, removes it via remove_if, observes dirty EMPTY (v1 isn't
        // there yet) on BOTH the initial check and round 1's verify-after-
        // clear re-check, and correctly clears the sentinel to `false` — this
        // is NOT a bug at this point, dirty genuinely is empty.
        let drained = buffered.drain_once_for_test(usize::MAX).await.unwrap();
        assert_eq!(drained, 1, "the racing drain should have flushed only v0");
        assert!(
            !buffered.dirty_nonempty_flag(),
            "sentinel correctly clear here — dirty is genuinely empty before v1 lands"
        );

        // Release the writer — it now inserts v1 into dirty. Pre-round-2-fix
        // this leaves the sentinel stuck `false` (masking v1); post-fix, the
        // per-item republish immediately after v1's dirty.insert restores it.
        //
        // The background flusher was disabled above (before its first poll),
        // so nothing else can touch `dirty`/`dirty_nonempty` concurrently
        // here — the assertion below is deterministic, not a race with any
        // other drainer.
        hook.release();
        let keys = insert_task
            .await
            .unwrap()
            .expect("insert_many must succeed");
        assert_eq!(keys.len(), 2);
        let v1_key = keys[1].clone();

        assert!(
            buffered.dirty_nonempty_flag(),
            "round-2 gap: v1 landed in dirty after the racing drain cleared the \
             sentinel, but nothing republished `true` for it"
        );

        // Force-evict v1 from the moka cache: `get()` checks the cache
        // FIRST and only falls through to the `dirty_nonempty`-gated `dirty`
        // probe on a cache miss. `insert_many` also writes v1 into cache, so
        // without this eviction `get()` would trivially cache-hit regardless
        // of whether the sentinel is correct — this line is what actually
        // exercises the vulnerable fast-path. With the flusher disabled and
        // no other drainer running, `inner` provably lacks v1 at this point,
        // so this deterministically forces the code through the
        // dirty-probe path the fix protects.
        buffered.evict_from_cache_for_test(&v1_key).await;

        // End-to-end: v1 must be visible immediately (not masked, not
        // stale-missed via the false sentinel skipping the dirty probe).
        // Pre-round-2-fix: v1 sits in `dirty`, the sentinel is wrongly
        // `false` (the racing drain cleared it before v1 landed), and no
        // legitimate flush has necessarily happened yet — `get()` fast-path
        // skips the dirty probe and falls through to `inner`, which also
        // lacks v1 → NotFound (the masked write).
        let got = buffered.get(v1_key).await.unwrap();
        assert_eq!(got.as_ref(), b"v1-535");

        // v0 reached inner via the racing drain.
        let v0_key = keys[0].clone();
        assert!(
            buffered.get(v0_key).await.is_ok(),
            "v0 should have been flushed to inner by the racing drain"
        );
    }
}
