//! Facade shim: receipt/state types come from `shamir-engine-repo`; the
//! image writer and table seed struct stay in `repo::instance::seed`.

pub use crate::repo::instance::seed::install::{SeedImageWriter, SeedTable};
pub use shamir_engine_repo::seed::install::{translate_row, SeedInstallState, SeedReceipt};
