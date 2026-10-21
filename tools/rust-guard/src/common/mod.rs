//! Shared primitives for the rust-guard subcommands: git-backed file
//! access, structured findings/reports, and the exceptions manifest.
//! This module is declarations and re-exports only (repo rule); the
//! items live in the child modules.

pub(crate) mod exceptions;
pub(crate) mod finding;
pub(crate) mod gitfs;
pub(crate) mod suggest;
