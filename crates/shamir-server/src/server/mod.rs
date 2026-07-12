//! Boot orchestration — extracted from `main.rs` so it is reusable from
//! integration tests.
//!
//! [`ServerLauncher`] owns the [`Config`] + bootstrap policy and produces
//! a [`ServerHandle`] when launched. The handle holds the bound listener
//! addresses (so test code can connect a real client to them) plus
//! shutdown plumbing for the listener tasks, the background scheduler,
//! and the audit appender.
//!
//! The launcher does NOT install the rustls crypto provider — callers
//! must do that exactly once per process (`rustls::crypto::aws_lc_rs::default_provider().install_default()`).
//! This is enforced by rustls itself: a second install is a no-op.

mod boot_error;
mod bootstrap_mode;
mod meta_sinks;
mod server_handle;
mod server_launcher;

pub use boot_error::BootError;
pub use bootstrap_mode::BootstrapMode;
pub use server_handle::ServerHandle;
pub use server_launcher::audit_store_b_vs_directory;
pub use server_launcher::ServerLauncher;
