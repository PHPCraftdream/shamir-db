//! Integration tests for `shamir_server::user_directory::FjallUserDirectory`.
//!
//! Covers the durability requirement (state survives restart), the
//! atomicity / monotonicity rules from spec §12.6, and concurrent insert
//! ID-uniqueness expectations.
//!
//! All tests use `tempfile::TempDir` so the redb file is cleaned up after
//! each run.

mod basic;
mod common;
mod invalidate;
mod remove_v2;
mod superuser;
