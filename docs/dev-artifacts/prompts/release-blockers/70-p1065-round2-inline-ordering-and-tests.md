# Brief 70 — #1065 round 2: fix the inline path's write order (real fix, not just logging) + write the required tests

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## What round 1 got right — do not touch

- `InProgress` write before mutation (Defect 1) — correctly added at both
  DROP and RENAME dispatch sites in `admin_table_index.rs`.
- `eprintln!` → `log::error!` (Defect 3) — done.
- Client-supplied `request_id` correlation id (Defect 4) — correctly
  threaded through `DropIndexOp`/`RenameIndexOp` (`shamir-query-types`) and
  their builders (`shamir-query-builder`), additive/backward-compatible
  (`#[serde(default, skip_serializing_if = "Option::is_none")]`, matching
  the repo's established convention). `op.request_id.unwrap_or_else(RecordId::new)`
  at the dispatch sites is correct.
- Idempotent retry check (reads `ddl_op_log::read_op_status` before
  mutating, short-circuits if a record already exists) — present at both
  sites.
- Versioned envelope (`ddl_op_log.rs`, a 1-byte version prefix, rejects
  unrecognized versions on read) — correctly implemented.
- **The RECOVERY paths' write-order fix (Defect 2, for `SucceededViaCrashRecovery`)
  is CORRECTLY done**: `recover_index2_drops`
  (`table_manager_index_mgmt.rs`) and the hash RENAME recovery (both
  regular and unique families) now write the terminal status BEFORE
  clearing their tombstone — verified by reading the diff directly. Do
  not touch these.

Leave all of the above exactly as round 1 left it.

## What round 1 did NOT fix — the actual point of the task's title

**The inline/synchronous success path's write order is still wrong**, and
round 1's own comment in the diff admits it: at both `admin_table_index.rs`
DROP and RENAME sites, the terminal `Succeeded` status is STILL written
AFTER the mutating call (`table.drop_index(...)`/`table.rename_index(...)`)
returns — which is AFTER that call's OWN internal tombstone-clear already
happened (verify this yourself: `TableManager::drop_index`,
`table_manager_index_mgmt.rs:891-902`, delegates straight to
`self.index_manager.drop_index(name_id, op_id_str)` — `IndexManager::drop_index`,
in the `shamir-index` crate, is where the actual tombstone-write, posting
sweep, and tombstone-CLEAR sequence lives, per the P0-3/#959 durable-tombstone
pattern). A crash between the mutation completing and this status write
lands exactly in the gap the whole task exists to close, for the MOST
COMMON case (the synchronous, non-crash-recovery path) — this is the
task's own title ("crash-safe порядок записи") and it is not yet true for
this path.

**Round 1's comment claims this "is not feasible without significant
refactoring."** Verified independently: this is NOT true as stated. The
real constraint is narrower — `ddl_op_log` currently lives in
`crates/shamir-engine/src/table/ddl_op_log.rs`, and `shamir-index` (where
`IndexManager::drop_index`'s tombstone-clear actually happens) cannot call
into `shamir-engine` (dependency direction is `shamir-engine` →
`shamir-index`, confirmed via `crates/shamir-engine/Cargo.toml:26`,
`shamir-index = { path = "../shamir-index" }` — the reverse would be
circular). But `ddl_op_log.rs` itself only depends on types `shamir-index`
can already reach or trivially add: `shamir_storage::{error::DbError,
types::{RecordKey, Store}}` (already a `shamir-index` dependency, confirmed
`crates/shamir-index/Cargo.toml:13`), `bytes::Bytes`,
`shamir_types::types::record_id::RecordId` (already a dependency,
`crates/shamir-index/Cargo.toml:10`), and
`shamir_query_types::read::DdlOpStatus` — **NOT yet a `shamir-index`
dependency, but adding it does not create a cycle** (verified:
`shamir-query-types/Cargo.toml` has zero references to `shamir-index` or
`shamir-engine`). This is a targeted module relocation, not a redesign.

## Fix — move `ddl_op_log` down to `shamir-index`, write status inside `IndexManager` before its own tombstone-clear

1. Add `shamir-query-types = { path = "../shamir-query-types" }` to
   `crates/shamir-index/Cargo.toml` (mirror the existing dependency
   declaration style already used for `shamir-types`/`shamir-storage` in
   that same file).
2. Move `crates/shamir-engine/src/table/ddl_op_log.rs` to
   `crates/shamir-index/src/base_index/ddl_op_log.rs` (or wherever fits
   this crate's existing module layout best — check `shamir-index/src/lib.rs`
   for the convention). Keep its contents unchanged (the round-1 versioned-
   envelope logic is correct, just move the file).
3. In `crates/shamir-engine/src/table/mod.rs` (or wherever
   `crate::table::ddl_op_log` is currently declared), replace the module
   with a re-export: `pub use shamir_index::base_index::ddl_op_log;` (adjust
   the exact path to wherever you actually placed it in step 2) — this
   keeps EVERY existing call site in `shamir-engine` (the recovery paths
   round 1 already correctly fixed, plus anywhere else) compiling
   unchanged, since they all reference it as `crate::table::ddl_op_log::...`
   or `ddl_op_log::...` via a `use` import.
4. Inside `IndexManager::drop_index`/`drop_unique_index` (and the RENAME
   equivalents, wherever they live — grep for where the tombstone-clear for
   these actually happens, since `TableManager::drop_index` is just a
   thin barrier-acquiring wrapper per what you'll find at
   `table_manager_index_mgmt.rs:891-902`), write the terminal `Succeeded`
   status DIRECTLY, BEFORE the tombstone-clear step — mirroring EXACTLY the
   pattern round 1 already got right for the recovery paths (read
   `recover_index2_drops`'s new shape in `table_manager_index_mgmt.rs` for
   the concrete "status write, then clear" ordering to copy). This
   requires threading `op_id` (already a parameter — `Option<RecordId>` /
   `op_id_str: Option<String>`, per the existing `#1051` plumbing) through
   to wherever the tombstone-clear physically happens, and knowing the
   `DdlOpKind` at that point (same family-resolution logic already present
   in the caller — pass it down, or resolve it locally if the family is
   already known inside `IndexManager` at that point).
5. Once `IndexManager` writes its OWN terminal status before its OWN
   tombstone-clear, **remove the now-redundant (and wrongly-ordered)
   inline status write from `admin_table_index.rs`** — the mutation call
   already produces a durably-correct-ordered status by the time it
   returns. Do NOT leave both — a duplicate write is harmless
   (`write_op_status` is documented as an idempotent overwrite) but
   pointless and confusing; remove the outer one now that the inner one is
   authoritative and correctly ordered.
6. If, after actually attempting this, you find a GENUINE blocker (not
   just "this touches multiple files" — an actual technical
   impossibility), STOP and report EXACTLY what blocks it, citing the
   specific code that makes it impossible. Do not fall back to "not
   feasible" without a concrete, cited reason this time.

## Also fix while you're in this code

- **Dead branch.** `admin_table_index.rs`'s DROP INDEX handler has an
  `else { false }` branch in the family-dispatch `if`/`else if` chain that
  is now unreachable (round 1's own comment says so: "This branch is
  unreachable — we already short-circuited above"). Remove the dead branch
  entirely rather than leaving unreachable code with a comment
  acknowledging it's unreachable.
- **Double-check `is_unique`'s pre-mutation determination for RENAME.**
  Round 1 changed `table.unique_index_exists(&op.to)` (checking the NEW
  name, AFTER the rename) to `table.unique_index_exists(&op.rename_index)`
  (checking the OLD/source name, BEFORE the rename) — necessary to
  classify `DdlOpKind` before the mutation for the `InProgress` write. This
  is very likely correct (checking the source's family before it's renamed
  away should identify the same family), but it's a behavior-relevant
  change to existing logic — write a small dedicated test that renames
  BOTH a regular and a unique index and asserts the LOGGED `DdlOpKind`
  variant matches the correct family in each case, specifically to catch
  a regression here if the pre-mutation check turns out subtly wrong for
  some edge case you find.

## Tests — completely missing from round 1, this is the primary deliverable of this round

Round 1 wrote ZERO tests despite the original brief requiring 6. Write
them now, in a new file (check the existing convention —
`crates/shamir-db/src/shamir_db/tests/` for DDL-related test file naming,
e.g. mirror `p1_2_ddl_result_contract_tests.rs`'s style/location):

1. **InProgress written before mutation.** Race the DDL call against a
   pause hook via `tokio::select!` (NEVER `tokio::spawn`+`drop` — see
   `#1060`'s crash-recovery tests, `crates/shamir-engine/src/table/tests/p1060_online_index_crash_recovery_tests.rs`,
   for the exact proven pattern to copy) — you likely need to ADD a new
   test-only pause hook seam at the right point if none exists yet for
   this specific DDL path; check first whether one already exists before
   adding a new one. Assert the op-status log shows `InProgress` for the
   `op_id` while parked mid-operation.
2. **Terminal status durable before tombstone clear, for the INLINE path
   specifically** (this is the NEW thing round 2 fixes — test it
   directly, not just the recovery path which round 1 already covers
   implicitly via existing recovery tests). Race a DROP INDEX call against
   a pause hook positioned between the status write and the tombstone
   clear (inside `IndexManager::drop_index`, wherever you added the new
   write) — simulate a crash there, reopen, and assert the status log
   already shows `Succeeded` for that `op_id` (proving the write truly
   happens before the point that could crash and lose it).
3. **Status-write failure is not silently swallowed as a bare success.**
   However you decide to signal a status-write failure to the client
   (per the original brief's guidance — enrich the response, or another
   mechanism), write a test proving the caller can tell the difference
   between "fully succeeded, status durable" and "mutation succeeded but
   status write failed."
4. **Client-supplied correlation id round-trips.** DROP INDEX with a
   supplied `request_id` → the returned `op_id` equals it → polling by
   that same id finds the status.
5. **Idempotent retry.** Send the SAME DROP INDEX request (same
   `request_id`) twice. Assert the second call does NOT re-execute the
   drop and returns the SAME status as the first.
6. **Versioned envelope round-trips + rejects unrecognized versions.**
   Write then read a status record, assert correct decode; write a
   record with a corrupted/future version byte and assert `read_op_status`
   returns a clean `Err`, not a panic or silent misdecode.
7. **RENAME `DdlOpKind` family classification** (the `is_unique` check
   described above) — regular index rename logs `RenameHashIndex`, unique
   index rename logs `RenameUniqueHashIndex`.

**Every test must FAIL on code lacking the mechanism it proves.**

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-db
./scripts/test.sh -p shamir-server
```

Report the exact diff, confirm the module move compiles and every
existing call site still resolves, list which of the 7 tests above you
wrote, their individual pass/fail status, and the full gate's final
summary lines for every crate above. If you get stuck on the module
move or the `IndexManager` write-order change, report exactly where and
why — do not silently fall back to leaving the inline path unordered
again.
