//! `structure` — AST rules for changed/moved areas (CI-03a chunk B).
//!
//! Re-exports only; logic lives in the sibling files.

pub mod diff;
pub mod owned;
pub mod rules;
pub mod select;
pub mod vacated;

pub use select::{run, StructureArgs};
