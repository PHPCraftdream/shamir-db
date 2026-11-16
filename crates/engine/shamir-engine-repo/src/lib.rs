//! ShamirDB repo layer: the `journal`, `meta` and `seed` primitives the
//! engine's `RepoInstance` shell composes. Re-exported by the
//! `shamir-engine` facade at `shamir_engine::repo::*`.

pub mod gate_visibility;
pub mod journal;
pub mod meta;
pub mod seed;

// Preserved historical module paths (the modules moved into `journal/`;
// every old `crate::repo::<module>` path keeps resolving with the same
// audience through these re-exports).
pub use journal::backend::durability_policy;
pub use journal::backend::group_commit;
pub use journal::backend::repo_types;
pub(crate) use journal::bounded_journal_read;
pub use journal::changelog_store;
pub use seed::install as seed_install;

pub use changelog_store::write_domain::{
    RepoWriteDomain, RepoWriteError, RepoWritePermit, RepoWriteReservation,
};
pub use durability_policy::{DurabilityPolicy, RetentionOverflow, WalCommitTier};
// Cross-crate seam: the `RepoInstance` shell and the tx commit core use
// these `pub(crate)` handles/policy helpers from `shamir-engine`.
pub use durability_policy::{checkpoint_allocation_floor_for, install_empty_coverage};
pub use journal::backend::durability_handle::DurabilityHandle;
pub use journal::bounded_journal_read::BoundedJournalRead;
pub use repo_types::{BoxRepo, BoxRepoFactory, RepoFactory};
pub use seed_install::{SeedInstallState, SeedReceipt};

/// Re-export the engine-internal retention policy so downstream crates
/// (e.g. `shamir-db`) can reference it without a direct `shamir-tx` dep.
pub use shamir_tx::Retention as MvccRetention;
