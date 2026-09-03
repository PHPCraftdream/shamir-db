//! F-36 (#844, P0) — `FkReverseCache` invalidate-vs-build race closure tests.
//!
//! ## Why a separate file (not `fk_race_closure_tests.rs`)
//!
//! `fk_race_closure_tests.rs` is scoped to the END-TO-END SSI race closure:
//! every test there drives the real `execute_batch` entry point and injects a
//! concurrent writer through `RaceInjectingResolver`'s `resolve_repo`-call-
//! ordinal hook (`INJECT_AT_RESOLVE_REPO_CALL`), whose entire mechanism (the
//! ordinal analysis in the module doc, `InjectedWriter`, `TxTestResolver`) is
//! coupled to the `execute_batch` Delete/Update call sequence. That layer is
//! the wrong place to test the cache's OWN invalidate-vs-build race: F-36's
//! race is inside `FkReverseCache::get_or_build_by_parent` itself (the
//! generation-gated publish), which sits BELOW `execute_batch`.
//!
//! These tests exercise `FkReverseCache::get_or_build_by_parent` DIRECTLY with
//! a caller-supplied build closure, so the natural — and most precise —
//! injection point is INSIDE that build closure (a `tokio::sync::Notify` the
//! scan awaits, while the test harness fires `invalidate()` against the shared
//! cache). This reuses the SHAPE of `RaceInjectingResolver`'s controllable-gate
//! injection (a deterministic, no-sleeps handshake that fires a caller action
//! at an exact program point — here "build is parked mid-scan" instead of "the
//! Nth `resolve_repo` call"), applied at the layer where the F-36 race lives.
//!
//! ## Coverage
//!
//! 1. **`build_paused_then_invalidate_never_publishes_stale_snapshot`** — the
//!    exact race from the brief: pause a build mid-scan, fire `invalidate()`
//!    while it's parked, release it, and assert the now-stale scan result is
//!    NEVER published and the call returns fresh post-invalidate data; the NEXT
//!    call serves that fresh snapshot from the warm cache (no rebuild).
//! 2. **`two_concurrent_misses_run_at_most_one_real_scan`** — single-flight
//!    `build_lock` collapses the stampede: two concurrent cold-cache misses
//!    run exactly ONE real scan (asserted via a call counter in the injected
//!    build closure).
//! 3. **`invalidate_then_single_get_returns_fresh_data`** — plain regression
//!    that the generation logic doesn't leave the cache stuck cold or stale:
//!    `invalidate()` followed by a single `get_or_build_by_parent` returns the
//!    fresh post-invalidate data, not the pre-invalidate snapshot.
//! 4. **`concurrent_invalidate_strictly_between_gencheck_and_publish_never_over_publishes`**
//!    (F-47, #858) — the F-36 RESIDUAL window itself, restated by the
//!    2026-07-28 readonly review's §3 P0-2: test 1 above pauses the build
//!    mid-SCAN (before the generation is even re-checked), which the OLD
//!    generation-then-store code already handled correctly. This test uses
//!    `fk_reverse_cache.rs`'s `TEST_POST_GENCHECK_PRE_PUBLISH_HOOK` to pause
//!    AFTER the scan/gen-check has already passed and BEFORE the publish
//!    itself — the exact gap the module's own doc named "documented, not
//!    closed" — and fires a concurrent `invalidate()` to completion strictly
//!    inside that gap. On the pre-F-47 code (`generation.load() ==
//!    gen_at_start` then a separate unconditional `populate`/`store`), the
//!    concurrent `invalidate()` change is invisible to the publish and the
//!    stale scan result overwrites the fresh post-invalidate `None` — the
//!    call returns STALE data and the cache is stuck serving it. On the
//!    fixed (F-47) code, the publish is a single pointer-identity CAS against
//!    the captured `Arc`; since `invalidate()` already replaced that `Arc`
//!    by the time the CAS runs, the CAS loses, the stale result is dropped,
//!    and the build retries against the fresh state.
//! 5. **`concurrent_invalidate_storm_converges_after_bounded_retries`** (#1200)
//!    -- the CAS-loss retry loop's `tokio::task::yield_now().await` (added so
//!    a continuous `invalidate()` storm cooperates with the executor between
//!    attempts instead of hot-looping) doesn't change the loop's termination
//!    or correctness: forces FIVE consecutive CAS losses (via the same
//!    post-scan/pre-publish seam test 4 uses, re-armed before each release)
//!    and asserts the call still converges -- terminates within a timeout,
//!    publishes exactly the LAST attempt's data, pays exactly one build
//!    invocation per forced loss plus the final winner, and leaves the cache
//!    genuinely warm afterward. A bounded, deterministic proof the retry path
//!    is exercised and correct -- not a starvation reproduction, which would
//!    be flaky by nature.

use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_query_types::admin::FkAction;
use tokio::sync::Notify;

use crate::repo::fk_reverse_cache::{
    PostGenCheckPrePublishHook, TEST_POST_GENCHECK_PRE_PUBLISH_HOOK,
};
use crate::repo::{FkReverseCache, ReverseFkEntry, TaggedReverseFkEntry};

/// Build a `ReverseFkEntry` whose `child_table` is `child` (everything else
/// identical), so tests can distinguish "old schema" vs "new schema" scan
/// results by a single field. `on_delete`/`on_update` are `NoAction` — the
/// race under test is cache publish correctness, not action filtering.
fn entry(child: &str) -> ReverseFkEntry {
    ReverseFkEntry {
        child_table: child.to_string(),
        child_field: "parent_id".to_string(),
        parent_ref_field: "id".to_string(),
        on_delete: FkAction::NoAction,
        on_update: FkAction::NoAction,
    }
}

/// Pre-DDL ("old schema") scan result.
fn old_entry() -> ReverseFkEntry {
    entry("child_old")
}

/// Post-DDL ("new schema") scan result.
fn new_entry() -> ReverseFkEntry {
    entry("child_new")
}

/// Scan result tagged with a schema version, so a single mutating counter can
/// flip what the "current schema" returns across an invalidate boundary.
fn versioned_entry(v: usize) -> ReverseFkEntry {
    entry(&format!("child_v{v}"))
}

/// One parent-tagged entry wrapping `e`, for the flat list the build closure
/// returns.
fn tagged(e: ReverseFkEntry) -> TaggedReverseFkEntry {
    TaggedReverseFkEntry {
        parent_table: "parent".to_string(),
        entry: e,
    }
}

// ============================================================================
// 1. The exact race from the brief: build paused → invalidate → resume → the
//    stale scan result is NEVER published; the call returns fresh
//    post-invalidate data, and the NEXT call serves it from the warm cache.
//
// Determinism: the build closure (invocation 1) parks on a `Notify` after
// bumping a call counter. The harness busy-waits on that counter (so it knows
// `get_or_build_by_parent` already captured the generation BEFORE calling
// build), fires `invalidate()` (bumps the generation), THEN releases the
// paused build. `invalidate()` strictly happens-before the build's return
// (gated by the release) and hence before the post-build generation load, so
// the generation check deterministically observes the bump and refuses to
// publish the stale invocation-1 result, looping to a fresh invocation-2
// rebuild instead. No sleeps, no timing dependence.
// ============================================================================

#[tokio::test]
async fn build_paused_then_invalidate_never_publishes_stale_snapshot() {
    let cache = Arc::new(FkReverseCache::new());
    let build_calls = Arc::new(AtomicUsize::new(0));
    let resume = Arc::new(Notify::new());

    // Build closure: invocation 1 = "old schema" scan (parks on `resume` so the
    // test can fire invalidate() mid-build); invocation 2+ = "new schema"
    // (post-DDL) scan. `Fn` (re-callable) so the generation-mismatch retry can
    // invoke it a second time under the same single-flight lock.
    let build = {
        let build_calls = Arc::clone(&build_calls);
        let resume = Arc::clone(&resume);
        move || {
            let build_calls = Arc::clone(&build_calls);
            let resume = Arc::clone(&resume);
            async move {
                let n = build_calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // Park mid-scan holding build_lock — the test fires
                    // invalidate() while we wait here.
                    resume.notified().await;
                    Ok::<_, Infallible>(vec![tagged(old_entry())])
                } else {
                    Ok::<_, Infallible>(vec![tagged(new_entry())])
                }
            }
        }
    };

    // Drive get_or_build_by_parent on a spawned task so the harness can call
    // invalidate() on the shared cache while the build is parked.
    let cache_for_task = Arc::clone(&cache);
    let handle = tokio::spawn(async move {
        cache_for_task
            .get_or_build_by_parent("parent", build)
            .await
            .unwrap()
    });

    // Wait until the build has STARTED. By construction, build() is only called
    // AFTER get_or_build_by_parent captured gen_at_start, so once this counter
    // is >= 1 the invalidate below is strictly after that capture.
    while build_calls.load(Ordering::SeqCst) < 1 {
        tokio::task::yield_now().await;
    }

    // Fire invalidate() while the build is parked: bumps the generation and
    // clears the snapshot. The paused build will still return OLD data, but the
    // post-build generation check must reject it.
    cache.invalidate();

    // Release the paused build; it returns OLD data → generation check rejects
    // → loop → invocation 2 rebuilds against the NEW generation → publishes
    // NEW data.
    resume.notify_one();

    let published = handle.await.unwrap();
    assert_eq!(
        published.to_vec(),
        vec![new_entry()],
        "a build whose scan raced a concurrent invalidate must NOT publish its \
         stale snapshot — get_or_build_by_parent must drop it and rebuild fresh \
         post-invalidate data under the same single-flight lock"
    );

    // The race cost exactly two build invocations: the rejected stale one plus
    // the fresh retry.
    assert_eq!(
        build_calls.load(Ordering::SeqCst),
        2,
        "stale build rejected + one fresh retry"
    );

    // The NEXT call must serve the fresh snapshot from the WARM cache (no
    // rebuild) — proves the cache isn't stuck cold and the fresh data is
    // actually cached, not just returned once.
    let calls_before_next = build_calls.load(Ordering::SeqCst);
    let next = cache
        .get_or_build_by_parent("parent", {
            let build_calls = Arc::clone(&build_calls);
            move || {
                let build_calls = Arc::clone(&build_calls);
                async move {
                    build_calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, Infallible>(vec![tagged(old_entry())])
                }
            }
        })
        .await
        .unwrap();
    assert_eq!(
        next, published,
        "the next call must serve the same fresh post-invalidate snapshot"
    );
    assert_eq!(
        build_calls.load(Ordering::SeqCst),
        calls_before_next,
        "the next call must hit the warm cache (no rebuild) — the fresh \
         post-invalidate snapshot is genuinely cached"
    );
}

// ============================================================================
// 2. Single-flight: two concurrent misses on a cold cache run at most ONE real
//    scan. The build closure counts its own invocations and yields once
//    mid-scan so the second miss genuinely contends on build_lock (exercising
//    the double-checked-locking re-check path), then both callers must observe
//    the single built result. Asserted honestly: exactly ONE scan, not "at most
//    two" — the single-flight lock guarantees the second miss's post-lock
//    re-check finds the first's already-warm result.
// ============================================================================

#[tokio::test]
async fn two_concurrent_misses_run_at_most_one_real_scan() {
    let cache = Arc::new(FkReverseCache::new());
    let build_calls = Arc::new(AtomicUsize::new(0));

    let make_build = |build_calls: &Arc<AtomicUsize>| {
        let build_calls = Arc::clone(build_calls);
        move || {
            let build_calls = Arc::clone(&build_calls);
            async move {
                build_calls.fetch_add(1, Ordering::SeqCst);
                // Yield mid-scan so the other concurrent miss genuinely waits
                // on build_lock (rather than the two tasks running strictly
                // back-to-back); the single-flight guarantee must hold either
                // way.
                tokio::task::yield_now().await;
                Ok::<_, Infallible>(vec![tagged(new_entry())])
            }
        }
    };

    let cache_a = Arc::clone(&cache);
    let cache_b = Arc::clone(&cache);
    let calls_a = Arc::clone(&build_calls);
    let calls_b = Arc::clone(&build_calls);
    let h1 = tokio::spawn(async move {
        cache_a
            .get_or_build_by_parent("parent", make_build(&calls_a))
            .await
            .unwrap()
    });
    let h2 = tokio::spawn(async move {
        cache_b
            .get_or_build_by_parent("parent", make_build(&calls_b))
            .await
            .unwrap()
    });

    let (r1, r2) = tokio::join!(h1, h2);
    let r1 = r1.unwrap();
    let r2 = r2.unwrap();

    assert_eq!(
        build_calls.load(Ordering::SeqCst),
        1,
        "two concurrent cold-cache misses must run exactly ONE real scan \
         (single-flight build_lock); the second miss's post-lock re-check finds \
         the first's already-warm result"
    );
    assert_eq!(r1.to_vec(), vec![new_entry()]);
    assert_eq!(r2.to_vec(), vec![new_entry()]);
}

// ============================================================================
// 3. Plain non-race regression: invalidate() followed by a single
//    get_or_build_by_parent returns fresh post-invalidate data, not the stale
//    pre-invalidate snapshot. Proves the generation-check loop doesn't
//    accidentally leave the cache permanently cold or permanently stale.
// ============================================================================

#[tokio::test]
async fn invalidate_then_single_get_returns_fresh_data() {
    let cache = FkReverseCache::new();
    let schema_version = Arc::new(AtomicUsize::new(1));

    // First call: populates the cache with v1 data.
    let first = {
        let schema_version = Arc::clone(&schema_version);
        cache
            .get_or_build_by_parent("parent", {
                let schema_version = Arc::clone(&schema_version);
                move || {
                    let schema_version = Arc::clone(&schema_version);
                    async move {
                        let v = schema_version.load(Ordering::SeqCst);
                        Ok::<_, Infallible>(vec![tagged(versioned_entry(v))])
                    }
                }
            })
            .await
            .unwrap()
    };
    assert_eq!(first.to_vec(), vec![versioned_entry(1)]);

    // Simulate a DDL change: flip the "schema" the scan reads, then invalidate.
    schema_version.store(2, Ordering::SeqCst);
    cache.invalidate();

    // Second call must return FRESH (v2) data, not the stale v1 snapshot.
    let second = {
        let schema_version = Arc::clone(&schema_version);
        cache
            .get_or_build_by_parent("parent", {
                let schema_version = Arc::clone(&schema_version);
                move || {
                    let schema_version = Arc::clone(&schema_version);
                    async move {
                        let v = schema_version.load(Ordering::SeqCst);
                        Ok::<_, Infallible>(vec![tagged(versioned_entry(v))])
                    }
                }
            })
            .await
            .unwrap()
    };
    assert_eq!(
        second.to_vec(),
        vec![versioned_entry(2)],
        "invalidate() followed by get_or_build_by_parent must return fresh \
         post-invalidate data, not the stale pre-invalidate snapshot"
    );
    assert_ne!(second, first);
}

// ============================================================================
// 4. F-47 (#858) — the F-36 RESIDUAL window itself: a concurrent invalidate()
//    that completes STRICTLY BETWEEN the post-scan generation re-check and the
//    publish must never be over-published. This is the one interleaving test 1
//    (above) does NOT cover: that test's `resume` gate pauses the build INSIDE
//    the scan, before `get_or_build_by_parent` has even reached its
//    generation-compare — a window the OLD (F-36) code already closed
//    correctly. This test instead pauses AFTER the scan returns and AFTER the
//    (pre-F-47) generation compare would have already passed, using
//    `fk_reverse_cache.rs`'s `TEST_POST_GENCHECK_PRE_PUBLISH_HOOK` — the exact
//    program point named in the module's own "Residual window" doc.
//
// Determinism: the build closure returns immediately (no pause of its own).
// The seam INSIDE `get_or_build_by_parent` (fired right after `build().await`
// returns, right before the publish attempt) is what parks — installed via
// the hook below. The harness busy-polls `reached` (no sleeps) until the
// build has arrived at that seam, fires a concurrent `invalidate()` (which
// itself completes synchronously — `ArcSwap::rcu` returns once its own CAS
// has won), THEN releases the parked build. `invalidate()` strictly
// happens-before the release, and the release strictly happens-before the
// paused build resumes toward its publish attempt — so the interleaving is
// exactly "scan done, gen-check-equivalent already passed, THEN a concurrent
// invalidate completes, THEN the publish attempt runs" — the precise gap the
// module's own doc names.
// ============================================================================

#[tokio::test]
async fn concurrent_invalidate_strictly_between_gencheck_and_publish_never_over_publishes() {
    let cache = Arc::new(FkReverseCache::new());
    let build_calls = Arc::new(AtomicUsize::new(0));

    // Install the pause seam BEFORE the first get_or_build_by_parent call.
    // Every arrival at the seam bumps `reached` and (one-shot, via `armed`)
    // the FIRST arrival parks; the retry's second arrival (after a CAS loss)
    // passes straight through.
    let hook = Arc::new(PostGenCheckPrePublishHook {
        reached: AtomicUsize::new(0),
        resume: Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    TEST_POST_GENCHECK_PRE_PUBLISH_HOOK
        .set(Arc::clone(&hook))
        .expect("hook must not already be installed in this test process");

    // Build closure: invocation 1 returns "old schema" (pre-invalidate) data;
    // invocation 2+ returns "new schema" (post-invalidate) data — mirrors
    // test 1's `versioned_entry`/counter pattern so a retry's publish is
    // distinguishable from the rejected first attempt's. Neither invocation
    // pauses on its own — the pause under test lives INSIDE
    // get_or_build_by_parent (the hook above), strictly AFTER this closure
    // returns and BEFORE the publish attempt.
    let build = {
        let build_calls = Arc::clone(&build_calls);
        move || {
            let build_calls = Arc::clone(&build_calls);
            async move {
                let n = build_calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Ok::<_, Infallible>(vec![tagged(old_entry())])
                } else {
                    Ok::<_, Infallible>(vec![tagged(new_entry())])
                }
            }
        }
    };

    let cache_for_task = Arc::clone(&cache);
    let handle = tokio::spawn(async move {
        cache_for_task
            .get_or_build_by_parent("parent", build)
            .await
            .unwrap()
    });

    // Wait until the build has reached the post-scan/pre-publish seam and
    // actually parked there (invocation 1, "old schema", has already
    // returned).
    while hook.reached.load(Ordering::SeqCst) < 1 {
        tokio::task::yield_now().await;
    }

    // Fire a concurrent invalidate() strictly WHILE the build is parked at
    // the post-scan/pre-publish seam — i.e. AFTER any pre-F-47
    // generation-compare would already have passed, but BEFORE the publish
    // itself. `invalidate()` runs to full completion (its own `rcu` resolves
    // synchronously) before we release the paused build below.
    cache.invalidate();

    // Release the paused build. It will now attempt to publish its STALE
    // (invocation-1, pre-invalidate) scan result.
    hook.resume.notify_one();

    let published = handle.await.unwrap();

    // The stale (invocation-1) publish attempt must have been rejected
    // (F-47: pointer-identity CAS against its captured pre-scan Arc, which
    // invalidate() already replaced) and the build must have retried
    // (invocation 2, "new schema") and published THAT instead. On the
    // pre-F-47 code (two-atomic generation-then-store, no CAS), the
    // generation compare had ALREADY passed before this seam even fires, so
    // the stale invocation-1 result gets published unconditionally here —
    // this assertion is what catches that.
    assert_eq!(
        published.to_vec(),
        vec![new_entry()],
        "a build whose publish raced a concurrent invalidate() completing \
         strictly between its post-scan check and its own publish must NEVER \
         over-publish the stale pre-invalidate snapshot — must drop it and \
         retry against the fresh post-invalidate state instead"
    );
    assert_eq!(
        build_calls.load(Ordering::SeqCst),
        2,
        "stale build rejected by the CAS + exactly one fresh retry"
    );

    // The NEXT call must serve the fresh snapshot from the WARM cache (no
    // further rebuild) — confirms the cache isn't stuck cold or re-scanning.
    let calls_before_next = build_calls.load(Ordering::SeqCst);
    let next = cache
        .get_or_build_by_parent("parent", {
            let build_calls = Arc::clone(&build_calls);
            move || {
                let build_calls = Arc::clone(&build_calls);
                async move {
                    build_calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, Infallible>(vec![tagged(old_entry())])
                }
            }
        })
        .await
        .unwrap();
    assert_eq!(
        next, published,
        "the next call must serve the same fresh post-invalidate snapshot"
    );
    assert_eq!(
        build_calls.load(Ordering::SeqCst),
        calls_before_next,
        "the next call must hit the warm cache (no rebuild)"
    );
}

// ============================================================================
// 5. #1200 -- the CAS-loss retry loop's `tokio::task::yield_now().await`
//    (inserted between a lost CAS and the next retry attempt so a continuous
//    `invalidate()` storm cooperates with the executor instead of hot-looping)
//    must not change the loop's termination/correctness. This forces FIVE
//    consecutive CAS losses via the same post-scan/pre-publish seam test 4
//    uses, re-arming it before each release so the NEXT retry also parks at
//    the same seam, then lets the sixth attempt through uncontested. A
//    bounded, deterministic exercise of the retry path -- not a starvation
//    reproduction (which would be flaky by nature).
// ============================================================================

#[tokio::test]
async fn concurrent_invalidate_storm_converges_after_bounded_retries() {
    const FORCED_LOSSES: usize = 5;

    let cache = Arc::new(FkReverseCache::new());
    let build_calls = Arc::new(AtomicUsize::new(0));

    let hook = Arc::new(PostGenCheckPrePublishHook {
        reached: AtomicUsize::new(0),
        resume: Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    TEST_POST_GENCHECK_PRE_PUBLISH_HOOK
        .set(Arc::clone(&hook))
        .expect("hook must not already be installed in this test process");

    // Build closure: invocation `n` returns `versioned_entry(n)` -- a
    // distinct value per attempt, so the final published result unambiguously
    // identifies which invocation actually won the publish.
    let build = {
        let build_calls = Arc::clone(&build_calls);
        move || {
            let build_calls = Arc::clone(&build_calls);
            async move {
                let n = build_calls.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(vec![tagged(versioned_entry(n))])
            }
        }
    };

    let cache_for_task = Arc::clone(&cache);
    let handle = tokio::spawn(async move {
        cache_for_task
            .get_or_build_by_parent("parent", build)
            .await
            .unwrap()
    });

    // Force FORCED_LOSSES CAS losses: for each of the first FORCED_LOSSES
    // arrivals at the post-scan/pre-publish seam, fire invalidate() (making
    // that attempt's captured Arc stale) and re-arm the hook so the retry
    // this release triggers also parks at the same seam.
    for round in 1..=FORCED_LOSSES {
        while hook.reached.load(Ordering::SeqCst) < round {
            tokio::task::yield_now().await;
        }
        cache.invalidate();
        hook.armed.store(true, Ordering::SeqCst);
        hook.resume.notify_one();
    }

    // The (FORCED_LOSSES + 1)-th arrival: release it WITHOUT a further
    // invalidate() or re-arm, so its CAS wins against an uncontested state
    // and the loop terminates.
    while hook.reached.load(Ordering::SeqCst) < FORCED_LOSSES + 1 {
        tokio::task::yield_now().await;
    }
    hook.resume.notify_one();

    let published = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect(
            "get_or_build_by_parent must terminate/converge after a bounded \
             number of forced CAS losses, not spin forever",
        )
        .unwrap();

    // Exactly one build invocation per forced retry, plus the final winner.
    assert_eq!(
        build_calls.load(Ordering::SeqCst),
        FORCED_LOSSES + 1,
        "expected exactly one build invocation per forced CAS loss plus the \
         final winning attempt"
    );

    // The published result must be the LAST invocation's data -- the one
    // that won its CAS once every prior attempt had been forced stale.
    assert_eq!(published.to_vec(), vec![versioned_entry(FORCED_LOSSES)]);

    // The cache is now genuinely warm: a further call hits it without
    // triggering another build.
    let calls_before_next = build_calls.load(Ordering::SeqCst);
    let next = cache
        .get_or_build_by_parent("parent", {
            let build_calls = Arc::clone(&build_calls);
            move || {
                let build_calls = Arc::clone(&build_calls);
                async move {
                    build_calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, Infallible>(vec![tagged(old_entry())])
                }
            }
        })
        .await
        .unwrap();
    assert_eq!(
        next, published,
        "the next call must serve the same converged snapshot"
    );
    assert_eq!(
        build_calls.load(Ordering::SeqCst),
        calls_before_next,
        "the next call must hit the warm cache (no rebuild)"
    );
}
