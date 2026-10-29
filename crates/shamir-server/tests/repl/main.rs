//! Repl integration tests.

#[path = "../common/mod.rs"]
mod common;
mod repl_convergence_e2e;
mod repl_pull_e2e;
mod repl_supervisor_boot;
#[cfg(unix)]
#[path = "../seed_recovery_e2e/mod.rs"]
mod seed_recovery_e2e;
mod set_replicator_wire;
mod set_superuser_wire;
