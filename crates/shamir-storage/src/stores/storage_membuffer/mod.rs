//! `MemBufferStore` — concurrent write-back buffer over any `Store`,
//! backed by **`moka`** as the in-RAM cache.
//!
//! # Design
//!
//! ```text
//!  ┌──────────────────────────────────────────────────────┐
//!  │  read / write                                        │
//!  │     │                                                │
//!  │     ▼                                                │
//!  │  ┌─────────────────────────────────────────┐         │
//!  │  │ moka::future::Cache<RecordKey, Slot>    │         │
//!  │  │  - max_capacity, weigher, TTL handled   │         │
//!  │  │    by moka (W-TinyLFU eviction)         │         │
//!  │  │  - lock-free read path (per-thread      │         │
//!  │  │    event buffers internally)            │         │
//!  │  │  - async eviction listener flushes      │         │
//!  │  │    dirty entries inline                 │         │
//!  │  └────────────┬────────────────────────────┘         │
//!  │               │                                       │
//!  │  ┌────────────┴────────────────────────────┐         │
//!  │  │ DashSet<RecordKey> dirty                │         │
//!  │  │  - lock-free per-shard inserts/removes  │         │
//!  │  │  - drained by background flusher        │         │
//!  │  └────────────┬────────────────────────────┘         │
//!  └───────────────┼──────────────────────────────────────┘
//!                  ▼
//!           ┌─────────────┐
//!           │ Inner Store │
//!           └─────────────┘
//! ```
//!
//! # Why moka
//!
//! The previous implementation was a `Mutex<LruCache>` + `Mutex<HashSet>`.
//! Bench `membuffer_concurrent_rw` showed flat throughput as readers
//! scaled (632 Kelem/s at 8 readers vs 754 Kelem/s at 1) — every op
//! serialised on the single cache mutex.
//!
//! `moka` is a production-grade concurrent cache:
//!   * eviction handled internally (W-TinyLFU + LRU window),
//!   * TTL handled by `time_to_live`,
//!   * byte-cap handled by `weigher` + `max_capacity`,
//!   * reads + writes are lock-free for non-conflicting keys (per-thread
//!     event buffers, drained by a background maintenance task — the
//!     same pattern Caffeine uses on the JVM).
//!
//! # Durability contract (unchanged)
//!
//! `insert / set / remove` return as soon as the dirty set is
//! updated — the inner store hasn't been touched yet. The flusher drains
//! `dirty` in batches; for synchronous durability call `flush().await`.
//!
//! # Concurrency (RV-STORAGE-98f6650d)
//!
//! Dirty mutation routes (`insert`/`insert_many`/`set`/`set_many`/
//! `remove`/`remove_many`/`transact`/drain) hold a fixed-size striped
//! revision epoch (`revision::StripeGuard`, packed `{revision, active}`
//! CAS) across their publication. The moka cache stores tagged values
//! `{rev, Slot}` and is populated ONLY by clean backing fills — writers
//! never republish into it. Every cache hit validates its tag against the
//! stripe stamp (unchanged stamp, zero active writers, matching revision,
//! non-exhausted stripe), so stale physical entries are harmless. See
//! `revision.rs`.
//!
//! # apply_config
//!
//! Atomic config fields are written directly. For sizing /
//! TTL changes, the cache is **rebuilt** with the new config —
//! `drain_all().await` runs first so no dirty data is lost. This is rare
//! (DDL-driven only), so the rebuild cost is amortised. The rebuilt cache
//! uses the same tagged value type.

mod core;
mod revision;
mod store_impl;

pub use core::{MemBufferConfig, MemBufferStore};

#[cfg(test)]
pub(crate) use core::FillPauseHook;
pub(crate) use core::Slot;
#[cfg(test)]
pub(crate) use revision::StripedRevisions;
