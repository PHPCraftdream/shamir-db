//! Storage backends for ShamirDB — the `Store` / `Repo` traits plus the
//! per-engine implementations (in_memory, redb, sled, fjall, nebari,
//! persy, canopy) and a generic in-memory caching wrapper.
//!
//! This crate sits on top of `shamir-types` (it produces / consumes
//! `InnerValue`, `RecordId`, etc.) and is consumed by `shamir-db` —
//! specifically the engine layer in `shamir-db::db::engine` — so that
//! splitting backends behind cargo features later is a small, local
//! change rather than a workspace-wide refactor.
//!
//! The module map mirrors the in-tree layout (`db/storage/*`) exactly,
//! and `shamir-db` re-exports this whole crate as `db::storage` so
//! existing `crate::storage_redb::*` paths keep working
//! without any caller-side rewrites.

pub mod error;
pub mod storage_cached;
pub mod storage_canopy;
pub mod storage_fjall;
pub mod storage_in_memory;
pub mod storage_nebari;
pub mod storage_persy;
pub mod storage_redb;
pub mod storage_sled;
pub mod types;
