//! Facade shim for the moved `repo::seed` tree: `install` and its receipt
//! types live in `shamir-engine-repo`; `prepare_seed` and the image writer
//! stay in `repo::instance::seed`.

pub mod install;

pub use crate::repo::instance::seed::prepare_seed;
