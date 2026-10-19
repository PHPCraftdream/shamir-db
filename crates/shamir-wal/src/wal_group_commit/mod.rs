//! Two-tier group-commit coordinator — see [`WalGroupCommit`].

#[path = "wal_group_commit.rs"]
mod group_commit;
mod history_coverage;
mod recoverable_through;
mod wal_coverage;

pub use group_commit::{WalDurability, WalGroupCommit};
pub use recoverable_through::SyncCoordinator;
pub use wal_coverage::WalCoverage;
