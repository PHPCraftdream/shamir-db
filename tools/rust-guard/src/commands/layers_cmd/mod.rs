//! `layers` — module-layering gate (no new backward dependencies).
//!
//! Re-exports only; logic lives in the sibling files.

pub(crate) mod config;
pub(crate) mod graph;
pub(crate) mod refs;
pub(crate) mod run;

pub(crate) use run::{run, LayersArgs};
