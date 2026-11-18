//! Application-schema migration admin handlers (release-1 R1c step 2b).
//!
//! Re-export/declaration-only module map (rust-guard: no logic in `mod.rs`).
//! Wired from `execute/mod.rs` via `#[path]` as `admin_db_schema_migration`:
//!
//! - `schema_migration` — the `ApplicationSchema` dispatcher arm: status,
//!   plan and apply admission entry.
//! - `plan` — the side-effect-free plan arm (step 2b-2) and the admission
//!   helpers shared with apply.
//! - `apply` — the journaled, fenced apply arm's ADMISSION entry (step
//!   2b-3a): the entry point and its admission helpers.
//! - `apply_operations` — the fenced operation runner: per-operation DDL
//!   execution and the exact postconditions (§4.1–§4.2).
//! - `apply_journal` — journal append under the fence, error mapping and
//!   the `applied` result wire encoding.
//! - `fence` — the in-process per-database maintenance fence (§4.3) and the
//!   synchronous batch-entry refusal helper.

mod apply;
pub(in crate::shamir_db) mod apply_journal;
mod apply_operations;
pub(in crate::shamir_db::execute) mod fence;
mod plan;
mod schema_migration;

pub(in crate::shamir_db::execute) use fence::maintenance_fence_refusal;
