# Brief for #802 (F-12) — fsync durability for backup/restore's copy + atomic rename

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The gap

`crates/shamir-server/src/backup.rs` and `crates/shamir-server/src/restore.rs`
copy files and perform atomic renames with NO explicit `sync_all`/fsync
anywhere in either path:

1. `copy_dir_recursive` (`backup.rs:504-530`, `pub(crate)`, shared by BOTH
   `backup()` at `backup.rs:212` and `restore()`'s step 3 at
   `restore.rs:196`) copies each file via `fs::copy(&path, &target)?`
   (`backup.rs:519`) with no follow-up sync — the copied bytes may still
   be sitting in the OS page cache, not yet on stable storage, when the
   function returns.
2. `write_manifest` (`backup.rs:293-311`) writes `manifest.json` via
   `fs::write(&manifest_path, json)?` (`backup.rs:309`) — same gap, no
   sync.
3. `restore()`'s step 5 atomic swap (`restore.rs:225-277`) performs up to
   two `fs::rename` calls (`data_dir -> backup_sibling` at
   `restore.rs:230`, `temp_dir -> data_dir` at `restore.rs:244`, or the
   single fresh-target rename at `restore.rs:275`) with no fsync of the
   containing directory afterward. A bare `fs::rename` can return success
   before the directory-entry update itself is durable — on a real power
   loss shortly after a "successful" rename, the filesystem may not
   reflect that rename on next boot (the classic "you fsync'd the file
   but not the directory" gap). This makes atomic rename NOT actually
   crash-durable without a following directory fsync.

## Existing precedent to follow, not reinvent

`crates/shamir-wal/src/wal_segment.rs:31-75` already solves the exact
"fsync a directory" problem for WAL segment creation:

```rust
#[cfg(unix)]
fn fsync_parent_dir(path: &std::path::Path) -> DbResult<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => return Ok(()), // no parent (root) — nothing to fsync
    };
    match std::fs::File::open(parent) {
        Ok(dir_f) => {
            if let Err(e) = dir_f.sync_all() {
                log::warn!(/* ... */);
            }
        }
        Err(e) => {
            log::warn!(/* ... */);
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn fsync_parent_dir(_path: &std::path::Path) -> DbResult<()> {
    // Windows / non-unix: directory fsync is not required for the
    // durability guarantee that matters here. No-op.
    Ok(())
}
```

Replicate this SAME design in `shamir-server` (it's `fn`-private to
`shamir-wal`, not importable directly — write a local equivalent in
`backup.rs` or `restore.rs`, whichever ends up owning the call sites;
`pub(crate)` if both files need it). Match its conventions exactly:
`#[cfg(unix)]` does the real `File::open(dir).sync_all()`, logging (not
propagating) a failure; `#[cfg(not(unix))]` is a documented no-op. Do NOT
invent a different cross-platform strategy — this workspace already
decided Windows doesn't need this specific guarantee the same way, stated
plainly in the existing comment; carry that same rationale into the new
copy.

No off-the-shelf crate exists for this in the dependency graph (`fs4` is
already a dep but only used for the advisory `data_dir` lock, not fsync) —
raw `std::fs` is the established pattern here.

## The fix — three call sites

1. **`copy_dir_recursive`** (`backup.rs:504-530`): after `fs::copy(&path,
   &target)?` succeeds, sync the copied file's contents to stable
   storage. `fs::copy` doesn't hand back an open file handle, so re-open
   the just-copied destination file and sync it — use
   `OpenOptions::new().write(true).open(&target)` (NOT `.create(true)` or
   `.truncate(true)`, which would wipe what was just copied) then
   `.sync_all()`. This single change fixes file-durability for BOTH
   `backup()`'s copy AND `restore()`'s step-3 copy, since they share this
   function.
2. **`write_manifest`** (`backup.rs:293-311`): replace the
   `fs::write(&manifest_path, json)?` at line 309 with an explicit
   `File::create` + `write_all` + `sync_all` sequence (matching
   `shamir-wal`'s `segment_meta.rs:83-107` tmp-file-then-sync-then-rename
   pattern's OWN internal sync step, minus the rename — this file isn't
   renamed into place, it's the final artifact) so the manifest itself is
   durable before `backup()` returns success.
3. **`restore()`'s step 5** (`restore.rs:225-277`): after EACH successful
   `fs::rename` in this function (the first `data_dir -> backup_sibling`
   rename at line 230, the second `temp_dir -> data_dir` rename at line
   244, the rollback rename at line 249 if it runs, AND the single
   fresh-target rename at line 275), call the new `fsync_parent_dir`-style
   helper on `parent` (already in scope as a local variable throughout
   this function) so the directory-entry change from each rename is
   pushed to stable storage before `restore()` returns. Since a fsync
   failure here is logged-not-propagated (matching the wal_segment.rs
   precedent), this cannot introduce a NEW error path into `restore()`'s
   return type — `RestoreError` should not need a new variant.

## Explicitly out of scope

- Do NOT add a parent-directory fsync to `backup()`'s own `dest_dir`
  creation (`backup.rs:208`, `fs::create_dir_all`) — `backup()` performs
  no rename at all (v1 is create-then-copy-in-place, no staging), so the
  specific "rename durability" gap this task targets doesn't apply there.
  If you judge it's ALSO worth a durability improvement, don't add it
  silently — note it as a documented residual/follow-up instead (same
  convention as the reference-profile gap flagged in F-11's commit),
  do not expand this task's scope to cover it.
- Do NOT change `RestoreError`'s variants, error propagation, or the
  swap/rollback control flow itself — this task ONLY adds sync calls
  around the EXISTING copy/write/rename operations, matching the
  established "log a fsync failure, don't fail the operation" precedent.
- Do NOT touch `crates/shamir-wal/src/wal_segment.rs` itself — read it for
  the pattern, don't modify it.

## Tests

Practical constraint to design around: this codebase (and this task's
own scope) cannot literally simulate an OS power-loss event in a portable
unit test — there is no way to truncate the OS page cache mid-syscall
from a `#[test]`. Match the honesty this campaign has used for similar
residuals elsewhere (e.g. F-9's documented reap-race residual): the test
suite verifies the CODE PATHS that matter (sync calls happen and don't
error in the happy path; an interrupted copy/rename still leaves no
partial/corrupt state in the live `data_dir`), not literal hardware
power-loss recovery. Add, in `crates/shamir-server/src/tests/backup_tests.rs`
and/or `crates/shamir-server/src/tests/restore_tests.rs` (check which
file already covers which side; extend both if the change touches both):

1. **Happy-path regression**: a full `backup()` → `restore()` round trip
   into a fresh target still succeeds and produces byte-identical data
   (`verify_manifest` on the restored `data_dir`, if applicable, or a
   direct file-content comparison) — proving the new sync_all/fsync calls
   don't silently break or slow the existing flow to the point of
   timing out.
2. **Interrupted-copy safety, extending the existing fault-injection
   convention**: `restore_tests.rs` already has `#[cfg(windows)]`
   sharing-violation-based fault injection (see
   `swap_failure_with_successful_rollback_gets_new_message_and_leaves_data_dir_intact`,
   `step5_first_rename_failure_cleans_up_staged_temp_dir`,
   `step4_invalidation_failure_cleans_up_staged_temp_dir`). Add a sibling
   test forcing `copy_dir_recursive`'s COPY step (step 3) to fail partway
   through a multi-file snapshot (e.g. via the same open-handle sharing-
   violation technique used by the existing tests, targeting one of the
   later files in copy order) and assert: the copy error propagates, the
   staged `*.restore_tmp_*` dir is cleaned up (existing N-6 behavior —
   confirm it still holds with the new sync call added to the copy loop),
   and — the key NEW assertion for this task — `data_dir` (and its
   pre-existing content, if any) is completely untouched, since step 5
   never ran. This is the closest portable proxy to "crash between copy
   and rename": whatever state existed before the interrupted copy is
   exactly what remains after.
3. Document in a comment near the new `fsync_parent_dir`-equivalent
   helper (and in `docs/guide-docs/KNOWN_LIMITATIONS.md`, a new entry) that
   directory-fsync is a no-op on non-unix (matching the established
   `wal_segment.rs` rationale) and that true power-loss crash injection is
   outside what a portable unit test can exercise — the interrupted-copy
   test above is the practical regression guard this task provides
   instead.
4. Update `docs/guide-docs/guide/07-operations.md`'s existing "Backup"
   section (~line 383-428) with a short subsection noting the fsync
   durability guarantees now provided (files synced before manifest is
   written; manifest synced; each rename's containing directory synced on
   unix) and the Windows no-op caveat — do not rewrite the existing
   stop-and-copy explanation, just add to it.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-server` and
  `cargo clippy -p shamir-server --all-targets -- -D warnings` must be
  clean.
- Follow workspace conventions: `use` at file top, surgical diff, no
  incidental refactors of `backup.rs`/`restore.rs` beyond what this task
  needs.
- If a sync/fsync call's own I/O error should propagate (e.g. the file
  `sync_all()` calls in `copy_dir_recursive`/`write_manifest` — unlike the
  DIRECTORY fsync, these are on files whose content correctness IS the
  point, so per the wal_segment.rs precedent's OWN distinction between
  "file sync" (propagate — content durability matters) and "directory
  fsync" (log-only — a softer, best-effort guarantee), a failed FILE sync
  should return an error (via the existing `BackupError::Io`/
  `RestoreError::Io` `#[from]` conversions), while a failed DIRECTORY
  fsync should only log. Confirm this asymmetry matches
  `wal_segment.rs`'s own treatment before implementing — if that file
  treats both identically, follow ITS actual precedent instead of this
  brief's assumption.

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -- backup
./scripts/test.sh -p shamir-server -- restore
```
