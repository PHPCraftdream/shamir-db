pub mod instance;
pub mod journal;
pub mod seed;

#[cfg(test)]
pub mod tests;

// Preserved historical module paths (the modules moved into `instance/`,
// `journal/` and `seed/`; every old `crate::repo::<module>` path keeps
// resolving with the same audience through these re-exports).
pub use instance::config as repo_config;
pub(crate) use instance::repo_instance::catalog::visibility_impl as visibility_observer;
pub use instance::support::catalog::fk_reverse_cache;
pub use instance::version_provider;
pub use journal::backend::durability_policy;
pub use journal::backend::group_commit;
pub use journal::backend::repo_types;
pub(in crate::repo) use journal::bounded_journal_read;
pub use journal::changelog_store;
pub use seed::install as seed_install;
pub use seed::prepare_seed;

pub use changelog_store::write_domain::{
    RepoWriteDomain, RepoWriteError, RepoWritePermit, RepoWriteReservation,
};
pub use durability_policy::{DurabilityPolicy, RetentionOverflow, WalCommitTier};
pub use fk_reverse_cache::{
    build_reverse_fk_entries, FkReverseCache, ReverseFkEntry, TaggedReverseFkEntry,
};
pub use instance::repo_instance;
pub use journal::bounded_journal_read::BoundedJournalRead;
pub use repo_config::RepoConfig;
pub use repo_instance::RepoInstance;
pub use repo_types::{BoxRepo, BoxRepoFactory, RepoFactory};
pub use seed_install::{SeedImageWriter, SeedInstallState, SeedReceipt, SeedTable};
pub use version_provider::repo_token;
pub use version_provider::to_mvcc_retention;
pub use version_provider::RepoVersionProvider;

/// Re-export the engine-internal retention policy so downstream crates
/// (e.g. `shamir-db`) can reference it without a direct `shamir-tx` dep.
pub use shamir_tx::Retention as MvccRetention;
