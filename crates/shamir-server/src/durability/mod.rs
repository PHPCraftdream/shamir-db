//! Server-default durability policy; sparse per-repo overrides are not PR1.

mod config;

pub use config::DurabilityConfig;

#[cfg(test)]
mod tests;
