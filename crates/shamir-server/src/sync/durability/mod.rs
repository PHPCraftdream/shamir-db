//! Server-default durability policy; sparse per-repo overrides are not PR1.

pub(crate) mod maintenance_lock;

pub mod backup;
pub mod restore;
pub mod tx_registry;

mod config;

pub use config::DurabilityConfig;

#[cfg(test)]
mod tests;
