//! Re-exports only (CLAUDE.md §module-layout) — logic lives in topical
//! subfolders.

mod basic;
mod ids;
pub use basic::base;
pub use basic::common;
pub use basic::numeric_key;
pub use ids::record_id;
pub use ids::repo_record;
pub mod value;
pub mod value_error;

#[cfg(test)]
mod tests;
