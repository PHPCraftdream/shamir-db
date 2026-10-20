//! Manifest for `stores` — per-backend storage implementations.

pub mod storage_cached;
pub mod storage_in_memory;
pub mod storage_membuffer;
pub mod storage_mirrored;

#[cfg(feature = "fjall")]
pub mod storage_fjall;
