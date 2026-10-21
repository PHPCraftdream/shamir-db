//! Admission receipts on `TableManager` (moved from `table/write_receipt/`).
//!
//! NOTE (engine sweep T1, chunk W3): this manifest is mounted at the OLD path
//! `crate::table::write_receipt` via a `#[path]` attribute in
//! `table/mod.rs`, because `table_manager.rs` is still a plain file this
//! chunk. Chunk W4 turns `table_manager.rs` into `table_manager/mod.rs` and
//! re-mounts this tree at `crate::table::table_manager::admission`.
//! The sibling `domain/` and `views/` manifests are intentionally NOT
//! declared here yet: their files contain `impl TableManager` blocks that
//! rely on being CHILD modules of `table_manager` (mounted via `#[path]`
//! from `table_manager.rs`) for private-field access, so they keep their
//! current mounting until W4 re-parents them.

mod crud;
mod outcome;

pub use outcome::WriteOutcome;
