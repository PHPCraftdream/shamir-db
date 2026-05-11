//! Storage backends for ShamirDB — the `Store` / `Repo` traits plus the
//! per-engine implementations.
//!
//! Per-backend modules are gated behind cargo features. The default
//! feature set (`all-backends`) enables every backend so today's
//! consumers see no change. Embedded / minimal builds can opt out:
//!
//! ```toml
//! shamir-storage = { version = "0.1", default-features = false, features = ["redb"] }
//! ```
//!
//! `error`, `types` (Store/Repo trait surface) and `storage_in_memory`
//! are always compiled — they have no extra deps and are required by
//! tests across the workspace.

pub mod error;
pub mod storage_cached;
pub mod storage_in_memory;
pub mod storage_membuffer;
pub mod types;

#[cfg(feature = "canopy")]
pub mod storage_canopy;
#[cfg(feature = "fjall")]
pub mod storage_fjall;
#[cfg(feature = "nebari")]
pub mod storage_nebari;
#[cfg(feature = "persy")]
pub mod storage_persy;
#[cfg(feature = "redb")]
pub mod storage_redb;
#[cfg(feature = "sled")]
pub mod storage_sled;
