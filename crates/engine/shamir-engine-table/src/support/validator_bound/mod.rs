//! Validator pieces bound to engine/table storage in the table crate and
//! re-exported by the engine validator facade.

pub mod persistence;
pub mod validator_db;
pub use validator_db::{BatchFkCache, FkCacheEntry, ValidatorDb, ValidatorFkCtx};

#[cfg(test)]
mod tests;
