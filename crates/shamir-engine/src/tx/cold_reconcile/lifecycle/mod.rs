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
#[cfg(test)]
pub(crate) use job::JobSubmitError;
pub(crate) use owner::{ForegroundLifecycle, RepoUnitLifecycle, WorkerTerminal};
#[cfg(test)]
pub(crate) use owner::{InstallError, RepoWorkerLifecycle, ShutdownError};
#[cfg(test)]
pub(crate) use worker::{spawn_worker, FailClosedResource, LifecycleStatus, WorkerLease};

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
