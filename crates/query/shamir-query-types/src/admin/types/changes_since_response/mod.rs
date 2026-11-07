//! Typed `changes_since` response DTOs (VIS-5b). `mod.rs` is a manifest:
//! types live in sibling files.

pub mod event;
pub mod record_change;
pub mod response;
pub(crate) mod u64_wire;

pub use event::ChangesSinceEvent;
pub use record_change::{ChangeOpKind, ChangesSinceRecordChange};
pub use response::ChangesSinceResponse;

#[cfg(test)]
mod tests;
