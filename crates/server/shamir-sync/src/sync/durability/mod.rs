//! Server-default durability policy; sparse per-repo overrides are not PR1.

pub mod maintenance_lock;

pub mod backup;
pub mod restore;
pub mod tx_registry;

mod config;

#[cfg(test)]
mod tests;

pub use config::DurabilityConfig;
