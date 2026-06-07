pub mod changelog_store;
pub mod group_commit;
pub mod repo_config;
pub mod repo_instance;
pub mod repo_types;
pub mod version_provider;

#[cfg(test)]
pub mod tests;

pub use repo_config::RepoConfig;
pub use repo_instance::{repo_token, to_mvcc_retention, RepoInstance};
pub use repo_types::{BoxRepo, BoxRepoFactory, RepoFactory};
pub use version_provider::RepoVersionProvider;

/// Re-export the engine-internal retention policy so downstream crates
/// (e.g. `shamir-db`) can reference it without a direct `shamir-tx` dep.
pub use shamir_tx::Retention as MvccRetention;
