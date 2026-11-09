//! Static runtime policy: protocol versions and transaction commit policy.

pub(crate) mod maintenance_lock;

pub mod service;
pub mod token_cli;
pub mod tokens;
pub(crate) mod transactions_config;
pub use shamir_sync::version;
#[cfg(windows)]
pub mod windows_service;
