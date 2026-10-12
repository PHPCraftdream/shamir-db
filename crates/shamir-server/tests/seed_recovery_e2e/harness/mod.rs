//! Shared fixtures for the SEED-5 certification scenarios.
//!
//! Everything disk-bound here is Unix-only (the whole test crate is
//! `#![cfg(unix)]` in `main.rs`): the seed export/install path requires
//! directory fsync, and the child-process kill test needs POSIX signals.

pub mod child;
pub mod follower;
pub mod ops;
pub mod wire;
