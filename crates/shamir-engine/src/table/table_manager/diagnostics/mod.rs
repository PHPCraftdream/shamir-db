//! Diagnostics impl blocks on `TableManager` (engine sweep T1, W4; moved
//! from the former `table/doctor.rs` and `table/degraded_index_count.rs`).
//!
//! Mounted as a child tree of `table_manager` via `#[path]` from
//! `table_manager.rs`, so every file here can see `TableManager`'s
//! private/`pub(super)` fields.

pub mod degraded_index_count;
pub mod doctor;
