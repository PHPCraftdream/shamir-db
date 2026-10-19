//! `conservation` — source inventory diff between two revisions
//! (CI-03a chunk B).
//!
//! Re-exports only; logic lives in the sibling files.

pub mod delta;
pub mod inventory;
pub mod item;
pub mod matching;
pub mod run;

pub use run::{run, ConservationArgs};
