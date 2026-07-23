# Brief: CR-B7 — restore: invalidate tickets before swap + manifest hardening (#773)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

**This task touches disk-level atomic operations (rename swaps) — read
this brief fully before editing, and reason carefully about ordering.**

## Problem 1 — ticket invalidation runs AFTER the atomic swap, verified against the current tree 2026-07-23

`crates/shamir-server/src/restore.rs::restore` (~lines 105-187) runs, in
order: liveness probe (Step 1) → source manifest verification (Step 2,
`backup::verify_manifest(from)`, ~line 113) → copy snapshot to a temp
sibling directory (Step 3, ~lines 115-134) → **atomic swap** (Step 4, ~lines
136-167: rename current `data_dir` to `.pre_restore_backup_<timestamp>`,
then rename the temp dir into place as the new `data_dir`) → **ticket
invalidation** (Step 5, ~lines 169-179: `FjallUserDirectory::open(data_dir
.join("users"))` then `invalidate_all_tickets(now_ns)`, against the
NOW-LIVE restored `data_dir`).

If Step 5 fails (`RestoreError::UserDirectory`/`Invalidate`, ~lines 73-76),
`restore` returns an error — but the swap in Step 4 already completed. The
new (restored) `data_dir` is live, and pre-restore resumption tickets
(issued before the restore point, against whatever server state existed
then) were never invalidated against it — a security-relevant partial-
failure window where the operator sees an error but the actual live state
is silently NOT what the error implies.

### Fix — do the ticket invalidation INSIDE the staging copy, before the swap

Reorder so a failure at the invalidation step leaves the CURRENT
(pre-restore) `data_dir` completely untouched:

1. Steps 1-3 stay exactly as they are (liveness probe, source manifest
   verify, copy snapshot into `temp_dir`).
2. **New Step, before the swap**: open the user directory INSIDE
   `temp_dir` (i.e. `FjallUserDirectory::open(temp_dir.join("users"))`,
   NOT `data_dir.join("users")`) and call `invalidate_all_tickets(now_ns)`
   there. This validates two things at once: the staged snapshot's user
   store is structurally loadable (a corrupt/incompatible snapshot fails
   HERE, before anything touches the live `data_dir`), and pre-restore
   tickets are invalidated in the copy that is ABOUT to become live, not
   after it already is. `drop(users)` immediately after (mirrors the
   existing post-swap code's own `// release fjall's lock before
   returning` comment) so the lock is released before the swap renames
   this same directory tree.
3. Consider whether a `Database::persist(PersistMode::SyncAll)` (or
   whatever this store's flush call is — check
   `FjallUserDirectory`/`shamir-storage`'s `storage_fjall.rs` for the
   exact method, the module doc for `backup.rs` already references
   `Database::persist(PersistMode::SyncAll)`) is needed after the
   invalidation write and before `drop(users)`/the swap, so the
   invalidated state is durably on disk before the directory gets
   renamed into place — check whether `invalidate_all_tickets` already
   does this internally (it commits via a fjall batch per the existing
   code, ~line 992-994 area) or whether an explicit extra flush is
   needed for this specific "about to be renamed and become the live
   store" scenario.
4. **THEN** perform Step 4 (the atomic swap), exactly as today — no
   change to the swap logic itself, the rollback-on-second-rename-failure
   behavior, or the `.pre_restore_backup_*` preservation semantics.
5. Remove the OLD Step 5 (post-swap invalidation) entirely — it's now
   redundant, the invalidation already happened in staging.
6. Update `restore.rs`'s module doc comment (~lines 1-32, the "Procedure"
   list) to describe the NEW step order accurately — this is exactly the
   kind of doc/code drift `CLAUDE.md` and this session's own `CR-A7`
   precedent care about; don't leave the doc describing the old order.
7. `RestoreReport.users_invalidated` (~line 87) is populated from the
   staging-phase invalidation's return count instead of the post-swap
   one — the field's meaning doesn't change, just which call produces the
   number.

**A failure at the new pre-swap invalidation step must leave `data_dir`
(the CURRENT, pre-restore data) completely untouched** — this is the core
new guarantee; write a test proving it (see Tests below). The existing
`.pre_restore_backup_*` rollback mechanism (protects against a
POST-swap second-rename failure) is UNCHANGED and stays as the backstop
for that separate failure mode.

## Problem 2 — manifest hardening gaps, verified against the current tree

`crates/shamir-server/src/backup.rs`'s `Manifest`/`ManifestFileEntry`
(~lines 95-113) and `verify_manifest` (~lines 246-300):

- `Manifest.format_version` **already exists** as a field (`u32`,
  `MANIFEST_FORMAT_VERSION = 1`, ~line 116) — contrary to what an earlier
  draft of this task assumed, you do NOT need to add the field. What's
  missing is **validation**: `verify_manifest` currently never checks
  `manifest.format_version` against `MANIFEST_FORMAT_VERSION` at all — add
  that check near the top of `verify_manifest` (right after the
  `serde_json::from_slice` parse, ~line 254), rejecting an unknown/future
  format version with a clear new `BackupError` variant (e.g.
  `UnsupportedManifestFormatVersion { found: u32, expected: u32 }`) rather
  than silently proceeding to interpret entries under assumptions that may
  not hold for a different format.
- **No path-traversal guard.** `verify_manifest`'s per-entry loop (~lines
  259-278) does `snapshot_dir.join(&entry.path)` directly — `PathBuf::join`
  with an ABSOLUTE path argument REPLACES the base entirely (a documented
  Rust stdlib behavior), and a relative path containing `..` components
  escapes `snapshot_dir` even without being absolute. A tampered/malicious
  manifest could therefore make `verify_manifest` read a file OUTSIDE the
  snapshot directory. Add a validation pass over every `entry.path` (both
  in `verify_manifest` and anywhere else a manifest is trusted) that
  rejects: an absolute path (check via `Path::is_absolute()`), or any path
  containing a `..` (`ParentDir`) component (check via
  `Path::components()` and reject `std::path::Component::ParentDir`). Add
  a new `BackupError` variant (e.g. `UnsafeManifestPath(String)`) for
  this rejection.
- **No duplicate-entry check.** `verify_manifest`'s `accounted` set
  (~line 194, a `TFxSet<String>`) is built by inserting each entry's path
  as it's verified — currently nothing checks whether a path was already
  seen before this backup's own manifest was generated (a hand-crafted
  manifest could list the same path twice, one entry's checksum
  potentially replacing/shadowing the other's expectations in ways that
  could confuse a naive restore-from-manifest consumer, even though
  today's `restore.rs` doesn't directly copy per-manifest-entry — harden
  anyway per the review, since verify_manifest's own reasoning should not
  assume a well-formed manifest). Reject if `accounted.insert(...)` would
  find the key already present (i.e. check BEFORE inserting, and treat a
  second occurrence as a hard error) — new `BackupError` variant (e.g.
  `DuplicateManifestEntry(String)`).

### Backup destination guard (same area, same task)

`backup()` (~lines 140-170) does not check whether `to` is a path INSIDE
`from` (`data_dir`) — a `backup --to <data_dir>/somewhere` (or any
ancestor-descendant relationship in that direction) would recursively back
up the backup's own destination into itself, corrupting the operation.
Add a check near the top of `backup()` (after the existing
`from.exists()`/`from.is_dir()` checks, before the destination-exists
check) that canonicalizes both `from` and `to` (`Path::canonicalize`,
handling the case where `to` doesn't exist yet — canonicalize `to`'s
existing ancestor and compare prefixes, since `to`'s own timestamped
subdir won't exist prior to this call) and rejects with a new
`BackupError` variant (e.g. `DestinationInsideSource { from: PathBuf, to:
PathBuf }`) if `to`'s canonical path is inside (or equal to) `from`'s
canonical path.

## Tests (TDD — write failing tests first)

Mirror the existing conventions: `crates/shamir-server/src/tests/backup_tests.rs`
for unit-level backup/manifest tests, `crates/shamir-server/tests/backup_restore_e2e.rs`
for the full end-to-end restore flow (see its existing
`restore_cli_rolls_back_data_and_invalidates_tickets` test, ~line 615, as
the template for how this suite already sets up a real server boot →
write → backup → restore cycle with `TempDir`).

- **Core new guarantee**: inject a failure at the (new, pre-swap)
  invalidation step (e.g. by staging a snapshot whose `users` fjall store
  is deliberately corrupted/unopenable, or another reliable failure
  injection point you find in the staging step) and assert the ORIGINAL
  `data_dir` is completely untouched afterward (same files, same content,
  no `.pre_restore_backup_*` sibling created at all, since the swap never
  ran) — this is THE test that proves the reordering fix.
- **Happy-path regression**: the existing
  `restore_cli_rolls_back_data_and_invalidates_tickets` (and any other
  existing restore/backup test) must stay green — full restore still
  works end-to-end, tickets still end up invalidated in the final live
  `data_dir`.
- **Manifest with a `../` escape path** is rejected by `verify_manifest`
  (construct a manifest by hand in a test fixture with a malicious
  `entry.path`, confirm the new `UnsafeManifestPath`-class error).
- **Manifest with an absolute path entry** is rejected (same class,
  separate test case — `Path::is_absolute()` and `..`-component are two
  different checks, test both).
- **Duplicate manifest entries** rejected.
- **Unknown/future `format_version`** rejected (hand-craft a manifest with
  `format_version: 999`).
- **`backup --to` destination inside source** rejected (e.g. `backup(&src,
  &src.join("subdir"))` or `backup(&src, &src)`).

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning. Primary code area: `shamir-server`
(`restore.rs`, `backup.rs`, their test files). Disjoint from the cursor
work — do not touch `db_handler/`, `cursor_registry.rs`, or
`byte_budget.rs`.
