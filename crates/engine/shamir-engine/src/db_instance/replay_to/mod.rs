//! Strict cold replay of a bounded WAL over an offline repository copy.

mod api;
mod open;
mod scope;
mod staging;

pub use api::{replay_to, ReplayReport, ReplayToError, ReplayToOpCounters};
pub use scope::{ReplayScope, ReplayScopeError};

#[cfg(test)]
mod tests;
