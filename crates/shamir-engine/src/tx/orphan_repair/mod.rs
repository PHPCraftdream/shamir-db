mod metrics;
mod missing_evidence;
mod repair;
mod rescan;
mod settle;

pub(in crate::tx::drainer) use metrics::{record_orphan_failure, Metrics};
pub(in crate::tx::drainer) use missing_evidence::settle_missing_evidence;
pub(in crate::tx::drainer) use rescan::rescan_window_prefix;

#[cfg(test)]
pub(in crate::tx::drainer) use repair::repair_event;
pub(in crate::tx::drainer) use settle::settle_event;

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
