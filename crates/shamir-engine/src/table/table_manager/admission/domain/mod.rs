//! Parent manifest for admission sub-trees.
//!
//! NOTE (engine sweep T1, chunk W3): NOT yet declared by `admission/mod.rs`;
//! chunk W4 wires `barrier` and `index_admission` here when
//! `table_manager.rs` becomes `table_manager/mod.rs`.

mod barrier;
mod index_admission;
