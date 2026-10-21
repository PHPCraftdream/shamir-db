//! Operation impl blocks on `TableManager` (engine sweep T1, W4).
//!
//! Mounted as a child tree of `table_manager` via `#[path]` from
//! `table_manager.rs`, so every file here can see `TableManager`'s
//! private/`pub(super)` fields.

pub(in crate::table) mod table_manager_buffer;
pub(in crate::table) mod table_manager_crud;
pub(in crate::table) mod table_manager_index_mgmt;
pub(in crate::table) mod table_manager_replication;
pub(in crate::table) mod table_manager_sorted_index;
