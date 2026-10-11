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

/// B06 seam after a successful CAS, before returning its retained snapshot.
#[cfg(test)]
pub(crate) static TEST_POST_PUBLISH_PRE_RETURN_HOOK: std::sync::OnceLock<
    Arc<PostGenCheckPrePublishHook>,
> = std::sync::OnceLock::new();

async fn fire_post_publish_pre_return_test_hook() {
    #[cfg(test)]
    if let Some(hook) = TEST_POST_PUBLISH_PRE_RETURN_HOOK.get() {
        if hook
            .armed
            .compare_exchange(
                true,
                false,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            hook.reached
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // Only the armed caller waits; notify_one retains an early permit.
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
/// references it, across all `on_delete`/`on_update` actions).
///
/// (#1181) Value is `Arc<[ReverseFkEntry]>`, not `Vec<ReverseFkEntry>`: a
/// warm-cache hit in [`FkSnapshot::by_parent`] is then a pointer
/// (refcount) clone instead of a deep clone of every entry's owned `String`
/// fields — shared by all three discovery callers (`fk_restrict.rs`,
/// `fk_actions.rs`, `fk_on_update.rs`), each of which already re-filters this
/// same slice down to the handful of entries it cares about.
type ParentIndex = shamir_collections::TFxMap<String, Arc<[ReverseFkEntry]>>;

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

/// One populated, immutable reverse-FK graph. Invalidation affects future
/// acquisitions, never the entries or role decisions made from this snapshot.
#[derive(Clone)]
pub struct FkSnapshot {
    state: Arc<VersionedState>,
}

impl FkSnapshot {
    fn cache(&self) -> &CacheState {
        self.state
            .cache
            .as_ref()
            .expect("FkSnapshot always holds a populated state")
    }

    pub fn by_parent(&self, table: &str) -> Arc<[ReverseFkEntry]> {
        self.cache()
            .by_parent
            .get(table)
            .cloned()
            .unwrap_or_else(|| Arc::from(Vec::new()))
    }

    pub fn is_fk_child(&self, table: &str) -> bool {
        self.cache()
            .by_child
            .get(table)
            .is_some_and(|parents| !parents.is_empty())
    }

    pub fn is_parent_with_delete_action(&self, table: &str) -> bool {
        self.cache().by_parent.get(table).is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.on_delete != FkAction::NoAction)
        })
    }

    pub fn is_parent_with_update_action(&self, table: &str) -> bool {
        self.cache().by_parent.get(table).is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.on_update != FkAction::NoAction)
        })
    }
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

    /// Compatibility lookup for existing users; all data comes from one
    /// retained snapshot, including after a successful publish.
    pub async fn get_or_build_by_parent<F, Fut, E>(
        &self,
        parent_table: &str,
        build: F,
    ) -> Result<Arc<[ReverseFkEntry]>, E>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<TaggedReverseFkEntry>, E>>,
    {
        Ok(self
            .get_or_build_snapshot(build)
            .await?
            .by_parent(parent_table))
    }

    /// Acquire one populated repo-wide graph. Warm reads retain an owned Arc,
    /// not an arc-swap Guard. Cold reads remain single-flight; a CAS loss
    /// discards the scan and yields before retrying against a new generation.
    /// The build closure must return ALL parent-tagged entries in the repo;
    /// both parent and child indices are derived from that one complete scan.
    /// Errors come only from the build closure and never publish partial data.
    pub async fn get_or_build_snapshot<F, Fut, E>(&self, build: F) -> Result<FkSnapshot, E>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<TaggedReverseFkEntry>, E>>,
    {
        let state = self.state.load_full();
        if state.cache.is_some() {
            return Ok(FkSnapshot { state });
        }
        let _guard = self.build_lock.lock().await;
        let state = self.state.load_full();
        if state.cache.is_some() {
            return Ok(FkSnapshot { state });
        }
        loop {
            let captured = self.state.load_full();
            let all_entries = build().await?;
            fire_post_gencheck_pre_publish_test_hook().await;
            if let Some(state) = self.try_publish(&captured, all_entries) {
                fire_post_publish_pre_return_test_hook().await;
                return Ok(FkSnapshot { state });
            }
            tokio::task::yield_now().await;
        }
    }

    // Legacy assertions in out-of-scope fk_race_closure_tests require these
    // names. No production peek API: test helpers require populated state.
    #[cfg(test)]
    pub fn is_fk_parent_with_delete_action(&self, table: &str) -> bool {
        let state = self.state.load_full();
        assert!(
            state.cache.is_some(),
            "legacy role assertion requires a warm cache",
        );
        let snapshot = FkSnapshot { state };
        snapshot.is_parent_with_delete_action(table)
    }

    #[cfg(test)]
    pub fn is_fk_parent_with_update_action(&self, table: &str) -> bool {
        let state = self.state.load_full();
        assert!(
            state.cache.is_some(),
            "legacy role assertion requires a warm cache",
        );
        let snapshot = FkSnapshot { state };
        snapshot.is_parent_with_update_action(table)
    }

    /// Build both indices from a flat, parent-tagged list of ALL reverse-FK
    /// entries in the repo and attempt to publish them via a SINGLE
    /// pointer-identity `ArcSwap::compare_and_swap` against `captured` — the
    /// exact `Arc<VersionedState>` the caller observed before running the
    /// scan `all_entries` came from.
    ///
    /// Returns the published Arc if the CAS won (no concurrent writer
    /// replaced `captured` since it was read), or `None` if it lost.
    /// On loss, the caller must drop the scan and retry against the newer
    /// state. F-47's generation-check-then-publish remains one atomic CAS,
    /// not a separate load + store.
    fn try_publish(
        &self,
        captured: &Arc<VersionedState>,
        all_entries: Vec<TaggedReverseFkEntry>,
    ) -> Option<Arc<VersionedState>> {
        // Accumulated as `Vec` (push-friendly) and converted to the cache's
        // `Arc<[ReverseFkEntry]>` storage shape once, below — `ParentIndex`
        // itself can't `.push()` into an `Arc<[T]>` slice.
        let mut by_parent_vecs: shamir_collections::TFxMap<String, Vec<ReverseFkEntry>> =
            shamir_collections::TFxMap::default();
        let mut by_child: ChildIndex = shamir_collections::TFxMap::default();

        for tagged in all_entries {
            by_child
                .entry(tagged.entry.child_table.clone())
                .or_default()
                .insert(tagged.parent_table.clone());
            by_parent_vecs
                .entry(tagged.parent_table)
                .or_default()
                .push(tagged.entry);
        }

        let by_parent: ParentIndex = by_parent_vecs
            .into_iter()
            .map(|(parent, entries)| (parent, Arc::from(entries)))
            .collect();

        let new_state = Arc::new(VersionedState {
            generation: captured.generation + 1,
            cache: Some(CacheState {
                by_parent,
                by_child,
            }),
        });

        let previous = arc_swap::Guard::into_inner(
            self.state
                .compare_and_swap(captured, Arc::clone(&new_state)),
        );
        Arc::ptr_eq(&previous, captured).then_some(new_state)
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
        // F-55 (#881): a resolve failure on any single child table aborts
        // the WHOLE repo-wide scan instead of silently treating that table
        // as "declares no foreign keys". Swallowing the error here would
        // publish an incomplete FK graph via the caller's CAS, allowing a
        // subsequent parent UPDATE/DELETE to violate referential integrity
        // by never discovering the missed child as a dependent.
        let child_table = resolver.resolve(&child_ref).await?;

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
