# S5c-X PR6b-3 / W2 — producers write v3; durable provenance; admission & server mapping

Task 121 brief: `docs/dev-artifacts/prompts/remediation/121-s5cx-retire-legacy-follower.md`
(read it first). Parent of this stage: W1 already landed the typed refusal error
(`DbError::FollowerStateRefused { reason: FollowerRefusalReason }` in `shamir-storage`,
reasons `LegacyBookmarkFormat | MalformedBookmark | LegacyFollowerState`), the strict v3
loader `load_bookmark_v3`, the classifier
`crate::meta::repl::bookmark_v3::{classify_follower_bookmark, FollowerBookmarkState}`
(`Native(BookmarkV3) | Fresh | FloorWithoutBookmark { floor }`), and the v3-only
`save_fence_record_v3_aware` in `crates/shamir-engine/src/meta/repl/bookmark.rs`. READ those
files before editing. `has_v3_record` and the legacy writers no longer exist.

## Hard rules
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git
  command that mutates the working tree or index. Only edit files; the orchestrator commits.
- NO `cargo test` / `build` / `check` / `clippy` / `nextest` or any other compile or test
  command. Only `cargo fmt -p <crate>` for touched crates as your LAST step.
- Never silence a lint with `#[allow]` — remove dead code instead (`-D warnings` in CI).
- ≤7 entries per folder, ≤1000 lines per file; files/folders already over a limit must not
  grow. All `use` at the top. `mod.rs` re-exports only. Tests in `tests/` folders wired into
  manifests. No machine-specific absolute paths. No new heavy awaits on deep inline chains
  (heap-pin if you add one). Every `impl Trait` fn returns one concrete type on every branch.

## Deliverables

### 0. Close a W1 gap first (small, same crate)
In `crates/shamir-engine/src/meta/repl/bookmark_v3.rs`, `read_strict_record` returns
`decode_bookmark_v3`'s raw `DbError::Storage` when the bytes SNIFF as v3 but fail the strict
decode (truncated at >=16 bytes, trailing bytes, fence/bookmark mismatch). The design's
state table requires a typed format/corruption error there: map a failed decode of
v3-sniffed bytes to `DbError::FollowerStateRefused { reason: MalformedBookmark }`. Keep
`decode_bookmark_v3` itself unchanged (its Storage errors are internal invariants); only
`read_strict_record` (and thus the projections, the classifier, `load_bookmark_v3` and
`save_fence_record_v3_aware`) must surface the typed refusal. Update
`meta/repl/tests/bookmark_refusal_tests.rs` with one truncated-v3 fixture asserting the
typed reason.

### 1. Cold open: classification BEFORE every early-return branch
`crates/shamir-engine/src/tx/cold_reconcile/algorithm.rs` (`reconcile_with_hook`):
- Move `let info = repo.tx_info_store().await?;` to the TOP, before the
  `if !repo.has_persistent_wal()` branch.
- Immediately after it: `let follower_state = classify_follower_bookmark(&info).await?;` —
  a legacy/malformed record returns the typed refusal and FAILS THE OPEN (this is the
  "refuse at open" row of the state table; it now also holds across the non-persistent-WAL
  and `h <= j` early-return branches, because classification ran first).
- `let follower_provenance = matches!(follower_state, FollowerBookmarkState::Native(_));`
  and when provenance holds: `repo.tx_gate().await?.enable_receipt_native_visibility();`
  (durable role state — a valid v3 record, bookmark zero and fence-less included — is the
  ONLY thing that identifies a follower; `LastCommittedVersion`, `RepoIdentity`, WAL
  capability and subscriptions must not).
- DELETE the later `let bookmark = load_replication_bookmark(..)` /
  `let follower_fence = load_fence_record(..)` /
  `follower_provenance = bookmark > 0 || follower_fence.is_some()` block (lines ~91-93) —
  the earlier classification replaces it. Keep the `h <= j` early return where it is
  (AFTER `complete_outstanding_receipt`).
- Keep everything else (WAL classification, eventless handling, gap handling, journal
  repair, `reserve_recovery_floor`) byte-for-byte in behavior. "Legacy WAL evidence"
  (`classification.rs`) is NOT a legacy follower bookmark — do not touch it.
`crates/shamir-engine/src/tx/cold_reconcile/receipt_recovery.rs`:
- DELETE the `has_v3_record` block (lines ~42-48) — visibility is now enabled by the
  caller's classification before this runs. Keep the strict `load_bookmark_v3` read
  (absent → `Ok(())`); legacy records can no longer reach here (open refused earlier).
- Update the module doc accordingly. Keep `follower.rs` (`follower_certified_through`) as
  is — it reads `clean_through` through the strict v3 projection, which is exactly the
  design's "read J from strict v3".

### 2. Seed installation writes a v3 baseline
`crates/shamir-engine/src/repo/seed_install/writer.rs` `finish()` (~line 305-310): replace
`save_replication_bookmark(bookmark_info, self.receipt.source_version)` with
`crate::meta::repl::bookmark_v3::save_bookmark_v3(bookmark_info, &BookmarkV3 { fence: None,
bookmark: self.receipt.source_version, clean_through: Some(1), outstanding_receipt: None })`.
Semantics (design §3 "Mandatory seed-install conversion"): upstream bookmark = seed source
version; local clean horizon = the certified local baseline (currently 1 — matches the
`feed.record_clean_horizon(1)` call just above); no outstanding receipt; fence stays absent
and is adopted later by `save_fence_record_v3_aware` WITHOUT losing this baseline (that
preservation is W1's in-place update). Keep the boundary(5) seam position and the
`Installing → Complete` publication order exactly. Update imports and the doc comment on
`finish` (new-format installation, not migration). Check
`crates/shamir-engine/src/repo/seed_install/receipt.rs` `validate_progress` still compiles
against the v3 projection (`load_replication_bookmark`) — it does; don't change it.

### 3. Follower admission replaces the activation selector
`crates/shamir-engine/src/tx/follower_durability/receipt/resume/activation.rs` — rewrite:
- DELETE the `RECEIPT_LOOKUP_DISABLED` seam and `disable_receipt_lookup` (the forced-legacy
  negative control is retired), and DELETE `receipt_path_selected`. There is no successful
  "choose legacy" result anymore — selection becomes validation/admission.
- ADD:
```rust
pub(crate) enum FollowerAdmission { Native, Fresh }
/// Validate durable follower state at first follower admission. Typed
/// refusals propagate; `Fresh` means the caller must adopt (write the v3
/// baseline) before any apply work.
pub(crate) async fn admit_follower(repo: &RepoInstance, info: &Arc<dyn Store>) -> DbResult<FollowerAdmission>
```
  Logic: `classify_follower_bookmark` → `Native` → `Ok(Native)`; `Fresh` → refuse when
  `crate::repo::seed_install::SeedInstallState::load(info).await?.is_some()` (conflicting
  installation state — reason `LegacyFollowerState`); refuse when
  `!repo.storage_supports_atomic_batch()` with `DbError::AtomicBatchUnsupported` (keep the
  existing message text); else `Ok(Fresh)`. `FloorWithoutBookmark` → `Err(
  DbError::FollowerStateRefused { reason: LegacyFollowerState })` (reseed required; this is
  the legacy floor-without-bookmark evidence). Legacy/malformed classification errors
  propagate untouched.
- Update the module doc: receipt-only contract, refusal semantics, operator reseeds.

### 4. Pipeline wiring (durable provenance before service)
`crates/shamir-engine/src/tx/follower_durability/receipt/apply.rs` in `run_receipt_pipeline`:
- Keep the existing permit/paused-reservation admission and the marker-mutex acquisition
  EXACTLY where they are. Immediately AFTER the marker mutex (`let _marker = ...`), run
  `match resume::activation::admit_follower(repo, &info).await? { Native => {} Fresh => {
  save_bookmark_v3(&info, &BookmarkV3 { fence: None, bookmark: 0, clean_through: None,
  outstanding_receipt: None }).await?; } }` — the fresh ADOPTION writes the durable v3
  baseline (bookmark 0, no fence, no J, no receipt) before any version is allocated, any
  table resolved, or any receipt staged. Log the adoption at info level.
- THEN (replacing the unconditional entry call at the top) call
  `gate.enable_receipt_native_visibility();` — moved below admission so the flag is derived
  from durable role state (Native record, or the baseline this pipeline just wrote), never
  enabled "opportunistically". Keep the `requires_reopen` check right after.
- `apply_fresh` (~line 303-317): DELETE the `None =>` legacy-synthesis branch of the
  `load_bookmark_v3` match — after strict loading and mandatory adoption, `None` is a
  contradiction: return
  `Err(DbError::RepoRequiresReopen("replication bookmark v3 baseline vanished".into()))`.
  The `load_replication_bookmark(info)` watermark read above it stays (v3 projection).
- Update `run_receipt_pipeline`'s doc comment: admission → adoption → visibility order.
`crates/shamir-engine/src/tx/apply_replicated.rs` `apply_replicated_batch`: drop the
`receipt_path_selected` routing — the entry is now unconditionally the receipt pipeline:
```rust
pub async fn apply_replicated_batch(repo, events, bookmark, policy) -> DbResult<ApplyOutcome> {
    let info = repo.tx_info_store().await?;
    apply_batch_receipt(repo, info, events.to_vec(), bookmark, policy).await
}
```
  Fix its imports (`receipt_path_selected` gone). The legacy functions below it
  (`apply_replicated_batch_legacy`, `apply_batch_with_info`, `apply_prepared`,
  `apply_replicated`, `BatchDurabilityGuard`, `reserve_floor*`) are now dead — DO NOT delete
  them in this stage (W3 owns the deletion); just ensure YOUR edits compile-textually and
  don't reference deleted names.
`crates/shamir-engine/src/tx/follower_durability/receipt/mod.rs`: update re-exports
(`receipt_path_selected` gone; nothing new needs re-export beyond what siblings use).

### 5. Server: machine-readable terminal classification end to end
`crates/shamir-server/src/replication/error.rs`: add a TERMINAL variant
```rust
#[error("follower state refused ({reason:?}); no migration exists — reseed required")]
FollowerStateRefused { reason: shamir_db::storage::error::FollowerRefusalReason },
```
`crates/shamir-server/src/replication/identity_fence/checks.rs`: add
`pub(crate) fn storage_error(error: shamir_db::storage::error::DbError) -> ReplError`
mapping `DbError::FollowerStateRefused { reason }` → `ReplError::FollowerStateRefused {
reason }`, everything else → `ReplError::Bookmark(other.to_string())`; replace ALL
`map_err(|e| ReplError::Bookmark(e.to_string()))` sites in this file with `map_err(storage_error)`.
The identity startup barrier must not flatten the typed refusal into a retryable string
(design §2).
`crates/shamir-server/src/replication/follower_loop.rs`:
- Bookmark read (~line 355): on `Err(e)` where `matches!(e, shamir_db::storage::error::DbError::FollowerStateRefused { .. })` → return
  `Err(ReplError::FollowerStateRefused { reason })` (terminal). Other errors keep the
  transient backoff path.
- Batch-apply arm (~line 541): check the SAME match BEFORE the `requires_reopen` probe and
  return the terminal typed error; everything else unchanged.
- Refresh the stale PR6b-2 comment above the apply call (~line 523): receipts are the only
  path; unsupported durable states are refused.
`crates/shamir-server/src/replication/supervisor.rs`: add
`ReplError::FollowerStateRefused { .. }` to the terminal quarantine family (the
`IdentityMismatch`/`Seed` arm): `mark_subscription_terminal(&sub_name, "resync_required",
&e.to_string())` + the `shamir_repl_resync_required_total` metric, WITHOUT the
`SKIP_FENCE_QUARANTINE_FOR_TEST` seam (that seam is identity-fence-specific). Keep the edit
minimal — a parallel PR (PR8b-2) owns the catalogue/quarantine-cache internals.
Also in the supervisor's Resume path (~line 343-364, after the `requires_reopen` check):
load the repo's `tx_info_store` and run
`shamir_db::engine::meta::repl::bookmark_v3::classify_follower_bookmark(&info).await` — on
`Err` of the refused kind, `mark_subscription_terminal("resync_required", ...)` and return
`Err(ReplError::FollowerStateRefused { reason })` so a refused repo can never be resumed.

### 6. Reference sweep (report, don't fix out of scope)
`rg` for `receipt_path_selected`, `disable_receipt_lookup`, `has_v3_record`,
`save_replication_bookmark` — everything left is W3/W4/W5 residue (engine legacy apply fns,
batch.rs, advance API, tests). List each with file:line.

## Last step
`cargo fmt -p shamir-engine -p shamir-server` (only your touched crates).

## Report back
Files changed; the exact admission/adoption ordering you implemented; the supervisor/resume
refusal wiring; residue list (file:line); files over 900 lines you touched; deviations.
