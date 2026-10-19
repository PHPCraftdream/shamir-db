mod access_control;
mod artifact_kind;
mod changelog;
#[path = "../execute/repo_admin/revocation/mod.rs"]
mod revocation;
pub use revocation::receiver::{ChangelogDeliveryGate, ChangelogReceiver};
mod core;
mod db_gateway;
mod db_management;
#[path = "seed/tests/durability_init.rs"]
mod durability_init;
mod function_management;
pub(crate) mod schema_management;
mod seed;
mod seed_catalogue;
mod table_management;
mod validator_management;

#[cfg(test)]
mod tests;

pub(super) const SYSTEM_DB_NAME: &str = "__system__";

pub use artifact_kind::{ArtifactKind, KIND_FIELD};
pub use core::{FunctionSource, ReplicationControlGuard, ShamirDb};
#[cfg(any(test, feature = "test-util"))]
pub use seed_catalogue::{
    FinalWriteBarrierForTest, SeedCatalogueFlushFaultForTest, TerminalRowSetFaultForTest,
};

mod shutdown_report;
pub use shutdown_report::ShutdownReport;
