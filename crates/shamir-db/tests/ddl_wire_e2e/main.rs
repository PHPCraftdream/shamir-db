//! End-to-end tests for function, validator, and folder DDL over the wire
//! (`ShamirDb::execute`).
//!
//! Verifies that every new `BatchOp` variant reaches the facade, passes
//! the auth gate, and round-trips through the catalogue.

mod crud;
mod drops;
mod error_codes;
mod folders;
mod helpers;
mod idempotency_cascade;
