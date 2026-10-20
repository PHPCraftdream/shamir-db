//! Storage backends for ShamirDB — the `Store` / `Repo` traits plus the
//! per-engine implementations.
//!
//! Per-backend modules are gated behind cargo features. The default
//! feature set (`all-backends`) enables every backend so today's
//! consumers see no change. Embedded / minimal builds can opt out:
//!
//! ```toml
//! shamir-storage = { version = "0.1", default-features = false, features = ["fjall"] }
//! ```
//!
//! `error`, `types` (Store/Repo trait surface) and `storage_in_memory`
//! are always compiled — they have no extra deps and are required by
//! tests across the workspace.

pub mod core;
pub mod key_bytes;
mod stores;

pub use core::{error, types};
pub use stores::storage_cached;
pub use stores::storage_in_memory;
pub use stores::storage_membuffer;
pub use stores::storage_mirrored;

#[cfg(feature = "fjall")]
pub use stores::storage_fjall;

/// Task #535/#539-style test seam: commit-window park for the repository-wide
/// atomic batch (driven from the shamir-engine test binary — always compiled,
/// inert when not armed).
#[doc(hidden)]
#[path = "test_seams/commit_window_park_seam.rs"]
pub mod commit_window_park_seam;

/// Task #535/#539: test-only deterministic seam for `MemBufferStore::drain_once`
/// (see `membuffer_clear_race_hook`'s module doc for the current shape).
#[cfg(test)]
#[path = "test_seams/membuffer_clear_race_hook.rs"]
mod membuffer_clear_race_hook;

#[cfg(test)]
mod tests;
