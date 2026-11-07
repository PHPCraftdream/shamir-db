//! Formerly in-crate `shamir-client` tests, moved to this integration target.
//!
//! `ambient_sync_tests`, `cursor_stream_tests`, `interner_cache_tests` and
//! `resume_and_scratch_tests` are deliberately NOT wired in: they still reach
//! crate-private client items (`Client::for_test`, `LifecycleHooks`,
//! `should_advertise_interner_epochs`, the pub(crate) `Client::roundtrip`,
//! and the `*_for_test` accessors) that an external crate cannot see. They
//! sit unreachable (rust-guard exceptions in .rust-guard/exceptions.toml)
//! until a public seam exists for them.

mod auth_identity_tests;
mod v2_passthrough_tests;
