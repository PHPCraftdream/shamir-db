//! Validator pieces bound to engine storage; stay in shamir-engine at the crate split.

pub mod persistence;
pub mod validator_db;

#[cfg(test)]
mod tests;
