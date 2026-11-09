//! Shared subscription registry used by the server engine and sync loops.

pub mod registry;

pub use registry::SubscriptionRegistry;

#[cfg(test)]
mod tests;
