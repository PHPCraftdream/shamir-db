//! DDL-barrier admission impl blocks (`impl TableManager`).
//!
//! NOTE (engine sweep T1, chunk W3): NOT yet declared by any parent manifest.
//! These files must stay CHILD modules of `table_manager` for private-field
//! access; while `table_manager.rs` is still a plain file (chunk W3) they are
//! mounted via `#[path]` from `table_manager.rs`. Chunk W4 will declare this
//! module here once `table_manager/mod.rs` exists.

mod admitted_ops;
mod write_domain_barrier;
mod write_domain_rename_admitted;
