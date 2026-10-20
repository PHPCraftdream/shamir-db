mod cache;
pub(crate) mod core;
mod filter;
pub(crate) mod flow;
mod journal;

// Module-name re-exports so the old flat paths (`super::decode_cache`,
// `crate::subscriptions::payload`, `crate::subscriptions::filter_eval`,
// `crate::subscriptions::flow::reactive`, …) keep resolving for siblings,
// tests and the benches.
pub(crate) use cache::{decode_cache, deliver_cache};
pub(crate) use core::bridge;
pub use core::payload;
pub use filter::{filter_eval, target_match};
pub(crate) use flow::{push, registry};
pub(crate) use journal::{db_journal_source, journal_drain, journal_source};

pub use registry::SubscriptionRegistry;

#[cfg(test)]
mod tests;
