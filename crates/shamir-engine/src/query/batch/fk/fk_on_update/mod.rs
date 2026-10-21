//! Phase ②.2b — `ON UPDATE` referential-action enforcement.
//!
//! Mirrors the Phase D delete-path (`fk_restrict` + `fk_actions`) but fires on
//! the **UPDATE** path: when an `UPDATE` reassigns a parent *referenced*
//! `ref_field` to a new value, dependent child rows are fanned out according
//! to their FK's `on_update` action.
//!
//! ## Trigger condition (the fast no-op gate)
//!
//! Unlike delete (where *any* matched row removal is a trigger), an update
//! only matters if `op.set` actually reassigns one of the parent
//! `ref_field`s that a child FK with `on_update != NoAction` points at.  The
//! vast majority of updates touch ordinary fields, so [`plan_fk_on_update`]
//! **first** collects the set of field keys from `op.set` and intersects them
//! with discovered child `parent_ref_field`s; if the intersection is empty it
//! returns an empty plan immediately — zero scan overhead on the hot path.
//!
//! ## (old → new) pairs
//!
//! For each intersected `ref_field`, `new_value` is the literal scalar that
//! `op.set` assigns to that field.  `old_values` are the current
//! `ref_field` values from the parent rows matched by `op.where_clause`
//! (collected via the shared `collect_parent_values` scan).  Fan-out only
//! happens for `old != new` pairs.
//!
//! ## MVP scope — single level, literal scalar `new`
//!
//! - **Depth:** one level.  A re-keyed child row is *not* itself re-scanned
//!   for grandchildren.  This mirrors the conservative single-field MVP of
//!   Phase D and avoids FK-cycle recursion entirely (a re-key can otherwise
//!   ping-pong forever on a self/mutual FK cycle).  Multi-level update-cascade
//!   is tracked as a follow-up.
//! - **`new_value`:** only a literal scalar (`Int`/`Str`/`Bool`/`F64`/`Null`/
//!   `Bin`/`Dec`/`Big`) is propagated.  If `op.set` assigns the `ref_field` a
//!   computed (`$fn`/`$ref`/`$expr`) or non-scalar (`Map`/`List`/`Set`)
//!   value, the new value is not statically known at plan time, so we
//!   **reject** the update with `fk_update_unsupported_new_value` rather than
//!   silently skipping (least-surprise: the operator explicitly declared a
//!   referential action, so dropping the fan-out would violate it).
//!
//! ## Actions
//!
//! - **Restrict** — if any child row references one of the *old* values being
//!   changed, reject (`fk_restrict`).  Mirrors `check_fk_restrict` but keyed
//!   on the about-to-change old value, not the about-to-delete row.
//! - **Cascade** — for each child row whose FK == old, UPDATE its FK field to
//!   `new_value` (NOT a delete).  New `PendingMutation::UpdateField`.
//! - **SetNull** — for each child row whose FK == old, set the FK field to
//!   `Null` (child field must be nullable; reuses the `field_is_nullable`
//!   check).
//!
//! ## Atomicity / TOCTOU
//!
//! Same as delete: the plan is resolved before the mutation calls, and the
//! child mutations are applied **inside** the same implicit/explicit tx as
//! the parent update so the commit is atomic.  As of F-28 Step 2, the plan's
//! row-level probes (`collect_parent_values`, `any_child_references`) read via
//! `TableManager::list_stream_tx(Some(tx), ..)` — the calling transaction's
//! own snapshot-plus-staged-writes view — instead of committed-only state.
//! This closes the **in-tx** read-your-own-writes gap (D1): an old parent
//! value changed/removed earlier in the SAME transaction, or a child row
//! inserted/updated earlier in the same transaction, is now correctly
//! reflected in the RESTRICT/CASCADE/SET NULL fan-out decision.
//!
//! ## Cross-transaction race — CLOSED for the implicit path (F-28 Step 5, S3-C)
//!
//! The **cross-transaction** race — a genuinely concurrent OTHER
//! transaction's write landing between this plan and the eventual commit —
//! is now closed for the IMPLICIT update path the same way `fk_restrict.rs`
//! closes it for delete: `query_runner.rs`'s implicit UPDATE arm checks
//! `FkReverseCache::is_fk_parent_with_update_action` and opens the implicit tx as
//! `Serializable` when this table is an FK parent with a non-`NoAction`
//! `on_update` action (symmetric with the DELETE arm's upgrade — the
//! `list_stream_tx` child-table scans in `plan_fk_on_update` above are the
//! SAME shape as `fk_restrict.rs`/`fk_actions.rs`'s, so they carry the
//! identical cross-transaction race exposure and get the identical fix,
//! including a bounded internal retry for the common already-resolved-race
//! case). The child-side footprint widening
//! (`require_footprint_if_fk_child`) is unconditional — it fires on every
//! insert/update/set into an FK-child table regardless of which parent-side
//! operation (delete or update) eventually checks against it. See
//! `fk_restrict.rs`'s module doc for the full mechanism description and
//! `crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs`
//! for the deterministic end-to-end proof.
//!
//! Residual scope: same as `fk_restrict.rs`/`fk_actions.rs` — closed for the
//! implicit path; an explicit tx that stays Snapshot is unaffected unless
//! the caller itself opens it Serializable.

mod apply;
mod helpers;
mod plan;
mod scan;

pub(crate) use apply::apply_fk_update_plan;
pub(crate) use plan::plan_fk_on_update;
#[cfg(test)]
pub(crate) use scan::discover_on_update_refs;
