//! Type-erased Repo / RepoFactory enums.
//!
//! Each backend variant is `#[cfg]`-gated behind the matching feature
//! flag passed through from `shamir-storage`. With the default
//! feature set every backend is on; embedded builds can disable
//! whichever ones they don't need (`default-features = false,
//! features = ["redb"]`).

use shamir_storage::error::DbResult;
use shamir_storage::storage_cached::{CachedStore, WriteMode};
#[cfg(feature = "fjall")]
use shamir_storage::storage_fjall::FjallRepo;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
#[cfg(feature = "fjall")]
use shamir_storage::storage_mirrored::{is_durable_table_config, MirroredStore};

#[cfg(feature = "fjall")]
use shamir_collections::TFxSet;
use shamir_storage::types::{Repo, Store};
#[cfg(feature = "fjall")]
use shamir_types::types::common::THasher;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(feature = "fjall")]
use tokio::sync::OnceCell;
use tokio::task;

#[derive(Clone)]
pub enum BoxRepo {
    InMemory(Arc<InMemoryRepo>),
    #[cfg(feature = "fjall")]
    Fjall(Arc<FjallRepo>),
    /// Bounded LRU + write-back wrapper. See `MemBufferStore`.
    MemBuffer(Arc<MemBufferRepoComposite>),
    /// Full-mirror cache wrapper. Loads every record from inner on
    /// open; subsequent reads are pure-memory; writes go to cache
    /// + inner (Sync or Async per `WriteMode`). Useful for small
    ///   hot datasets where the working set fits in RAM and every
    ///   read should be free of disk latency. Stacks on top of
    ///   MemBuffer or any other backend.
    Cached(Arc<CachedRepoComposite>),
    /// F-33 (#836): in-memory-primary repo whose `__info__`/`__interner__`
    /// stores are additionally mirrored to a durable `fjall` backing repo.
    /// Everything else (`__data__`, `__history__`, `__tx__`,
    /// `__changelog__`) is plain ephemeral in-memory. See
    /// [`HybridRepoComposite`] for the per-store routing table.
    #[cfg(feature = "fjall")]
    Hybrid(Arc<HybridRepoComposite>),
}

pub struct MemBufferRepoComposite {
    pub inner: BoxRepo,
    pub config: MemBufferConfig,
}

pub struct CachedRepoComposite {
    pub inner: BoxRepo,
    pub mode: WriteMode,
}

/// F-33 Step 2 (#836): composes an ephemeral in-memory repo (table DATA)
/// with a durable fjall repo (table CONFIGURATION mirror). `store_get`
/// routes by STORE NAME — see [`HybridRepoComposite::build_store`] for the
/// full per-name routing table and rationale.
///
/// **Critical coupling**: `__interner__` is mirrored with an ALLOW-ALL
/// classifier (every key durable), not `is_durable_table_config`. Index
/// definitions living in `__info__` reference INTERNED `u64` field ids —
/// if `__interner__` did not persist in lockstep with `__info__`, a
/// reopened hybrid table's fresh interner would reassign those ids to
/// whatever field is touched first, silently corrupting every existing
/// index (an index on `email` silently becomes an index on some other
/// field). `is_durable_table_config` is scoped to the system-record shape
/// used by `__info__` and would incorrectly reject the interner's own key
/// shapes, so it cannot be reused here.
#[cfg(feature = "fjall")]
pub struct HybridRepoComposite {
    mem: Arc<InMemoryRepo>,
    disk: Arc<FjallRepo>,
    /// Per-store-name memoization, mirroring [`InMemoryRepo`]'s own
    /// `stores: DashMap` memoization so a [`MirroredStore`]'s (streamed,
    /// full-mirror) hydration happens exactly ONCE per store name rather
    /// than on every `store_get` call.
    stores: scc::HashMap<String, Arc<OnceCell<Arc<dyn Store>>>, THasher>,
}

#[cfg(feature = "fjall")]
impl HybridRepoComposite {
    fn new(mem: Arc<InMemoryRepo>, disk: Arc<FjallRepo>) -> Self {
        Self {
            mem,
            disk,
            stores: scc::HashMap::with_hasher(THasher::default()),
        }
    }

    /// Route `name` to its intended tier and build the resulting `Store`,
    /// memoized per name (see [`HybridRepoComposite::stores`]).
    ///
    /// Mirrors the async-safe memoization shape established at
    /// `RepoInstance::get_table`
    /// (`crates/shamir-engine/src/repo/repo_instance.rs` ~line 306-326):
    /// clone the `Arc<OnceCell>` out of the map and DROP the shard guard
    /// BEFORE the init `.await`. `scc::HashMap`'s per-shard access is
    /// backed by a synchronous lock; holding that guard across a
    /// long-running init `.await` (`MirroredStore::new` streams the
    /// ENTIRE mirror on hydration) risks the same guard-across-await
    /// worker-thread starvation under runtime oversubscription. The
    /// `OnceCell` itself provides the single-init serialization — the
    /// shard lock only needs to protect the map insert, not the init.
    async fn store_get_routed(&self, name: &str) -> DbResult<Arc<dyn Store>> {
        // `entry_sync` (not `entry_async`): the shard lock is synchronous,
        // so the guard must be dropped BEFORE the init `.await` below —
        // see this method's doc comment.
        let cell = {
            let entry = self
                .stores
                .entry_sync(name.to_string())
                .or_insert_with(|| Arc::new(OnceCell::new()));
            Arc::clone(entry.get())
        };

        cell.get_or_try_init(|| async move { self.build_store(name).await })
            .await
            .cloned()
    }

    /// Construct the actual `Store` for `name` per the routing table.
    ///
    /// | Name pattern       | Backing | Why                                |
    /// |---------------------|---------|-------------------------------------|
    /// | `__history__<table>`| mem     | Actual row data (MVCC routes all data through the version log). |
    /// | `__data__<table>`   | mem     | Dead keyspace for MVCC tables — routed to mem for consistency. |
    /// | `__info__<table>`   | disk, mirrored via `is_durable_table_config` | Mixed keyspace: config + data-derived state. |
    /// | `__interner__`      | disk, mirrored via allow-ALL | Critical coupling — see struct doc. |
    /// | `__tx__`            | mem     | MVCC/WAL recovery markers derived from the (ephemeral) tx history. |
    /// | `__changelog__`     | mem     | Changefeed journal describes ephemeral row data. |
    /// | anything else       | mem + warn + debug_assert | Fail-safe: never persist unknown state by accident. |
    async fn build_store(&self, name: &str) -> DbResult<Arc<dyn Store>> {
        if name == "__interner__" {
            // Critical coupling — see `HybridRepoComposite`'s doc comment.
            // Allow-ALL classifier: every key in this store is durable
            // config that must persist alongside `__info__`'s index
            // definitions, which reference this interner's ids.
            let disk_store = self.disk.store_get(name).await?;
            let mirrored = MirroredStore::new(disk_store, |_| true).await?;
            return Ok(Arc::new(mirrored));
        }

        if name.starts_with("__info__") {
            let disk_store = self.disk.store_get(name).await?;
            let mirrored = MirroredStore::new(disk_store, is_durable_table_config).await?;
            return Ok(Arc::new(mirrored));
        }

        if name.starts_with("__history__")
            || name.starts_with("__data__")
            || name == "__tx__"
            || name == "__changelog__"
        {
            return self.mem.store_get(name).await;
        }

        // Fail-safe: an unrecognized store name defaults to ephemeral
        // in-memory rather than guessing whether it needs to persist. The
        // 6 names above are the only ones any production caller requests
        // (confirmed by an exhaustive grep of `store_get(` call sites); a
        // debug build turns a 7th name into a loud CI failure instead of a
        // silent, unreviewed persistence decision. Release builds just log
        // and continue ephemeral — the safe direction per this design's
        // allowlist philosophy.
        log::warn!("hybrid repo: unrecognized store name {name:?}, defaulting to ephemeral");
        debug_assert!(
            false,
            "hybrid repo: unrecognized store name {name:?} — add it to the routing table in HybridRepoComposite::build_store"
        );
        self.mem.store_get(name).await
    }

    async fn store_delete_routed(&self, name: &str) -> DbResult<bool> {
        // Evict the memoized `Store` FIRST — `__info__`/`__interner__`
        // names are memoized as a `MirroredStore` whose in-memory PRIMARY
        // is never registered in `self.mem` at all (it lives only inside
        // the `MirroredStore`, reachable only through this memoization
        // cache). Leaving the stale entry in place would let a subsequent
        // `store_get` on the SAME name keep returning the old, still-
        // populated `MirroredStore` instead of re-routing through
        // `build_store` (which would re-hydrate from — now empty — disk).
        let cache_removed = self.stores.remove_sync(name).is_some();

        // `mem.store_delete` is a no-op / not-found for `__info__`/
        // `__interner__` names (never registered there) and a real
        // removal for the other 4 names — safe to call unconditionally
        // for every name rather than branching twice.
        let mem_removed = self.mem.store_delete(name).await?;
        // `disk.store_delete` deletes the durable mirror. For
        // `__info__`/`__interner__` names the durable copy must be
        // deleted too — deleting `mem`/the cache alone would leave a
        // stale disk copy that resurrects the config on the next
        // hydration. For every OTHER name this is a cheap not-found
        // no-op (nothing was ever written there).
        let disk_removed = self.disk.store_delete(name).await?;
        Ok(cache_removed || mem_removed || disk_removed)
    }

    async fn stores_list_routed(&self) -> DbResult<Vec<String>> {
        let mem_names = self.mem.stores_list().await?;
        let disk_names = self.disk.stores_list().await?;
        Ok(Self::merge_store_names(mem_names, disk_names))
    }

    /// Merges `disk_names` into `names`, skipping any `disk_names` entry
    /// already present in the (growing) result. Same de-dup semantics as
    /// the linear-scan `!names.contains(&disk_name)` check this replaces,
    /// but O(1) amortized per disk name via a `TFxSet` built once up
    /// front, instead of an O(names) scan per disk name (O(names × stores)
    /// overall — schema-sized, so it grows with the number of
    /// tables/stores in the repo). `pub(crate)` so `crate::repo::tests`
    /// can unit-test the merge logic directly against edge cases (empty
    /// input, duplicate disk names, disk names already in `names`)
    /// without needing to coax genuine duplicates out of a real
    /// `FjallRepo::stores_list()`.
    pub(crate) fn merge_store_names(
        mut names: Vec<String>,
        disk_names: Vec<String>,
    ) -> Vec<String> {
        let mut seen: TFxSet<String> = names.iter().cloned().collect();
        for disk_name in disk_names {
            if seen.insert(disk_name.clone()) {
                names.push(disk_name);
            }
        }
        names
    }
}

#[async_trait::async_trait]
impl Repo for BoxRepo {
    async fn store_get<S>(&self, name: S) -> DbResult<Arc<dyn Store>>
    where
        S: AsRef<str> + Send,
    {
        match self {
            BoxRepo::InMemory(repo) => repo.store_get(name).await,
            #[cfg(feature = "fjall")]
            BoxRepo::Fjall(repo) => repo.store_get(name).await,
            BoxRepo::MemBuffer(c) => {
                let inner_store = c.inner.store_get(name).await?;
                Ok(Arc::new(MemBufferStore::new(inner_store, c.config.clone())))
            }
            BoxRepo::Cached(c) => {
                let inner_store = c.inner.store_get(name).await?;
                let cached = match c.mode {
                    WriteMode::Sync => CachedStore::new_sync(inner_store).await?,
                    WriteMode::Async => CachedStore::new_async(inner_store).await?,
                };
                Ok(Arc::new(cached))
            }
            #[cfg(feature = "fjall")]
            BoxRepo::Hybrid(c) => c.store_get_routed(name.as_ref()).await,
        }
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        match self {
            BoxRepo::InMemory(repo) => repo.store_delete(name).await,
            #[cfg(feature = "fjall")]
            BoxRepo::Fjall(repo) => repo.store_delete(name).await,
            BoxRepo::MemBuffer(c) => c.inner.store_delete(name).await,
            BoxRepo::Cached(c) => c.inner.store_delete(name).await,
            #[cfg(feature = "fjall")]
            BoxRepo::Hybrid(c) => c.store_delete_routed(name.as_ref()).await,
        }
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        match self {
            BoxRepo::InMemory(repo) => repo.stores_list().await,
            #[cfg(feature = "fjall")]
            BoxRepo::Fjall(repo) => repo.stores_list().await,
            BoxRepo::MemBuffer(c) => c.inner.stores_list().await,
            BoxRepo::Cached(c) => c.inner.stores_list().await,
            #[cfg(feature = "fjall")]
            BoxRepo::Hybrid(c) => c.stores_list_routed().await,
        }
    }
}

impl From<Arc<InMemoryRepo>> for BoxRepo {
    fn from(repo: Arc<InMemoryRepo>) -> Self {
        BoxRepo::InMemory(repo)
    }
}

#[cfg(feature = "fjall")]
impl From<Arc<FjallRepo>> for BoxRepo {
    fn from(repo: Arc<FjallRepo>) -> Self {
        BoxRepo::Fjall(repo)
    }
}

// ============================================================================
// RepoFactory trait for async repo creation
// ============================================================================

/// Factory trait for asynchronously creating repositories.
/// Used to defer blocking file I/O operations to spawn_blocking.
#[async_trait::async_trait]
pub trait RepoFactory: Send + Sync {
    /// Creates a new repository, performing any blocking I/O in a separate thread.
    async fn create(&self) -> DbResult<BoxRepo>;
}

// ============================================================================
// RepoFactory implementations for async repo creation
// ============================================================================

/// Factory for InMemoryRepo - no blocking I/O needed
pub struct InMemoryRepoFactory;

#[async_trait::async_trait]
impl RepoFactory for InMemoryRepoFactory {
    async fn create(&self) -> DbResult<BoxRepo> {
        Ok(BoxRepo::InMemory(Arc::new(InMemoryRepo::new())))
    }
}

#[cfg(feature = "fjall")]
pub struct FjallRepoFactory {
    pub path: PathBuf,
}

#[cfg(feature = "fjall")]
#[async_trait::async_trait]
impl RepoFactory for FjallRepoFactory {
    async fn create(&self) -> DbResult<BoxRepo> {
        let path = self.path.clone();
        let repo = task::spawn_blocking(move || FjallRepo::new(path))
            .await
            .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))??;
        Ok(BoxRepo::Fjall(Arc::new(repo)))
    }
}

/// F-33 Step 2 (#836): builds a [`HybridRepoComposite`] — an in-memory
/// repo for ephemeral table data plus a DIRECT (not `MemBuffer`-wrapped)
/// fjall repo for the durable config mirror. Unlike
/// [`BoxRepoFactory::fjall`]'s default-wrapped convenience constructor,
/// the disk side here is deliberately unwrapped: config writes are rare
/// DDL events that should land durably right away, not sit in a buffered
/// flush window.
#[cfg(feature = "fjall")]
pub struct HybridRepoFactory {
    pub info_path: PathBuf,
}

#[cfg(feature = "fjall")]
#[async_trait::async_trait]
impl RepoFactory for HybridRepoFactory {
    async fn create(&self) -> DbResult<BoxRepo> {
        let mem = Arc::new(InMemoryRepo::new());
        let path = self.info_path.clone();
        let disk = task::spawn_blocking(move || FjallRepo::new(path))
            .await
            .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))??;
        Ok(BoxRepo::Hybrid(Arc::new(HybridRepoComposite::new(
            mem,
            Arc::new(disk),
        ))))
    }
}

// ============================================================================
// BoxRepoFactory - enum for type-erased factory
// ============================================================================

/// Type-erased factory that can create any repo type
pub enum BoxRepoFactory {
    InMemory(InMemoryRepoFactory),
    #[cfg(feature = "fjall")]
    Fjall(FjallRepoFactory),
    /// MemBuffer wrapper factory.
    MemBuffer(Box<MemBufferRepoFactory>),
    /// Full-mirror cache wrapper factory. Stacks on top of any
    /// other factory.
    Cached(Box<CachedRepoFactory>),
    /// F-33 (#836): hybrid backend — ephemeral in-memory data, durable
    /// fjall config mirror. See [`HybridRepoComposite`].
    #[cfg(feature = "fjall")]
    Hybrid(HybridRepoFactory),
}

pub struct MemBufferRepoFactory {
    pub inner: BoxRepoFactory,
    pub config: MemBufferConfig,
}

pub struct CachedRepoFactory {
    pub inner: BoxRepoFactory,
    pub mode: WriteMode,
}

impl BoxRepoFactory {
    /// The default `MemBufferConfig` we wrap every disk factory in.
    ///
    /// Conservative — small enough that memory is never a surprise,
    /// flush window short enough that "kill -9 = data loss" stays
    /// at sub-second scope. Matches industry default (Postgres
    /// `synchronous_commit=off`, SQLite `PRAGMA synchronous=NORMAL`).
    ///
    /// Users tuning for either side (more cache or stricter
    /// durability) construct their own `MemBufferConfig` and call
    /// `BoxRepoFactory::membuffer(inner, custom)` explicitly.
    fn default_membuffer_config() -> MemBufferConfig {
        MemBufferConfig {
            // 64 MiB resident cap — comfortable for embedded /
            // small server. Tune via explicit membuffer() composer
            // for hot-set workloads.
            max_bytes: 64 * 1024 * 1024,
            max_entries: 100_000,
            ttl_ms: None,
            // 500 ms idle flush — matches `MemBufferConfig::default()`
            // and the eventual DDL default. Per-table override via
            // DDL (next task).
            flush_interval_ms: 500,
            flush_batch_size: 256,
        }
    }

    /// Wrap a raw factory in the default `MemBufferConfig`. Used
    /// internally by every disk-backend constructor — they all
    /// return a MemBuffer-wrapped factory by default.
    fn wrapped(raw: BoxRepoFactory) -> Self {
        BoxRepoFactory::MemBuffer(Box::new(MemBufferRepoFactory {
            inner: raw,
            config: Self::default_membuffer_config(),
        }))
    }

    /// In-memory factory. NOT wrapped in MemBuffer — the underlying
    /// `InMemoryStore` is already memory-resident, the wrapper
    /// would just add a second cache layer with no perf gain and
    /// real read-after-write semantics confusion.
    pub fn in_memory() -> Self {
        BoxRepoFactory::InMemory(InMemoryRepoFactory)
    }

    /// Fjall, MemBuffer-wrapped by default.
    #[cfg(feature = "fjall")]
    pub fn fjall(path: impl Into<PathBuf>) -> Self {
        Self::wrapped(BoxRepoFactory::Fjall(FjallRepoFactory {
            path: path.into(),
        }))
    }

    // ---------------------- raw (unwrapped) factories ----------------------
    //
    // For tooling and tests that need bit-for-bit on-disk semantics
    // (no buffering window). NOT recommended for application code.

    /// Raw fjall, no MemBuffer.
    #[cfg(feature = "fjall")]
    pub fn fjall_raw(path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Fjall(FjallRepoFactory { path: path.into() })
    }

    /// Hybrid backend: ephemeral in-memory table data, durable fjall
    /// mirror (at `info_path`) for `__info__`/`__interner__`. NOT
    /// `MemBuffer`-wrapped — see [`HybridRepoFactory`]'s doc comment.
    #[cfg(feature = "fjall")]
    pub fn hybrid(info_path: impl Into<PathBuf>) -> Self {
        BoxRepoFactory::Hybrid(HybridRepoFactory {
            info_path: info_path.into(),
        })
    }

    /// Wrap a factory in a custom-config MemBuffer layer. Use this
    /// when the conservative default config doesn't fit your
    /// workload (very hot dataset, very strict latency window, etc).
    pub fn membuffer(inner: BoxRepoFactory, config: MemBufferConfig) -> Self {
        BoxRepoFactory::MemBuffer(Box::new(MemBufferRepoFactory { inner, config }))
    }

    /// Stack a full-mirror cache on top of `inner`. Sync mode
    /// writes through synchronously; Async mode write-behind via
    /// background tasks. Best for small hot datasets where the
    /// working set fits in RAM.
    ///
    /// Composable with `membuffer`: `cached(fjall(path))` gives
    /// `Cached → MemBuffer → fjall`.
    pub fn cached(inner: BoxRepoFactory, mode: WriteMode) -> Self {
        BoxRepoFactory::Cached(Box::new(CachedRepoFactory { inner, mode }))
    }

    /// The on-disk directory this factory ultimately writes to, if any.
    ///
    /// Disk backends return their `path`; `InMemory` returns `None`; the
    /// `MemBuffer`/`Cached` wrappers delegate to their inner factory so the
    /// real disk path at the bottom of the stack surfaces. Used by
    /// [`RepoInstance::repo_wal`] to decide whether to construct a
    /// file-backed WAL group (disk) or fall back to the KV-marker WAL
    /// (in-memory).
    pub fn backing_dir(&self) -> Option<PathBuf> {
        match self {
            BoxRepoFactory::InMemory(_) => None,
            #[cfg(feature = "fjall")]
            BoxRepoFactory::Fjall(f) => Some(f.path.clone()),
            BoxRepoFactory::MemBuffer(f) => f.inner.backing_dir(),
            BoxRepoFactory::Cached(f) => f.inner.backing_dir(),
            // A hybrid repo's actual DATA (`__history__`/`__data__`) is
            // ephemeral in-memory, same disposition as `InMemory`. A
            // file-backed WAL would durably record inflight write markers
            // that, on the next open, replay into a freshly-EMPTY
            // `__history__` — resurrecting a torn fragment of a dataset
            // that's supposed to be gone. `None` selects the in-memory
            // KV-marker WAL instead, consistent with the ephemeral-data
            // half of this design.
            #[cfg(feature = "fjall")]
            BoxRepoFactory::Hybrid(_) => None,
        }
    }
}

#[async_trait::async_trait]
impl RepoFactory for BoxRepoFactory {
    async fn create(&self) -> DbResult<BoxRepo> {
        match self {
            BoxRepoFactory::InMemory(f) => f.create().await,
            #[cfg(feature = "fjall")]
            BoxRepoFactory::Fjall(f) => f.create().await,
            BoxRepoFactory::MemBuffer(f) => {
                let inner_repo = f.inner.create().await?;
                Ok(BoxRepo::MemBuffer(Arc::new(MemBufferRepoComposite {
                    inner: inner_repo,
                    config: f.config.clone(),
                })))
            }
            BoxRepoFactory::Cached(f) => {
                let inner_repo = f.inner.create().await?;
                Ok(BoxRepo::Cached(Arc::new(CachedRepoComposite {
                    inner: inner_repo,
                    mode: f.mode,
                })))
            }
            #[cfg(feature = "fjall")]
            BoxRepoFactory::Hybrid(f) => f.create().await,
        }
    }
}

impl Clone for BoxRepoFactory {
    fn clone(&self) -> Self {
        match self {
            BoxRepoFactory::InMemory(_) => BoxRepoFactory::in_memory(),
            #[cfg(feature = "fjall")]
            BoxRepoFactory::Fjall(f) => BoxRepoFactory::fjall(f.path.clone()),
            BoxRepoFactory::MemBuffer(f) => {
                BoxRepoFactory::membuffer(f.inner.clone(), f.config.clone())
            }
            BoxRepoFactory::Cached(f) => BoxRepoFactory::cached(f.inner.clone(), f.mode),
            #[cfg(feature = "fjall")]
            BoxRepoFactory::Hybrid(f) => BoxRepoFactory::hybrid(f.info_path.clone()),
        }
    }
}
