//! Phase D.2 — `ON DELETE CASCADE` + `ON DELETE SET NULL`.
//!
//! Extends the Phase D.1 reverse-FK discovery + gate machinery to the
//! remaining referential actions.  Where D.1 (`RESTRICT`) only *rejects* the
//! parent delete, D.2 *acts* on the child rows:
//!
//! - **Cascade** — for each child row whose FK field equals a parent value
//!   being deleted, DELETE the child row.  Recurses: a cascaded child may
//!   itself be a parent of grandchildren.  A depth guard prevents infinite
//!   loops on FK cycles (returns `fk_cascade_depth`).
//! - **SetNull** — for each child row, UPDATE it setting the FK field to
//!   `Null`.  The child field must be nullable; this is checked at action
//!   time (`set_null_requires_nullable`) because the engine-mirror schema
//!   lives on the `TableManager` and is not reachable from the DDL bind path.
//!
//! ## Atomicity
//!
//! The cascade/setnull mutations run **inside the same implicit/tx batch** as
//! the parent delete so they commit atomically.  The full cascade tree
//! (including grandchild discovery) is now resolved **against the calling
//! transaction's own snapshot-plus-staged-writes view** (F-28 Step 2), via
//! `TableManager::list_stream_tx(Some(tx), ..)` and
//! `read_one_tx_bytes(id, Some(tx))` — planning still happens before the
//! actual mutation calls (`delete_tx` / `update_tx_bytes`) but now reads
//! read-your-own-writes state rather than committed-only state.
//!
//! ## TOCTOU caveat
//!
//! F-28 Step 2 closes the **in-tx** read-your-own-writes gap (D1): a child
//! row inserted earlier in the SAME transaction (before the parent delete
//! that triggers this cascade) is now visible to `collect_parent_values` /
//! `discover_action_refs`'s row-level scans, so it is correctly included in
//! the cascade/setnull plan instead of silently orphaned. A child row staged
//! as an UPDATE re-keying its FK away from the parent is likewise reflected
//! via the tx overlay, not the stale committed value.
//!
//! ## Cross-transaction race — CLOSED (F-28 Step 5, S3-C)
//!
//! The **cross-transaction** race — a genuinely concurrent OTHER
//! transaction inserting a new child reference between this plan and the
//! eventual commit — is now closed the same way `fk_restrict.rs` closes it:
//! the parent-side implicit-delete isolation upgrade
//! (`query_runner.rs`'s `implicit_tx_isolation_for_fk_parent`, gated on
//! `FkReverseCache::is_fk_parent_with_delete_action`) plus the child-side footprint
//! widening (`require_footprint_if_fk_child`, gated on
//! `FkReverseCache::is_fk_child`) — see `fk_restrict.rs`'s module doc for
//! the full mechanism description and
//! `crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs`
//! for the deterministic end-to-end proof (a CASCADE-shaped race is
//! action-agnostic to this mechanism — it protects any Serializable scan of
//! the child table regardless of what the caller does with the result).
//!
//! Residual scope: same as `fk_restrict.rs` — closed for the **implicit**
//! delete path; an explicit tx that stays Snapshot is unaffected unless the
//! caller itself opens it Serializable.

mod apply;
mod helpers;
mod plan;

pub(crate) use apply::apply_cascade_plan;
pub(crate) use plan::plan_cascade;
