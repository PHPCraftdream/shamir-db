# Brief 74 — #1067 (P1): DDL op-log wrong family classification + missing op_id for sorted/index2

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## The defect

`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs`:

- **DROP INDEX** (`handle_drop_index`, search `let kind = if is_unique`):
  resolves `DdlOpKind` for `is_unique`/`is_index2`/`is_regular` correctly,
  but for `is_sorted` it falls back to `DropHashIndex` with a comment
  admitting the gap: *"Sorted-family drops have no dedicated `DdlOpKind`
  variant yet in this slice... falls back to `DropHashIndex` like the rest
  of this status-log mechanism does for untracked families."* A client
  polling DROP-sorted-index status sees the wrong operation type.
- **RENAME INDEX** (`handle_rename_index`, search `let is_unique = table.unique_index_exists`):
  classification checks ONLY `is_unique` — `is_regular`, `is_sorted`, AND
  `is_index2` all collapse into the SAME `else { RenameHashIndex }` branch.
  A sorted or index2 rename is misreported as a hash rename.
- **Sorted family never receives `op_id` at all.** `table.drop_sorted_index(&op.drop_index)`
  (no op_id param) and, deeper down,
  `TableManager::drop_sorted_index(&self, index_name: &str) -> DbResult<bool>`
  (`crates/shamir-engine/src/table/table_manager_sorted_index.rs:325`) has
  no `op_id` parameter either — unlike `drop_index`/`drop_unique_index`/`drop_index2`,
  which all accept `Some(op_id)`. Same gap for RENAME:
  `SortedIndexManager::rename_index_sorted(&self, old_id: u64, new_id: u64) -> DbResult<()>`
  (`crates/shamir-index/src/base_index/sorted_index_manager.rs:1437`) has an
  explicit code comment admitting it: *"NOTE: Cannot write
  `DdlOpState::Failed` here because this layer (`SortedIndexManager`) does
  not have op_id in scope."* (same file, ~line 1446).
- **index2 RENAME has no terminal-status write at all.** #1066 (already
  landed) built the durable rename-tombstone/recovery mechanics for index2
  RENAME but explicitly deferred `DdlOpStatus`/`ddl_op_log` integration to
  THIS task — read `TableManager::rename_index`'s `is_index2` branch
  (`table_manager_index_mgmt.rs`, search `if is_index2 {` inside the RENAME
  dispatch function) to see the tombstone-write/`rename_entry`/
  `save_index2_metadata`/tombstone-clear sequence #1066 added; there is
  currently no `ddl_op_log::write_op_status` call anywhere in that branch.

## The fix

### 1. New `DdlOpKind` variants

`crates/shamir-query-types/src/read/ddl.rs` — add three variants, matching
the existing style exactly (doc comment per field, same shape as the
existing `DropHashIndex`/`RenameHashIndex` pairs):

```rust
/// `DROP INDEX` (sorted family).
DropSortedIndex { index_name: String },
/// `RENAME INDEX` (sorted family).
RenameSortedIndex { old_name: String, new_name: String },
/// `RENAME INDEX` (index2 family — FTS / functional / vector).
RenameIndex2 { old_name: String, new_name: String },
```

(`DropIndex2` already exists for index2 DROP — only RENAME needs a new
index2 variant.) Check for any exhaustive `match`/`matches!` over
`DdlOpKind` elsewhere in the workspace (`grep -rn "DdlOpKind::" crates/`)
that would need a new arm — the enum is very likely `#[non_exhaustive]` or
handled generically in most places (e.g. serialized opaquely), but verify
rather than assume; fix any compile break the new variants cause.

### 2. Thread `op_id` through the sorted family (mirrors #1048's hash work, #1065's index2 work)

- `SortedIndexManager::drop_index` (`crates/shamir-index/src/base_index/sorted_index_manager.rs:914`)
  — add an `op_id: Option<String>` parameter (same type convention
  `IndexManager::drop_index`/`drop_unique_index` in this same crate already
  use, per #1065). Inside, after the drop is durable and BEFORE
  `clear_from_dropping_sorted` (mirror #1065's exact write-order fix — read
  `IndexManager::drop_index` in `index_manager.rs` in this same crate for
  the pattern to copy verbatim: write `DdlOpStatus { kind: DdlOpKind::DropSortedIndex { index_name }, state: Succeeded { .. } }`
  via `ddl_op_log::write_op_status` BEFORE the tombstone clear, log
  `log::error!` — not swallow — on write failure).
- `TableManager::drop_sorted_index` (`crates/shamir-engine/src/table/table_manager_sorted_index.rs:325`)
  — add `op_id: Option<RecordId>` parameter (same convention as
  `drop_index`/`drop_unique_index` in this same file), thread it down to
  `SortedIndexManager::drop_index` as `op_id.map(|id| id.to_string())`.
- `SortedIndexManager::rename_index_sorted` (`sorted_index_manager.rs:1437`)
  — add an `op_id: Option<String>` parameter. Write the terminal `Succeeded`
  status (`DdlOpKind::RenameSortedIndex { old_name, new_name }`) AFTER step
  3 (`rekey_postings`) but BEFORE step 4 (tombstone clear) — same
  before-the-clear ordering discipline as everywhere else in this task
  family. You'll need `old_name`/`new_name` strings, not just the
  `old_id`/`new_id` interned u64s this function currently takes — resolve
  them via whatever name-lookup this manager already exposes (check
  `find_by_name_interned` or equivalent, used elsewhere in this same file)
  BEFORE the rename mutates anything, same reasoning as #1066's index2 fix
  (resolve identity before mutation, not after). Also fix the code comment
  admitting op_id isn't in scope — it will be, after this change.
- `TableManager::rename_index`'s `is_sorted` branch
  (`table_manager_index_mgmt.rs`, search `if is_sorted {`) — thread
  `op_id.as_ref().map(|id| id.to_string())` into the
  `rename_index_sorted` call, mirroring exactly how the `is_unique`/regular
  branches immediately above it already pass their own `op_id`.

### 3. Write the terminal status for index2 RENAME (deferred by #1066)

In `TableManager::rename_index`'s `is_index2` branch
(`table_manager_index_mgmt.rs`), after `save_index2_metadata` succeeds and
BEFORE `clear_from_renaming_index2` (the exact ordering #1066 already
established for the tombstone clear itself — slot the status write into
that same "after persist, before clear" position, mirroring how the
`is_unique`/regular RENAME branches already do this for their own families
a few lines above), write:
```rust
if let Some(ref id) = op_id {
    let status = DdlOpStatus {
        op_id: *id,
        kind: DdlOpKind::RenameIndex2 {
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
        },
        state: DdlOpState::Succeeded { completed_at: /* same pattern as neighboring branches */ },
    };
    if let Err(e) = crate::table::ddl_op_log::write_op_status(&self.info_store, &status).await {
        log::error!(/* same enriched-message convention as the neighboring branches */);
    }
}
```
Copy the EXACT surrounding style (variable names, error message shape) from
the `is_unique` branch's own terminal-status write immediately above in the
same function — don't invent a different shape.

Also add the equivalent write to `recover_index2_renames`
(`table_manager_index_mgmt.rs`, added by #1066) — when recovery drives a
tombstoned rename to completion, it should write
`SucceededViaCrashRecovery` for the resolved `op_id` (if the tombstone
entry carries one — it does, per #1066's `(id, old_name, new_name, op_id)`
tuple shape; #1066 loaded `op_id` but left it explicitly unused, per its
own doc comment — wire it up now). If `op_id` is `None` in the tombstone
entry (pre-#1067 tombstones, or non-DDL callers), skip the status write
silently — same convention `recover_index2_drops` already uses for a
`None` op_id.

### 4. Fix `admin_table_index.rs`'s classification

**DROP** (`handle_drop_index`): change the `is_sorted` arm from the
`DropHashIndex` fallback to `DdlOpKind::DropSortedIndex { index_name: op.drop_index.clone() }`.
Thread `Some(op_id)` into the `table.drop_sorted_index(&op.drop_index)`
call in the family-dispatch match (currently the ONLY family-dispatch arm
that doesn't pass `op_id` — check the `let removed = if is_regular { ... } else if is_sorted { table.drop_sorted_index(&op.drop_index) ... }`
block and match the calling convention the other three arms already use).

**RENAME** (`handle_rename_index`): the classification currently only
checks `is_unique` (via `table.unique_index_exists(&op.rename_index)`).
Add `is_sorted`/`is_index2` checks (mirror the exact resolution pattern
`handle_drop_index` already uses a few dozen lines earlier in this same
file — `table.sorted_index_exists(&op.rename_index).await`,
`table.index2_exists(&op.rename_index).await`) and branch to
`RenameSortedIndex`/`RenameIndex2` accordingly, falling back to
`RenameHashIndex`/`RenameUniqueHashIndex` only for the actual hash family.
Order the `if`/`else if` chain by resolving ALL FOUR families the same way
`handle_drop_index` does (checking exactly one is true — reuse or mirror
its cross-family-collision guard reasoning if it's not already shared code;
don't duplicate a subtly different version).

## Explicitly out of scope

Do not touch: the write-order fix itself (#1065, already correct), the
index2 RENAME tombstone/recovery atomicity (#1066, already correct — you
are ADDING a status write to its existing, working structure, not changing
when the tombstone is written/cleared), `DDL_OP_LOG_CAP`/eviction (#1068,
separate task).

## Tests — must discriminate families, not just check "status exists"

Per this task's own description: a test that only asserts "a status record
exists" is a known-tautological pattern already caught twice before
(#1051/#1052) — every test here must assert the SPECIFIC `DdlOpKind`
variant and its field values.

Required, in a new file following this codebase's test-organisation
convention (one file per topic under
`crates/shamir-db/src/shamir_db/tests/`, wired into `tests/mod.rs` — mirror
`p1065_ddl_status_contract_tests.rs`'s structure/API-usage style, which
uses the real `Batch`/`ddl::{drop_index,rename_index}` builder API, NOT a
fabricated one):

1. DROP each of the four families (regular, unique, sorted, index2) →
   poll status → assert the EXACT `DdlOpKind` variant matches that family
   (`DropHashIndex`/`DropUniqueHashIndex`/`DropSortedIndex`/`DropIndex2`)
   AND the `index_name` field is correct.
2. RENAME each of the four families → poll status → assert the EXACT
   `DdlOpKind` variant (`RenameHashIndex`/`RenameUniqueHashIndex`/`RenameSortedIndex`/`RenameIndex2`)
   with correct `old_name`/`new_name`.
3. Sorted DROP and sorted RENAME both carry a real `op_id` end-to-end —
   assert the returned `op_id` from the wire response matches what
   `ddl_op_log::read_op_status` finds (this is the "op_id was missing for
   sorted" half of the defect — prove it round-trips now).
4. index2 RENAME: assert a `Succeeded` status is durably written after a
   normal (no-crash) rename — this didn't exist before this task (#1066
   deferred it), so this test must FAIL against #1066-without-#1067's code
   (i.e., verify it would have failed before your change — either by
   temporarily reverting your status-write addition and confirming red, or
   by reasoning precisely about why the assertion is tight; do the revert
   check for at least this one).
5. Recovery: crash a sorted DROP/RENAME (or index2 RENAME) mid-flight using
   this codebase's established `tokio::select!` + pause-hook race pattern
   (NEVER `tokio::spawn` + `drop(JoinHandle)` — see
   `crates/shamir-engine/src/table/tests/p1060_online_index_crash_recovery_tests.rs`
   or `p1066_index2_rename_durability_tests.rs` for the proven shape to
   copy), reopen, and assert `SucceededViaCrashRecovery` is written with
   the correct family-specific `DdlOpKind` and the correct `op_id`.

**Every test must FAIL on code lacking the mechanism it proves.**

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-db
./scripts/test.sh -p shamir-server
./scripts/test.sh -p shamir-query-types
```

Paste the actual final summary line from each `./scripts/test.sh`
invocation (pass/fail counts) — literal output, not a paraphrase. List
every test you wrote by name with individual pass/fail status, and the
outcome of the mandatory revert-and-check self-verification. If anything
fails, fix it before reporting done. The standard on this codebase (see
#1065's 4-round history) is that everything you report is something you
personally watched pass, with the command's actual output as evidence.
