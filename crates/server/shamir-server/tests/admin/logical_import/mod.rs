//! Offline logical-import integration tests (CI-only).
//!
//! Positive round trips (export → import → engine re-read), scope options
//! (`--schema-only`, `--as-db`), identity/ownership mapping, and the typed
//! refusal matrix for corrupt, incoherent and conflicting inputs.

#[path = "common.rs"]
mod common;
#[path = "corrupt.rs"]
mod corrupt;
#[path = "identities.rs"]
mod identities;
#[path = "refusals.rs"]
mod refusals;
#[path = "roundtrip.rs"]
mod roundtrip;
#[path = "scope.rs"]
mod scope;
