//! Accessor-surface impl blocks on `TableManager` (engine sweep T1, W4).
//!
//! Mounted as a child tree of `table_manager` via `#[path]` from
//! `table_manager.rs`, so every file here can see `TableManager`'s
//! private/`pub(super)` fields.

#[allow(clippy::module_inception)]
mod accessors;
pub(in crate::table) mod table_manager_changefeed;
pub(in crate::table) mod table_manager_locks;
pub(in crate::table) mod table_manager_streaming;
pub(in crate::table) mod table_manager_validators;
pub(super) mod tokenizer;
