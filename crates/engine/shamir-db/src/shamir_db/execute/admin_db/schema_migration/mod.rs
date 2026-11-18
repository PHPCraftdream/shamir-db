//! Application-schema migration admin handlers (release-1 R1c step 2b).
//!
//! Re-export/declaration-only module map (rust-guard: no logic in `mod.rs`).
//! Wired from `execute/mod.rs` via `#[path]` as `admin_db_schema_migration`:
//!
//! - `schema_migration` — the `ApplicationSchema` dispatcher arm: status,
//!   plan and apply admission entry.
//! - `plan` — the side-effect-free plan arm (step 2b-2) and the admission
//!   helpers shared with apply.
//! - `apply` — admission, fenced operation execution and postconditions,
//!   journal append, error mapping and result encoding.
//! - `fence` — the in-process per-database maintenance fence (§4.3) and the
//!   synchronous batch-entry refusal helper.

mod apply;
pub(in crate::shamir_db::execute) mod fence;
mod function_family;
mod plan;
mod schema_migration;

#[cfg(test)]
pub(in crate::shamir_db) use apply::apply_journal;
pub(in crate::shamir_db::execute) use fence::maintenance_fence_refusal;

#[cfg(test)]
pub(in crate::shamir_db) use function_family::{test_capture_intent, test_verify};
