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

use shamir_storage::types::{Repo, Store};
use std::path::PathBuf;
use std::sync::Arc;
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
}

pub struct MemBufferRepoComposite {
    pub inner: BoxRepo,
    pub config: MemBufferConfig,
}

pub struct CachedRepoComposite {
    pub inner: BoxRepo,
    pub mode: WriteMode,
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
        }
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        match self {
            BoxRepo::InMemory(repo) => repo.store_delete(name).await,
            #[cfg(feature = "fjall")]
            BoxRepo::Fjall(repo) => repo.store_delete(name).await,
            BoxRepo::MemBuffer(c) => c.inner.store_delete(name).await,
            BoxRepo::Cached(c) => c.inner.store_delete(name).await,
        }
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        match self {
            BoxRepo::InMemory(repo) => repo.stores_list().await,
            #[cfg(feature = "fjall")]
            BoxRepo::Fjall(repo) => repo.stores_list().await,
            BoxRepo::MemBuffer(c) => c.inner.stores_list().await,
            BoxRepo::Cached(c) => c.inner.stores_list().await,
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
        }
    }
}
