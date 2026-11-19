//! Connection seam of the administrative shell (PR1): declarations and
//! re-exports ONLY (rust-guard structure gate).
//!
//! `scram.rs` implements the TLS-TCP + SCRAM and IPC + SCRAM connection
//! (`docs/dev-artifacts/design/admin-shell.md` §3.2, PR1 row); `profile.rs`
//! defines the strict on-disk connection profile. Nothing else belongs in
//! this file.

mod profile;
mod scram;

pub(in crate::shell) use profile::{list, load, load_from, Profile};
pub(in crate::shell) use scram::{prepare, request_error, REQUEST_TIMEOUT};
