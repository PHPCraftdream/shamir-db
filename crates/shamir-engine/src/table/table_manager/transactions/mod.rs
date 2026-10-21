//! Transaction staging impl blocks on `TableManager` (engine sweep T1, W4;
//! split from the former `table/table_manager_tx_ops.rs`).
//!
//! Mounted as a child tree of `table_manager` via `#[path]` from
//! `table_manager.rs`, so every file here can see `TableManager`'s
//! private/`pub(super)` fields.

mod tx_delete;
mod tx_insert;
pub(in crate::table) mod tx_support;
mod tx_update;
