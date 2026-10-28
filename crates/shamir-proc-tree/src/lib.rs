//! Bounded compiler process-tree supervision for guest Rust builds.
//!
//! The compile being supervised may start an sccache server; terminating the
//! process tree also terminates that server. This is an acknowledged tradeoff.

#![cfg_attr(not(windows), forbid(unsafe_code))]
#![cfg_attr(windows, deny(unsafe_code))]

#[cfg(windows)]
mod job_object_windows;
#[cfg(unix)]
mod process_tree_unix;
#[cfg(windows)]
mod process_tree_windows;
mod supervise;

#[cfg(test)]
mod tests;

#[cfg(unix)]
pub use process_tree_unix::ProcessTree;
#[cfg(windows)]
pub use process_tree_windows::ProcessTree;
pub use supervise::{supervise, TeardownFaults};
