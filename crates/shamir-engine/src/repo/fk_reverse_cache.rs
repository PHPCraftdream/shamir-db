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
//! - **Storage**: `ArcSwap<Option<CacheState>>` (RCU) — this is a
//!   read-heavy, rarely-invalidated snapshot, matching the workspace's
//!   `ArcSwap` convention for exactly this access pattern (see
//!   `TableManager::validator_bindings`).
//! - **Population**: cache-aside / lazy. A cache miss (the state is `None`,
//!   i.e. never built or just invalidated) triggers an O(tables) scan via
//!   the caller-supplied discovery closure, and the result is stored for
//!   every subsequent hit until the next invalidation. F-36 made this
//!   genuinely "exactly one scan per miss" under concurrency: a
//!   single-flight `build_lock` serializes concurrent misses (so the second
//!   one's post-lock re-check finds the first's result already warm), and a
//!   generation counter (bumped in `invalidate`) gates the publish — a scan
//!   that raced a concurrent DDL invalidation is dropped and rebuilt rather
//!   than publishing a stale snapshot. See
//!   [`FkReverseCache::get_or_build_by_parent`] for the (documented) residual
//!   window.
//! - **Invalidation**: whole-repo clear (`store(None)`), not a surgical
//!   single-entry update. DDL (schema mutation, table create/drop) is rare;
//!   deletes/cascades are comparatively frequent, so paying one full rebuild
//!   per DDL mutation (instead of one per delete) is the right tradeoff.
//! - **Two lookups, one build**: [`ReverseFkEntry`] rows are collected ONCE
//!   per rebuild (tagged with the parent table they target) and indexed two
//!   ways — by parent table name (what `fk_restrict`/`fk_actions` need
//!   today) and, derived from the exact same rows, by child table name (a
//!   "which parent tables does THIS table reference" reverse-reverse index,
//!   which F-28 Step 5's Serializable-upgrade decision will need for its
//!   `require_footprint_for` wiring). No second independent scan.

use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;
use shamir_query_types::admin::FkAction;
use tokio::sync::Mutex;

use crate::query::batch::TableResolver;
use crate::query::TableRef;

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

/// One repo's cached reverse-FK state. Absent (`state` holds `None`) means
/// "not built (yet), or invalidated — the next lookup must rebuild via the
/// O(tables) scan".
struct CacheState {
    by_parent: ParentIndex,
    by_child: ChildIndex,
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
    state: ArcSwap<Option<CacheState>>,
    /// Monotonically increasing generation counter, bumped in
    /// [`invalidate`](Self::invalidate) alongside the `state` clear. The
    /// build path in [`get_or_build_by_parent`](Self::get_or_build_by_parent)
    /// captures this BEFORE its O(tables) scan and refuses to publish the
    /// scan's result if a concurrent `invalidate` bumped it in the meantime
    /// — closing the F-36 cache-aside invalidate-vs-build race where a scan
    /// that started before a DDL could publish a stale snapshot AFTER it.
    generation: AtomicU64,
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
            state: ArcSwap::from_pointee(None),
            generation: AtomicU64::new(0),
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
        // Bump the generation FIRST, then clear the snapshot. A concurrent
        // build (holding `build_lock`, mid-scan) captured the generation
        // before its scan began and re-reads it after; this bump makes that
        // re-read observe the change so the build refuses to publish its
        // (now-stale, pre-DDL) result and rebuilds against the current
        // (post-DDL) state instead. See `get_or_build_by_parent` for the
        // residual window this leaves (a single atomic CAS across BOTH
        // `generation` and `state` isn't expressible here).
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.state.store(std::sync::Arc::new(None));
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
    /// # F-36 — generation-safe, single-flight rebuild
    ///
    /// Fast path: a cheap `ArcSwap::load` outside any lock serves every
    /// warm-cache reader. On a miss, [`build_lock`](Self::new) serializes
    /// the build path so two concurrent misses don't each run their own
    /// independent O(tables) scan (the second's post-acquire re-check finds
    /// the first's already-warm result). Inside the lock, the generation
    /// captured BEFORE the scan is re-checked AFTER it, and the result is
    /// published ONLY if no concurrent [`invalidate`](Self::invalidate)
    /// bumped the generation during the scan; on a mismatch the scan ran
    /// against stale (pre-DDL) schema, so it's dropped and rebuilt against
    /// the current generation — still under the same lock, never returning a
    /// stale-or-empty answer for this call.
    ///
    /// `build` is `Fn` (not `FnOnce`) precisely so that rare
    /// generation-mismatch retry can call it again.
    ///
    /// ## Residual window (documented, not closed)
    ///
    /// The post-scan generation compare and `populate`'s `ArcSwap::store`
    /// are two separate operations on two separate atomics, so a single
    /// atomic CAS across both isn't expressible. A concurrent `invalidate`
    /// whose `fetch_add` lands AFTER the post-scan generation load but whose
    /// `state.store(None)` lands BEFORE `populate`'s `store(Some)` can still
    /// be over-published by our subsequent `store`. This window contains no
    /// `.await` and no allocations beyond `populate`'s own index build, so
    /// it is far narrower than the pre-F-36 unconditional publish; it is
    /// bounded by the NEXT `invalidate` (the very next DDL clears it), and
    /// the single-flight `build_lock` guarantees the NEXT miss re-checks the
    /// generation under the lock and rebuilds fresh.
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

        // Build loop: capture the generation, run the scan, and publish ONLY
        // if no `invalidate` bumped the generation during the scan. On a
        // mismatch, drop the stale result and rebuild against the current
        // generation, still holding `build_lock`. We never return a
        // stale-or-empty answer here — only return once a
        // generation-matched populate has actually happened.
        loop {
            let gen_at_start = self.generation.load(Ordering::Acquire);

            let all_entries = build().await?;

            // Re-read the generation as close to the publish as possible. If
            // it's unchanged, no `invalidate` raced the scan — publish. If it
            // moved, a concurrent DDL invalidated mid-scan; loop and rebuild
            // against the new generation (see the doc's residual-window
            // note for the compare-vs-store ordering this can't fully close).
            if self.generation.load(Ordering::Acquire) == gen_at_start {
                self.populate(all_entries);
                return Ok(self.lookup_by_parent(parent_table).unwrap_or_default());
            }
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
        match guard.as_ref() {
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
        match guard.as_ref() {
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
        match guard.as_ref() {
            Some(cache) => cache
                .by_child
                .get(table)
                .is_some_and(|parents| !parents.is_empty()),
            None => false,
        }
    }

    fn lookup_by_parent(&self, parent_table: &str) -> Option<Vec<ReverseFkEntry>> {
        let guard = self.state.load();
        guard.as_ref().as_ref().map(|cache| {
            cache
                .by_parent
                .get(parent_table)
                .cloned()
                .unwrap_or_default()
        })
    }

    /// Build both indices from a flat, parent-tagged list of ALL reverse-FK
    /// entries in the repo and publish them atomically via one
    /// `ArcSwap::store`.
    fn populate(&self, all_entries: Vec<TaggedReverseFkEntry>) {
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

        self.state.store(std::sync::Arc::new(Some(CacheState {
            by_parent,
            by_child,
        })));
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
