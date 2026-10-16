//! Repository-owned worker lifecycle foundation.

#[allow(dead_code)]
// Inactive lifecycle interlock foundation; exercised by tests and reserved for receipt publication wiring.
pub(crate) mod interlock;
#[allow(dead_code)] // Inactive job envelope retained for the generic worker API and tests.
pub(crate) mod job;
#[allow(dead_code)] // Inactive lifecycle owner foundation; production wiring is handled separately.
pub(crate) mod owner;
#[allow(dead_code)]
// Inactive worker foundation; retained for tests and future receipt-pipeline wiring.
pub(crate) mod worker;

#[cfg(test)]
pub(crate) use interlock::PubTermInterlock;
// Receipt-pipeline production surface (PR6b-2c): these are re-exported
// unconditionally; the ones without non-test callers yet carry their own
// unused-import allowance.
pub(crate) use job::JobSubmitError;
pub(crate) use owner::{ForegroundLifecycle, RepoUnitLifecycle, WorkerTerminal};
#[cfg_attr(not(test), allow(unused_imports))]
// prod-dead: receipt pipeline (W3+) installs the worker
pub(crate) use owner::{InstallError, RepoWorkerLifecycle, ShutdownError};
#[cfg_attr(not(test), allow(unused_imports))]
// prod-dead: WorkerLease is a receipt-worker (W3+) bound
pub(crate) use worker::WorkerLease;
#[cfg(test)]
pub(crate) use worker::{spawn_worker, FailClosedResource, LifecycleStatus};

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
