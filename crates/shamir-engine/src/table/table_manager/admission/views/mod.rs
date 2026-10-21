//! Read-only accessor views over `TableManager`.
//!
//! NOTE (engine sweep T1, chunk W3): NOT yet declared by `admission/mod.rs`;
//! chunk W4 wires this when `table_manager.rs` becomes `table_manager/mod.rs`
//! (the file must stay a CHILD module of `table_manager` for
//! private-field access until then).

mod read_only_views;
