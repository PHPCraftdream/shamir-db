# Brief for F-19 (#812, P2) — fsync backup()'s destination directory

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-12 (#802, already landed) fixed two related durability gaps: file-content
fsync for copied files and the manifest (`backup.rs`), and directory-entry
fsync after each `fs::rename` in `restore()`'s atomic swap
(`restore.rs::fsync_dir`). Both `@oh`'s post-wave review (N-2) and the
deeper static-audit review (R9) found a remaining gap: **`backup()` itself
never fsyncs the destination directory it creates** — only `restore()`'s
RENAMES get a directory fsync. `backup()`'s `copy_dir_recursive` and
`write_manifest` create brand-new files/directories under `dest_dir` and
sync each file's CONTENT, but never sync `dest_dir` itself (or its newly
created subdirectories) to make those new DIRECTORY ENTRIES durable. Same
"fsync'd the file, not the directory" bug class F-12 already fixed for
renames, unaddressed for plain `create`.

Separately, R9 also noted the existing (already-correct-by-design)
choice that `restore()`'s directory-fsync failures are logged-only and
never propagated could be read as promising more durability than the code
actually delivers if the documentation isn't precise. This is NOT a code
bug — see `docs/guide-docs/KNOWN_LIMITATIONS.md` §9, which already
correctly explains the log-only rationale (matches `shamir-wal`'s
established convention). **Decision for this task: do NOT add a new
"strict mode" config knob that propagates directory-fsync failures** — that
would be a new feature, out of proportion for closing a documentation-
precision gap. Instead, tighten §9's wording (see below) so it cannot be
misread as promising full crash-durability.

## What to fix

### 1. `backup()`'s destination directory needs its own fsync

`crates/shamir-server/src/backup.rs`'s `backup()` function
(`~line 191-214`) calls `copy_dir_recursive(from, &dest_dir, ...)` then
`write_manifest(&dest_dir)`. Reuse the EXISTING `fsync_dir` helper already
defined in `restore.rs` (`~line 374-402`, `#[cfg(unix)]` real fsync +
`#[cfg(not(unix))]` no-op, logs-only on failure, never propagates) — make
it `pub(crate)` (it's currently private to `restore.rs`) and call it from
`backup()` once, AFTER `write_manifest` returns successfully (a single
fsync of `dest_dir` at that point covers every directory entry added to
it by BOTH steps — `fsync` on a directory flushes all of its current
entry metadata, not just recently-added ones, so one call after the last
write is sufficient for the top-level directory).

**Nested subdirectories**: a fjall-backed table directory can have nested
subdirectories (`copy_dir_recursive` recurses via `fs::create_dir_all`).
Fully rigorous durability would fsync EVERY directory `copy_dir_recursive`
creates, bottom-up, not just the top-level `dest_dir` — investigate whether
this is straightforward to add (e.g. `copy_dir_recursive` itself could
call `fsync_dir` on `target` right after `fs::create_dir_all(&target)`
succeeds, symmetric with how it already fsyncs each copied FILE right
after `fs::copy`). If straightforward, do it (mirrors the existing
per-file pattern this function already has). If it meaningfully
complicates the function, it's acceptable to scope this task down to just
the top-level `dest_dir` fsync and note the nested-subdirectory case as an
explicit follow-up in `KNOWN_LIMITATIONS.md` — use your judgment, but
document whichever choice you make.

### 2. Tighten `KNOWN_LIMITATIONS.md` §9's wording

Update the existing §9 section to:
- Mention the NEW `backup()`-destination-directory fsync (cite the new
  call site).
- Make explicit that this whole family of guarantees (file content fsync +
  directory-entry fsync on rename/create) is **best-effort protection
  against power loss**, not a blanket "crash-durable" guarantee — in
  particular the directory-fsync-failure-is-logged-only design means a
  directory-fsync failure does not fail the operation. This is intentional
  (matches `shamir-wal` precedent, already explained in the section) — just
  make sure the LANGUAGE around it can't be misread as a stronger promise
  than what's delivered. Don't invent a new "strict mode" — just make the
  existing wording precise.

## Tests

1. A test confirming `backup()`'s destination directory is fsync'd after a
   successful backup (on unix — the assertion can be as simple as "no
   error occurs and the fsync call happens", since asserting actual
   power-loss durability isn't unit-testable, per §9's own existing
   "Test scope note"; if there's an existing test-double/mock pattern for
   verifying an fsync call happened in this codebase, reuse it — otherwise
   a straightforward happy-path test that `backup()` succeeds and the
   directory is fsync'd without erroring is sufficient).
2. If you added recursive subdirectory fsync (see point 1 above), a test
   with a NESTED directory structure (e.g. a fake table dir with a
   subdirectory) confirming the backup succeeds and doesn't error.
3. Confirm existing `crates/shamir-server/src/tests/restore_tests.rs` /
   backup tests still pass unchanged (regression guard — `fsync_dir`'s
   signature/behavior must not change, only its visibility).

## Constraints

- Do NOT add a new "strict mode" config option — see the decision above.
- Do NOT change `fsync_dir`'s existing behavior (log-only on failure,
   `#[cfg(not(unix))]` no-op) — only its visibility (`pub(crate)`).
- Do NOT touch `restore()`'s existing rename-fsync call sites — already
  correct, out of scope.
- `cargo fmt -p shamir-server -- --check` and
  `cargo clippy -p shamir-server --all-targets -- -D warnings` must be
  clean.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -- backup
./scripts/test.sh -p shamir-server -- restore
./scripts/test.sh -p shamir-server --full
```
