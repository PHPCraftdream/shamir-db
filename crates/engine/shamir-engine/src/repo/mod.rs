//! Facade shim: `repo` is now the `shamir-engine-repo` crate; this module
//! re-exports its tree (same `shamir_engine::repo::*` paths as before)
//! plus the `instance/` shell items that stay in `shamir-engine`.

pub(crate) use shamir_engine_repo::gate_visibility;
pub use shamir_engine_repo::journal;
pub use shamir_engine_repo::meta;
pub mod seed;

#[cfg(test)]
pub mod durability_tests;
pub mod instance;
#[cfg(test)]
pub mod tests;

// Preserved historical module paths (the modules moved into `instance/`,
// or into `shamir-engine-repo`; every old `crate::repo::<module>` path
// keeps resolving with the same audience through these re-exports).
pub use instance::config as repo_config;
pub use instance::repo_instance;
pub use instance::seed::prepare_seed;
pub use instance::support::catalog::fk_reverse_cache;
pub use instance::version_provider;
pub use shamir_engine_repo::changelog_store;
pub use shamir_engine_repo::durability_policy;
pub use shamir_engine_repo::group_commit;
pub use shamir_engine_repo::repo_types;
pub use shamir_engine_repo::seed::install as seed_install;

pub use crate::repo::instance::seed::install::{SeedImageWriter, SeedTable};
pub use repo_config::RepoConfig;
pub use repo_instance::catalog::RenamePersistOutcome;
pub use repo_instance::catalog::{begin_migration_freeze, begin_migration_freeze_admitted};
pub use repo_instance::RepoInstance;
pub use repo_types::{BoxRepo, BoxRepoFactory, RepoFactory};
pub use seed_install::{SeedInstallState, SeedReceipt};
pub use shamir_engine_repo::changelog_store::write_domain::{
    RepoWriteDomain, RepoWriteError, RepoWritePermit, RepoWriteReservation,
};
pub use shamir_engine_repo::durability_policy::{
    DurabilityPolicy, RetentionOverflow, WalCommitTier,
};
pub use shamir_engine_repo::BoundedJournalRead;
pub(crate) use shamir_engine_repo::DurabilityHandle;
pub use version_provider::repo_token;
pub use version_provider::to_mvcc_retention;

/// Re-export the engine-internal retention policy so downstream crates
/// (e.g. `shamir-db`) can reference it without a direct `shamir-tx` dep.
pub use shamir_tx::Retention as MvccRetention;
