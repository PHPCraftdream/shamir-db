//! Versioned envelope for engine metadata.
//!
//! Wraps every persisted `__meta__/*` payload in
//! `[magic="SDB2"][version: u16][written_at_nanos: u64][payload: T]`
//! so future migrations can dispatch on `version` without ambiguity.

pub use shamir_index::{MetaEnvelope, MetaError, ENVELOPE_MAGIC, ENVELOPE_VERSION};
