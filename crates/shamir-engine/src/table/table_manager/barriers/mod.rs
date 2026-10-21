//! DDL write-barrier impl blocks on `TableManager` and the RAII
//! `WriteBarrierGuard` (engine sweep T1, W4; split from the tail of
//! `table_manager.rs`).
//!
//! Mounted as a child tree of `table_manager` via `#[path]` from
//! `table_manager.rs`, so every file here can see `TableManager`'s
//! private/`pub(super)` fields.

#[allow(clippy::module_inception)]
mod barriers;
pub(super) mod guard;
