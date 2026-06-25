//! End-to-end tests for function, validator, and folder DDL over the wire
//! (`ShamirDb::execute`).
//!
//! Verifies that every new `BatchOp` variant reaches the facade, passes
//! the auth gate, and round-trips through the catalogue.

mod helpers;

mod drop_function_guard;
mod error_codes;
mod folders_introspection;
mod idempotency_cascade;
mod lifecycle;
mod ownership;
mod rename_folder;
mod serde_roundtrip;
