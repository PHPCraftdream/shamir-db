//! Activity-view export (`shamir_active_*` gauges) plus the relocated
//! per-repo transaction-conflict counters (`transactions.rs`, a
//! rename-only move from `export/transactions.rs` to leave this topical
//! subfolder room instead of exceeding the parent's 7-entry limit).

mod activity_export;
#[cfg(test)]
mod tests;
pub(crate) mod transactions;

pub use activity_export::ActivityMetricsSnapshot;
pub(crate) use activity_export::{render_activity_series, ActivitySource};
