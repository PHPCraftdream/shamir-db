//! Repository-owned worker lifecycle foundation (S5c-X PR6b-2b).
//!
//! Three objects: the lifecycle owner, the worker execution state, and the
//! job envelope. States are `Open -> Closing -> Joined`; quarantine is
//! unconditional, and shutdown enforces ONE absolute deadline that covers
//! worker shutdown + commit quiescence + flush + checkpoint (the worker
//! performs all of those on its exit path). Full contract:
//! `docs/guide-docs/follower-journal-durability.md`.

pub(crate) mod interlock;
pub(crate) mod job;
pub(crate) mod owner;
pub(crate) mod worker;

pub use interlock::PubTermInterlock;
pub use job::{JobSubmitError, LifecycleJob};
pub use owner::{
    InstallError, RepoUnitLifecycle, RepoWorkerLifecycle, ShutdownError, WorkerTerminal,
};
pub use worker::{spawn_worker, WorkerExecutionState, WorkerLease};

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
