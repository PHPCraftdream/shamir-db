mod access;
mod catalog;
mod core;
mod db;
#[path = "../execute/repo_admin/revocation/mod.rs"]
mod revocation;
pub use revocation::receiver::{ChangelogDeliveryGate, ChangelogReceiver};
#[path = "seed/tests/durability_init.rs"]
mod durability_init;
mod seed;

#[cfg(test)]
mod tests;

pub use catalog::{ArtifactKind, ShutdownReport, KIND_FIELD};
#[cfg(any(test, feature = "test-util"))]
pub use catalog::{
    FinalWriteBarrierForTest, SeedCatalogueFlushFaultForTest, TerminalRowSetFaultForTest,
};
pub(super) use core::SYSTEM_DB_NAME;
pub use core::{FunctionSource, ReplicationControlGuard, ShamirDb};
pub(crate) use db::schema_management;
