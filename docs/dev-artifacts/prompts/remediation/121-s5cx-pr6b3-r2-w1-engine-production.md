# S5c-X PR6b-3 / R2-W1 — engine production fixes (review round 2)

Worktree: current directory. Parent task: `docs/dev-artifacts/prompts/remediation/121-s5cx-retire-legacy-follower.md`.
Round-1 state is already committed on the branch; you fix review findings in the working tree.

## Hard rules (verbatim)
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git
  command that mutates the working tree or index. Only edit files; the orchestrator commits.
- NO `cargo test` / `build` / `check` / `clippy` / `nextest` or any other compile or test
  command, by you or any child agent. Only `cargo fmt -p shamir-engine` as your LAST step.
- Never silence a lint with `allow`.
- Layout: <=7 entries/folder, <=1000 lines/file; over-limit trees must not grow.
- Imports at top; `mod.rs` re-exports only. No machine-specific absolute paths.

## Fixes (each with a full read before editing)

### 1. Duplicate `FenceRecord` binding (P1, compile blocker)
`crates/shamir-engine/src/meta/repl/bookmark.rs`: lines 18 and 23 both bind
`crate::meta::repl::fence_record::FenceRecord`. DELETE the plain `use` at line 18; KEEP the
`pub use` (it both re-exports the public path `crate::meta::repl_bookmark::FenceRecord` and
brings the type into the module scope used by `save_fence_record_v3_aware`). Re-read the file
afterwards and confirm exactly one binding remains.

### 2. Remove unread `PreparedTable` fields (P1, dead code)
`crates/shamir-engine/src/tx/apply_replicated.rs`: struct `PreparedTable` (line ~108) keeps
only `name` and `ops`. DELETE `token`, `mvcc`, `counter` and their population at lines
~262-267. The `per_table_mvcc().read_sync` probe MUST STAY (it produces the `NoMvccStore`
rejection) but as a boolean check, not a stored clone; the `let token = ...` local becomes
unused — remove it. Then `rg -n '\.(token|mvcc|counter)\b'` over the receipt modules
(`tx/follower_durability/`) and over every other reference to `PreparedTable` in the crate to
PROVE no surviving consumer reads the removed fields (`receipt/stage.rs` and `receipt/publish.rs`
use `table.name` / `table.ops` only — verify, do not assume).

### 3. `admit_follower`: capability gate for EVERY admission (P1, acceptance test 8)
`crates/shamir-engine/src/tx/follower_durability/receipt/resume/activation.rs`: move the
`repo.storage_supports_atomic_batch()` check to the TOP of `admit_follower`, before
classification, so BOTH `Native` and `Fresh` refuse with `DbError::AtomicBatchUnsupported`
(same message text) before any translation, interner first-touch or allocation. `Fresh` keeps
its `SeedInstallState` conflict check; `FloorWithoutBookmark` keeps the typed refusal.

### 4. Gate initialization derives follower visibility from durable v3 (P1, provenance)
`crates/shamir-engine/src/repo/repo_instance.rs` `tx_gate()` init (~859-893): after the gate
is constructed, classify the bookmark:
```rust
// PR6b-3: durable follower provenance is live from open. A valid v3 record
// enables receipt-native visibility before any read; a legacy or malformed
// record refuses gate initialization (fail closed — operator reseeds).
match crate::meta::repl::bookmark_v3::classify_follower_bookmark(&info_store).await? {
    crate::meta::repl::bookmark_v3::FollowerBookmarkState::Native(_) => {
        gate.enable_receipt_native_visibility();
    }
    _ => {}
}
```
The `?` deliberately propagates the typed refusal (this is the engine-side fail-closed).
Note in your report which existing tests this ordering touches (e.g. corrupt-marker tests
still fail at the earlier marker decode) and confirm with a full read that none of them
seed a LEGACY bookmark record and then expect `tx_gate()` to succeed.

### 5. Classification BEFORE WAL replay at open (P1, cold ordering)
`crates/shamir-engine/src/repo/durability_policy/durability_policy/repo_impl.rs`:
- `from_factory_with_durability` (~42-67): immediately AFTER `instance.set_durability(policy)?`
  and BEFORE `write_domain().admit()`:
```rust
// PR6b-3: refuse unsupported durable follower state BEFORE any WAL replay,
// floor persistence or journal publication.
let info = instance.tx_info_store().await?;
crate::meta::repl::bookmark_v3::classify_follower_bookmark(&info).await?;
```
- Apply the same three lines in `from_factory_with_test_segment_bytes` (cfg(test), ~12-39)
  after its `set_durability` call, before recovery.
With a full read, confirm these are the ONLY two open paths that reach
`recover_v2_inflight_admitted` (rg for its callers) — if you find another, fix it too and
report it.

### 6. Public receipt-pointer integrity helper (P1 prerequisite for the server fix)
`crates/shamir-engine/src/meta/repl/bookmark_v3.rs`: add
```rust
/// True when `v3`'s outstanding-receipt pointer (if any) resolves to a stored
/// receipt that is not `Checkpointed`. A dangling or retired pointer is a
/// corrupt durable state: admission-time resume must refuse it.
pub async fn receipt_pointer_resolves(info: &Arc<dyn Store>, v3: &BookmarkV3) -> DbResult<bool> {
    let Some(id) = v3.outstanding_receipt else { return Ok(true) };
    match crate::meta::repl::receipt::load_outstanding_receipt(info, &id).await? {
        None => Ok(false),
        Some(receipt) => Ok(receipt.phase != crate::meta::repl::ReceiptPhase::Checkpointed),
    }
}
```
Verify `load_outstanding_receipt` and `ReceiptPhase` are reachable from this module with a
full read of `meta/repl/receipt.rs` and `meta/repl/mod.rs`.

### 7. Dead-code / stale-allow sweep (engine only)
For EACH item below, rg for callers across the workspace; then either REMOVE the stale
`#[allow(dead_code)]` attribute (item has callers) or DELETE the item (zero callers):
- `meta/repl/receipt.rs`: `put_receipt` (~267), `delete_receipt` (~294) — likely dead now;
  `load_outstanding_receipt` (~280) — now used by bookmark_v3 + resume.
- `repo/changelog_store/follower_durability.rs`: the three `#[allow(dead_code)]` attrs
  (items are used by activation/stage/tests).
- `tx/follower_durability/receipt/apply.rs:46`: `#[cfg_attr(not(test), allow(dead_code))]`
  on `apply_batch_receipt` — production caller exists (`apply_replicated_batch`); remove.
- `receipt/resume/worker.rs`: `receipt_worker_parked_guards` / `receipt_worker_wait_idle`
  cfg(test) fns — keep+un-allow iff tests call them, else delete.
- `receipt/stage.rs` cfg(test) seam items and `receipt/checkpoint.rs` park helpers — verify
  each still has a test caller; delete truly orphaned ones.
NEVER add a new `allow`. List every removal with file:line in your report.

## Last step
`cargo fmt -p shamir-engine` (report the diff summary). NO other cargo command.

## Report
Per fix: file:line before/after; the sweep table (item -> callers found -> action); the
gate-init/test-interaction note from fix 4; confirmation that `from_factory_*` are the only
recovery entry points; files you touched now over 900 lines; deviations.

## ADDENDUM — CI compiler output on round 1 (fix these exact errors)
CI (fast tier) failed `shamir-engine` (lib) with 5 errors; fix every one and sweep the same
classes in every other touched crate:
- `E0252` duplicate `FenceRecord` at bookmark.rs:23 — covered by fix 1 above.
- `E0603` "struct import `FenceRecord` is private" for `crate::meta::repl::bookmark::FenceRecord`
  used at `tx/follower_durability/receipt/apply.rs:38` and `receipt/checkpoint.rs:21` —
  resolved by fix 1 (the single remaining `pub use` makes that path public). Do NOT change
  those two call sites.
- `E0603` "module `seed_install` is private": `crates/shamir-engine/src/repo/mod.rs:13`
  declares `mod seed_install;` while `tx/follower_durability/receipt/resume/activation.rs:38`
  (and the activation tests via `crate::repo::seed_install::{SeedInstallState, SeedReceipt}`)
  use the full module path. FIX: change it to `pub mod seed_install;` in `repo/mod.rs`
  (`seed_install/mod.rs` already `pub use`s `SeedInstallState`/`SeedReceipt`). With a full
  read of `repo/mod.rs`, check whether a duplicate re-export of the same names already
  exists (would itself become an ambiguity) and keep exactly one public path. Do not
  rewrite the call sites.
- `unused import` at bookmark.rs:23 — resolved by fix 1.
After your edits, re-read bookmark.rs, activation.rs, repo/mod.rs and the two FenceRecord
consumers and confirm: exactly ONE `FenceRecord` binding per module, every import path
resolves through a PUBLIC item, zero new `allow` attributes.
