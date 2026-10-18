//! `modules` — module-reachability gate (CI-03a chunk A).
//!
//! Re-exports only; logic lives in the sibling files.

pub mod json;
pub mod resolution;
pub mod roots;
pub mod walk;

pub use walk::has_excluded_dir;
pub use walk::run;
