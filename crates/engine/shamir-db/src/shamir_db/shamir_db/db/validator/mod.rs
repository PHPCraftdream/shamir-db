//! Validator lifecycle, binding and authoritative binding-holder
//! derivation. `mod.rs` re-exports only; one primary item per file.

pub(in crate::shamir_db) mod migration_admission;
pub(in crate::shamir_db) use validator_management::{
    ValidatorRegistrationError, ValidatorTargetAdmission,
};

mod binding_scan;
mod validator_drop;
mod validator_management;
mod validator_rename;
