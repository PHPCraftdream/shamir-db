//! F-28 Step 4 (#831) — cached per-repo reverse-FK map + O(1) per-table FK
//! role flags.
//!
//! `discover_restrict_refs` (`query::batch::fk_restrict`) and
//! `discover_action_refs` (`query::batch::fk_actions`) both used to do an
//! O(tables) scan — `repo.list_table_names()` then `table.collect_fk_refs()`
//! for every table in the repo — on EVERY delete/cascade-recursion-level.
//! This module caches that scan's result per repo, keyed by parent table
//! name, and invalidates the whole cache whenever the data
//! [`collect_fk_refs`](crate::table::TableManager::collect_fk_refs) reads
//! (a table's bound validators + the registry's compiled artifacts) can have
//! changed — see [`FkReverseCache::invalidate`].
//!
//! ## Design
//!
//! - **Storage**: `ArcSwap<VersionedState>` (RCU) — this is a read-heavy,
//!   rarely-invalidated snapshot, matching the workspace's `ArcSwap`
//!   convention for exactly this access pattern (see
//!   `TableManager::validator_bindings`). F-47 (#858) merged the generation
//!   counter and the cached indices into ONE `VersionedState` behind this
//!   single `ArcSwap`, so a generation-check-then-publish is a single atomic
//!   pointer-identity CAS, not two independent operations on two separate
//!   atomics.
//! - **Population**: cache-aside / lazy. A cache miss (the state's `cache` is
//!   `None`, i.e. never built or just invalidated) triggers an O(tables)
//!   scan via the caller-supplied discovery closure, and the result is
//!   stored for every subsequent hit until the next invalidation. F-36 made
//!   this genuinely "exactly one scan per miss" under concurrency via a
//!   single-flight `build_lock` (serializing concurrent misses so the
//!   second's post-lock re-check finds the first's result already warm) and
//!   a generation-gated publish; F-47 closed the residual publish-race that
//!   gate left open by replacing the two-atomic generation-then-store with
//!   one atomic `ArcSwap::compare_and_swap` — see
//!   [`FkReverseCache::get_or_build_by_parent`].
//! - **Invalidation**: whole-repo clear (a single `ArcSwap::rcu` publishing a
//!   bumped generation with a cleared cache), not a surgical single-entry
//!   update. DDL (schema mutation, table create/drop) is rare;
//!   deletes/cascades are comparatively frequent, so paying one full rebuild
//!   per DDL mutation (instead of one per delete) is the right tradeoff.
//! - **Two lookups, one build**: [`ReverseFkEntry`] rows are collected ONCE
//!   per rebuild (tagged with the parent table they target) and indexed two
//!   ways — by parent table name (what `fk_restrict`/`fk_actions` need
//!   today) and, derived from the exact same rows, by child table name (a
//!   "which parent tables does THIS table reference" reverse-reverse index,
//!   which F-28 Step 5's Serializable-upgrade decision will need for its
//!   `require_footprint_for` wiring). No second independent scan.

use std::sync::Arc;

use arc_swap::ArcSwap;
use shamir_query_types::admin::FkAction;
use tokio::sync::Mutex;

use crate::query::batch::TableResolver;
use crate::query::TableRef;

/// Test-only pause/resume handshake, installed via
/// [`TEST_POST_GENCHECK_PRE_PUBLISH_HOOK`]. Shape mirrors
/// `commit.rs`'s `PostValidatePrePublishHook` (F-46, #857): `reached` lets
/// the harness thread detect (via polling, no sleeps) that a build has
/// actually parked at the seam; `resume` is the release signal the harness
/// fires once a concurrent `invalidate()` has completed; `armed` makes the
/// pause one-shot (CAS true→false) so only the FIRST caller to reach the
/// seam actually parks — every later arrival (e.g. a retry after a CAS
/// loss) passes straight through.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct PostGenCheckPrePublishHook {
    pub(crate) reached: std::sync::atomic::AtomicUsize,
    pub(crate) resume: tokio::sync::Notify,
    pub(crate) armed: std::sync::atomic::AtomicBool,
}

/// Test-only pause seam: fires in [`FkReverseCache::get_or_build_by_parent`]
/// immediately AFTER the O(tables) scan (`build().await`) returns and BEFORE
/// [`FkReverseCache::try_publish`]'s CAS attempt.
///
/// F-47 (#858): this is the exact program point the F-36 residual-window doc
/// named — a concurrent `invalidate()` that completes strictly inside this
/// window used to still be over-published by an unconditional `store`. This
/// hook lets a test drive a concurrent `invalidate()` to completion at
/// EXACTLY this point (deterministically, no sleeps) to prove: (a) on the
/// pre-fix code, the stale scan result got published anyway; (b) on the
/// fixed code, the pointer-identity CAS observes the concurrent publish and
/// refuses to overwrite it.
///
/// `nextest` runs each test in its own process, so this global cannot leak
/// across test files. Uninstalled (the default for every test that doesn't
/// use it) this is a single `OnceLock::get()` read with no lock, no
/// allocation, no await point taken.
#[cfg(test)]
pub(crate) static TEST_POST_GENCHECK_PRE_PUBLISH_HOOK: std::sync::OnceLock<
    std::sync::Arc<PostGenCheckPrePublishHook>,
> = std::sync::OnceLock::new();

/// Parks on [`TEST_POST_GENCHECK_PRE_PUBLISH_HOOK`] if a test installed one;
/// a true no-op otherwise. See that static's doc for the exact program point
/// and rationale.
async fn fire_post_gencheck_pre_publish_test_hook() {
    #[cfg(test)]
    if let Some(hook) = TEST_POST_GENCHECK_PRE_PUBLISH_HOOK.get() {
        hook.reached
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let should_pause = hook
            .armed
            .compare_exchange(
                true,
                false,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok();
        if should_pause {
            hook.resume.notified().await;
        }
    }
}

/// A single reverse-FK reference: a child table + child field that
/// references some parent table with the given `on_delete`/`on_update`
/// actions.
///
/// Shared shape for both `fk_restrict.rs` (RESTRICT-only discovery) and
/// `fk_actions.rs` (CASCADE/SET NULL discovery) — the two discovery
/// functions filter this same cached list by `on_delete` instead of running
/// two independent O(tables) scans with two near-duplicate row shapes. The
/// `on_update` field is carried alongside (free — `FkAction` is a plain
/// `Copy` enum) so the cache's O(1) role-flag helpers can answer the
/// isolation-upgrade question for the implicit UPDATE path independently of
/// the DELETE path (F-35: previously a single `action` field stored only
/// `on_delete`, so an `on_delete = NoAction, on_update = Restrict` FK never
/// got the Serializable upgrade for its implicit UPDATE).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReverseFkEntry {
    /// The child table name (same repo as the parent).
    pub child_table: String,
    /// The child field path (dot-joined) that holds the FK value.
    pub child_field: String,
    /// The parent field the child references (`ref_field`).
    pub parent_ref_field: String,
    /// The `on_delete` referential action declared on this FK.
    pub on_delete: FkAction,
    /// The `on_update` referential action declared on this FK.
    pub on_update: FkAction,
}

/// One repo-wide reverse-FK reference, tagged with the PARENT table it
/// targets. This is the shape the rebuild closure returns (a flat list
/// covering every table in the repo in one O(tables) scan); the cache then
/// groups it into the two indices below.
#[derive(Debug, Clone)]
pub struct TaggedReverseFkEntry {
    /// The parent table this entry's FK references.
    pub parent_table: String,
    pub entry: ReverseFkEntry,
}

/// Parent table name → its reverse-FK entries (every child table that
/// references it, across all `on_delete` actions).
type ParentIndex = shamir_collections::TFxMap<String, Vec<ReverseFkEntry>>;

/// Child table name → set of distinct parent table names it references.
/// The O(1) "is table X an FK child" role flag F-28 Step 5 needs.
type ChildIndex = shamir_collections::TFxMap<String, shamir_collections::TFxSet<String>>;

/// One repo's cached reverse-FK state. Absent (`cache` holds `None`) means
/// "not built (yet), or invalidated — the next lookup must rebuild via the
/// O(tables) scan".
struct CacheState {
    by_parent: ParentIndex,
    by_child: ChildIndex,
}

/// F-47 (#858): `generation` and `cache` merged into ONE value behind ONE
/// `ArcSwap`, so a generation-check-then-publish is a single atomic
/// compare-and-swap on `Arc` pointer identity rather than two independent
/// operations on two separate atomics racing each other. See
/// [`FkReverseCache`] and [`FkReverseCache::get_or_build_by_parent`] for the
/// F-36 race this closes.
struct VersionedState {
    /// Monotonically increasing generation, bumped every time
    /// [`invalidate`](FkReverseCache::invalidate) publishes a new (cleared)
    /// state. Kept (rather than relying purely on pointer identity) so
    /// `get_or_build_by_parent`'s doc/log-friendly reasoning about "did a
    /// concurrent invalidate happen" stays readable; the actual correctness
    /// guarantee comes from `ArcSwap::compare_and_swap`'s pointer-identity
    /// check, not from comparing this number.
    generation: u64,
    cache: Option<CacheState>,
}

/// Per-repo cache of the reverse-FK map, lazily populated (cache-aside) and
/// invalidated wholesale on any schema mutation that can change a table's
/// `collect_fk_refs()` result (validator bindings or the registry's
/// compiled artifacts) or the repo's table membership.
///
/// Held on [`RepoInstance`](crate::repo::RepoInstance) as one instance per
/// repo, cloned (Arc-shared) across every handle to that repo — same
/// pattern as `per_table_mvcc` / `token_names`.
pub struct FkReverseCache {
    /// F-47 (#858): generation + cache merged into ONE `ArcSwap<VersionedState>`
    /// (see that type's doc) so [`invalidate`](Self::invalidate) and the
    /// build path's publish in
    /// [`get_or_build_by_parent`](Self::get_or_build_by_parent) are each a
    /// SINGLE atomic operation instead of two racing ones.
    state: ArcSwap<VersionedState>,
    /// Single-flight guard serializing the build/populate path of
    /// [`get_or_build_by_parent`](Self::get_or_build_by_parent). Sanctioned
    /// async-mutex exception (see `CLAUDE.md`): this is a low-frequency
    /// cache-miss path held across an `.await` (the O(tables) scan), not a
    /// hot path — `tokio::sync::Mutex` (not `std::sync::Mutex`) so the held
    /// guard doesn't block the executor thread while the scan awaits. Also
    /// collapses the concurrent-miss stampede: two simultaneous misses no
    /// longer each run their own independent full scan.
    build_lock: Mutex<()>,
}

impl Default for FkReverseCache {
    fn default() -> Self {
        Self::new()
    }
}

impl FkReverseCache {
    pub fn new() -> Self {
        Self {
            state: ArcSwap::from_pointee(VersionedState {
                generation: 0,
                cache: None,
            }),
            build_lock: Mutex::new(()),
        }
    }

    /// Invalidate the whole cache. Called from
    /// `ShamirDb::compile_table_schema` at its natural completion point
    /// (success OR F-27b's restore-on-failure already settled — either way
    /// the live registry state is final by the time this runs) and from
    /// table create/drop (`RepoInstance::add_table`/`remove_table`), since
    /// both change what `collect_fk_refs()` would discover across the repo.
    ///
    /// The next call to [`get_or_build_by_parent`](Self::get_or_build_by_parent)
    /// after this pays one more O(tables) scan and repopulates both indices.
    pub fn invalidate(&self) {
        // F-47 (#858): a single `ArcSwap::rcu` publishes the bumped
        // generation AND the cleared cache as ONE new `Arc<VersionedState>`
        // in one atomic operation — no separate "bump generation, then
        // clear state" two-step, so there is no window between them for a
        // concurrent builder to straddle. `rcu`'s retry loop (re-running the
        // closure against whatever is CURRENTLY stored if another writer won
        // first) also makes back-to-back concurrent `invalidate` calls
        // themselves race-free: each sees the other's generation bump rather
        // than clobbering it.
        self.state.rcu(|old| {
            Arc::new(VersionedState {
                generation: old.generation + 1,
                cache: None,
            })
        });
    }

    /// Cache-aside lookup: return the cached reverse-FK entries for
    /// `parent_table`, running `build` (the existing O(tables) discovery
    /// scan) once if the cache is currently empty (never built, or
    /// invalidated since the last build).
    ///
    /// `build` must return the FULL repo's reverse-FK entries — every
    /// parent → its children, each tagged with the parent table it targets
    /// (see [`TaggedReverseFkEntry`]) — not just `parent_table`'s. A single
    /// rebuild seeds every parent's entry at once, so a subsequent delete on
    /// a DIFFERENT parent table in the same repo also hits the cache instead
    /// of triggering its own rebuild.
    ///
    /// # F-36 — single-flight rebuild; F-47 (#858) — atomic versioned publish
    ///
    /// Fast path: a cheap `ArcSwap::load` outside any lock serves every
    /// warm-cache reader. On a miss, [`build_lock`](Self::new) serializes
    /// the build path so two concurrent misses don't each run their own
    /// independent O(tables) scan (the second's post-acquire re-check finds
    /// the first's already-warm result).
    ///
    /// Inside the lock, the publish is a SINGLE atomic operation: the exact
    /// `Arc<VersionedState>` in effect is captured BEFORE the scan, and after
    /// the scan the result is published via `ArcSwap::compare_and_swap`
    /// against that SAME captured `Arc` (pointer identity, not a number
    /// comparison). The CAS can only succeed if nothing — no concurrent
    /// [`invalidate`](Self::invalidate), no other builder — replaced the
    /// state since it was captured. F-36's original version of this method
    /// captured a plain `u64` generation and, after the scan, compared it
    /// against a FRESH `generation.load()` before a SEPARATE `state.store`
    /// — two independent operations on two independent atomics, leaving a
    /// window where a concurrent `invalidate`'s bump-then-clear could land
    /// between the compare and the store and get overwritten by a stale
    /// publish. F-47 closes that window by merging the generation and the
    /// cached indices into ONE `VersionedState` behind ONE `ArcSwap`, making
    /// the compare-then-publish a single atomic CAS instead of two racing
    /// operations.
    ///
    /// A CAS loss means some other writer (an `invalidate`, or in principle
    /// another concurrent builder) already published a newer state; the scan
    /// result is dropped and the whole build retries against that new state,
    /// still holding `build_lock` — this never returns a stale-or-empty
    /// answer, only once a genuinely uncontested publish (or an existing warm
    /// hit) has happened.
    ///
    /// `build` is `Fn` (not `FnOnce`) precisely so that a rare CAS-loss retry
    /// can call it again.
    pub async fn get_or_build_by_parent<F, Fut, E>(
        &self,
        parent_table: &str,
        build: F,
    ) -> Result<Vec<ReverseFkEntry>, E>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<TaggedReverseFkEntry>, E>>,
    {
        // Fast path: cheap ArcSwap load, no lock. A warm cache serves every
        // concurrent reader here.
        if let Some(hit) = self.lookup_by_parent(parent_table) {
            return Ok(hit);
        }

        // Miss: serialize the build path so concurrent misses don't each
        // independently re-scan. Held across the scan's `.await`, hence the
        // async-aware `tokio::sync::Mutex` (sanctioned low-frequency
        // exception — see `build_lock`'s doc).
        let _guard = self.build_lock.lock().await;

        // Double-checked locking: another builder may have populated the
        // cache while we waited for the lock. Re-check before paying for our
        // own scan (in the common no-raced-invalidate case this is exactly
        // the single-flight hit that collapses the stampede).
        if let Some(hit) = self.lookup_by_parent(parent_table) {
            return Ok(hit);
        }

        // Build loop: capture the CURRENT Arc<VersionedState> (not just its
        // generation number), run the scan, and publish via a single
        // pointer-identity CAS against that exact captured Arc. On a CAS
        // loss, drop the stale result and rebuild against the current
        // state, still holding `build_lock`.
        loop {
            let captured = self.state.load_full();

            let all_entries = build().await?;

            fire_post_gencheck_pre_publish_test_hook().await;

            if self.try_publish(&captured, all_entries) {
                return Ok(self.lookup_by_parent(parent_table).unwrap_or_default());
            }
            // CAS lost: some other writer (an `invalidate`, or in principle
            // another builder) already published a newer state since we
            // captured `captured`. Loop and rebuild against the new state.
        }
    }

    /// O(1) role-flag helper for F-28 Step 5: is `table` referenced by ANY
    /// FK with a non-`NoAction` `on_delete` action (i.e. is it a parent
    /// worth an isolation upgrade at implicit-DELETE-begin time)? Derived
    /// from the SAME cached rows `get_or_build_by_parent` serves — no second
    /// structure. Returns `false` on a cold/invalidated cache without
    /// triggering a rebuild (a pure peek); callers needing an authoritative
    /// answer call [`get_or_build_by_parent`](Self::get_or_build_by_parent)
    /// first so the cache is warm (see `query_runner.rs`'s
    /// `implicit_tx_isolation_for_fk_parent`, which does exactly that).
    pub fn is_fk_parent_with_delete_action(&self, table: &str) -> bool {
        let guard = self.state.load();
        match guard.cache.as_ref() {
            Some(cache) => cache
                .by_parent
                .get(table)
                .is_some_and(|entries| entries.iter().any(|e| e.on_delete != FkAction::NoAction)),
            None => false,
        }
    }

    /// O(1) role-flag helper for F-28 Step 5 / F-35: is `table` referenced
    /// by ANY FK with a non-`NoAction` `on_update` action (i.e. is it a
    /// parent worth an isolation upgrade at implicit-UPDATE-begin time)?
    ///
    /// Symmetric with [`is_fk_parent_with_delete_action`](Self::is_fk_parent_with_delete_action)
    /// but keyed on `on_update` — the two are consulted independently so a
    /// FK shaped `on_delete = NoAction, on_update = Restrict` upgrades the
    /// implicit UPDATE path (this helper) without upgrading the implicit
    /// DELETE path (the sibling helper). Same cold-cache pure-peek semantics
    /// — see `query_runner.rs`'s `implicit_tx_isolation_for_fk_parent`,
    /// which warms the cache before peeking.
    pub fn is_fk_parent_with_update_action(&self, table: &str) -> bool {
        let guard = self.state.load();
        match guard.cache.as_ref() {
            Some(cache) => cache
                .by_parent
                .get(table)
                .is_some_and(|entries| entries.iter().any(|e| e.on_update != FkAction::NoAction)),
            None => false,
        }
    }

    /// O(1) role-flag helper for F-28 Step 5: does `table` reference ANY
    /// other table via a FK (i.e. is it an FK child requiring
    /// `require_footprint_for` at insert/update-staging time)? Derived from
    /// the child→parents reverse-reverse index built alongside `by_parent`
    /// in the SAME rebuild. Same cold-cache semantics as
    /// [`is_fk_parent_with_delete_action`](Self::is_fk_parent_with_delete_action)
    /// — see `query_runner.rs`'s `require_footprint_if_fk_child`, which warms
    /// the cache before peeking.
    pub fn is_fk_child(&self, table: &str) -> bool {
        let guard = self.state.load();
        match guard.cache.as_ref() {
            Some(cache) => cache
                .by_child
                .get(table)
                .is_some_and(|parents| !parents.is_empty()),
            None => false,
        }
    }

    fn lookup_by_parent(&self, parent_table: &str) -> Option<Vec<ReverseFkEntry>> {
        let guard = self.state.load();
        guard.cache.as_ref().map(|cache| {
            cache
                .by_parent
                .get(parent_table)
                .cloned()
                .unwrap_or_default()
        })
    }

    /// Build both indices from a flat, parent-tagged list of ALL reverse-FK
    /// entries in the repo and attempt to publish them via a SINGLE
    /// pointer-identity `ArcSwap::compare_and_swap` against `captured` — the
    /// exact `Arc<VersionedState>` the caller observed before running the
    /// scan `all_entries` came from.
    ///
    /// Returns `true` if the CAS won (our result is now published — no
    /// concurrent `invalidate`/builder replaced `captured` since it was
    /// read), `false` if it lost (some other writer already published a
    /// newer state; the caller must drop this result and retry against the
    /// new one). This is the F-47 (#858) fix: generation-check-then-publish
    /// collapsed into one atomic CAS instead of a separate load + store.
    fn try_publish(
        &self,
        captured: &Arc<VersionedState>,
        all_entries: Vec<TaggedReverseFkEntry>,
    ) -> bool {
        let mut by_parent: ParentIndex = shamir_collections::TFxMap::default();
        let mut by_child: ChildIndex = shamir_collections::TFxMap::default();

        for tagged in all_entries {
            by_child
                .entry(tagged.entry.child_table.clone())
                .or_default()
                .insert(tagged.parent_table.clone());
            by_parent
                .entry(tagged.parent_table)
                .or_default()
                .push(tagged.entry);
        }

        let new_state = Arc::new(VersionedState {
            generation: captured.generation + 1,
            cache: Some(CacheState {
                by_parent,
                by_child,
            }),
        });

        let previous =
            arc_swap::Guard::into_inner(self.state.compare_and_swap(captured, new_state));
        Arc::ptr_eq(&previous, captured)
    }
}

/// Shared O(tables) rebuild scan: for every table in `repo_name`, resolve it
/// and collect every FK it declares (ANY `on_delete` action, including
/// `NoAction` — the cache is a repo-wide snapshot both `fk_restrict.rs`'s
/// RESTRICT-only discovery and `fk_actions.rs`'s CASCADE/SET-NULL discovery
/// filter from, so it must carry every action, not just the ones one caller
/// happens to care about).
///
/// This is the exact scan `discover_restrict_refs`/`discover_action_refs`
/// used to run on every call before F-28 Step 4; now it runs at most once per
/// [`FkReverseCache`] invalidation, via
/// [`FkReverseCache::get_or_build_by_parent`].
pub async fn build_reverse_fk_entries(
    resolver: &dyn TableResolver,
    repo_name: &str,
) -> shamir_storage::error::DbResult<Vec<TaggedReverseFkEntry>> {
    let repo = resolver.resolve_repo(repo_name).await?;
    let table_names = repo.list_table_names();
    let mut entries = Vec::new();

    for name in &table_names {
        let child_ref = TableRef::with_repo(repo_name, name);
        let child_table = match resolver.resolve(&child_ref).await {
            Ok(t) => t,
            Err(_) => continue,
        };

        for (field_path, fk) in child_table.collect_fk_refs() {
            entries.push(TaggedReverseFkEntry {
                parent_table: fk.ref_table.clone(),
                entry: ReverseFkEntry {
                    child_table: name.clone(),
                    child_field: field_path.join("."),
                    parent_ref_field: fk.ref_field.clone(),
                    on_delete: fk.on_delete,
                    on_update: fk.on_update,
                },
            });
        }
    }

    Ok(entries)
}
