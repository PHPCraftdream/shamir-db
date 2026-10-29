//! Validator facade (#1548 S2): portable validators live in
//! `shamir-engine-validator`; storage-bound pieces stay in `engine_bound`.

mod engine_bound;

#[doc(no_inline)]
pub use shamir_engine_validator::validator::*;

pub use engine_bound::validator_db::ValidatorDb;
pub use engine_bound::{persistence, validator_db};
