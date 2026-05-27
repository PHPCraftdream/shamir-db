//! Versioned metadata envelope and unified `__meta__/*` namespace for
//! engine-level persistent metadata (indexes, tables, WAL state, etc.).
//!
//! Introduced as Phase 0 of the new index system (FTS / Functional /
//! Vector). Existing `system:*` keys continue to work unchanged — this
//! module provides forward-compatible primitives for the rewrite.

pub mod envelope;
pub mod namespace;
pub mod recovery_marker;

pub use envelope::{MetaEnvelope, MetaError, ENVELOPE_MAGIC, ENVELOPE_VERSION};
pub use namespace::MetaKey;
