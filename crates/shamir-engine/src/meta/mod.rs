//! Versioned metadata envelope and unified `__meta__/*` namespace for
//! engine-level persistent metadata (indexes, tables, WAL state, etc.).
//!
//! Introduced as Phase 0 of the new index system (FTS / Functional /
//! Vector). Existing `system:*` keys continue to work unchanged — this
//! module provides forward-compatible primitives for the rewrite.

pub mod envelope;
pub mod namespace;
pub mod recovery_marker;
pub mod repl;
// Back-compat alias: every pre-existing `meta::repl_bookmark::…` path in
// this crate, shamir-db, shamir-server and tests keeps resolving unchanged.
pub use repl as repl_bookmark;
pub mod repo_identity;

pub use envelope::{MetaEnvelope, MetaError, ENVELOPE_MAGIC, ENVELOPE_VERSION};
pub use namespace::MetaKey;
pub use repo_identity::LifecycleState;

#[cfg(test)]
mod tests;
