//! Accessor-surface impl blocks on `TableManager` (engine sweep T1, W4).
//!
//! Declared as a child tree of `table_manager` in `table_manager.rs`, so
//! every file here can see `TableManager`'s private/`pub(super)` fields.

mod table_accessors;
pub(crate) mod table_manager_changefeed;
pub(crate) mod table_manager_locks;
pub(crate) mod table_manager_streaming;
pub(crate) mod table_manager_validators;
pub(super) mod tokenizer;
