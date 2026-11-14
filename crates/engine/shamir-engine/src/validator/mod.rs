//! Validator facade (#1548 S2): portable validators live in
//! `shamir-engine-validator`; storage-bound pieces live in the table
//! subtree (`validator_bound`) and are re-exported here.

#[doc(no_inline)]
pub use shamir_engine_validator::validator::*;

pub use crate::table::validator_bound::validator_db::{
    BatchFkCache, FkCacheEntry, ValidatorDb, ValidatorFkCtx,
};
pub use crate::table::validator_bound::{persistence, validator_db};
