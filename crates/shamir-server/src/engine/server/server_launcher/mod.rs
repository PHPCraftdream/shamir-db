//! Boot orchestration — [`ServerLauncher`] owns the
//! [`Config`](crate::config::Config) plus the bootstrap policy and produces
//! a [`ServerHandle`](crate::server::server_handle::ServerHandle) when
//! launched.
//!
//! The launcher does NOT install the rustls crypto provider — callers
//! must do that exactly once per process (`rustls::crypto::aws_lc_rs::default_provider().install_default()`).
//! This is enforced by rustls itself: a second install is a no-op.

mod accept_loops;
mod audit;
mod helpers;
mod launch;
mod listeners;

#[cfg(test)]
mod tests;

pub use audit::audit_store_b_vs_directory;
pub use launch::ServerLauncher;
