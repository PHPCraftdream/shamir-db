//! Admission receipts on `TableManager` (moved from `table/write_receipt/`).
//!
//! The files under `domain/` and `views/` contain `impl TableManager` blocks
//! that need private-field access, so they are mounted as children of
//! `table_manager` through `#[path]` attributes in `table_manager.rs` (this
//! same file is also mounted as `table::write_receipt`) and have no manifests
//! of their own.

mod crud;
mod outcome;

pub use outcome::WriteOutcome;
