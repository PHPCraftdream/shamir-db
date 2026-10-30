//! Version key codec and MVCC version plumbing (guards, providers, overlays).

#[path = "wal_event/version_codec.rs"]
pub mod version_codec;
pub mod version_guard;
pub mod version_provider;
pub mod versioned_overlay;
pub mod visibility_metrics;
pub mod wal_event;

#[cfg(test)]
mod tests;
