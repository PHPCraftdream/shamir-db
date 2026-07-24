# Brief: CR-D5 — polish batch: restore error-path, drain_all decision, hygiene nits (#786, N-6 + N-7 + N-9)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

This brief bundles three independent, small `@fh` review findings (N-6,
N-7, N-9). They touch disjoint files — land all three, but do not let one
block the others; if one proves larger than expected, finish the other two
and say so in your final report.

## Part 1 (N-6, LOW/MED) — `restore.rs` error-path polish

### Problem — confirmed by reading `crates/shamir-server/src/restore.rs` directly

`restore()`'s step-5 atomic swap (`restore.rs:159-189`) has two distinct
failure shapes that currently produce the IDENTICAL `RestoreError::SwapPartialFailure`
message:

- `restore.rs:165-177`: the second `fs::rename(&temp_dir, data_dir)` fails,
  the best-effort rollback (`fs::rename(&backup_sibling, data_dir)`) ALSO
  fails — this is the true "both directories exist, `data_dir` itself is
  now MISSING, operator must manually intervene" case. The current message
  ("operator must manually rename one of these two directories to
  {data_dir}") is CORRECT here.
- `restore.rs:178-183`: the second rename fails, but the rollback
  (`fs::rename(&backup_sibling, data_dir)`) SUCCEEDS — `data_dir` has
  already been restored to its PRE-restore state at this point. The
  IDENTICAL message is now WRONG: `data_dir` already exists again (holding
  the correct pre-restore data), there is nothing to "manually rename" —
  an operator following the message's own instruction in a live disaster
  scenario would attempt an unnecessary, confusing rename against a
  directory that's already fine.

### Fix

Split `RestoreError::SwapPartialFailure` into two distinct variants (or add
a field that changes the message — your call on the cleanest shape, but the
two cases MUST produce visibly different operator-facing text):

1. **Rollback succeeded** (`restore.rs:170`'s `is_err()` check is `false`):
   new variant/message stating plainly that the restore's final swap step
   failed, the automatic rollback SUCCEEDED, `data_dir` is intact and
   contains the ORIGINAL pre-restore data, and the restored copy (not
   swapped in) is left at `{temp_dir}` for inspection/retry — NOT a
   "manually rename" instruction, since nothing needs manual renaming.
2. **Rollback also failed** (`restore.rs:170`'s `is_err()` is `true`): keep
   today's message (it's accurate for this case) — `data_dir` is genuinely
   missing/uncertain, both `{pre_restore_backup}` and `{temp_dir}` exist,
   operator must manually choose and rename one to `{data_dir}`.

Also close the two smaller gaps this same code path has:

- **Staged `*.restore_tmp_*` cleanup on failure**: the temp dir created at
  `restore.rs:138` (`fs::create_dir_all(&temp_dir)`) is never removed if a
  LATER step fails (step 3's own copy failure at `restore.rs:141`, step 4's
  `FjallUserDirectory::open`/`invalidate_all_tickets` failure at
  `restore.rs:152-153`, or step 5's swap failure itself — in the swap-failure
  case `temp_dir` legitimately needs to survive per the error message above,
  so do NOT clean it up there; only the step-3/step-4 failure paths, where
  the error message gives the operator no reference to `temp_dir` at all
  today, need this). Add a best-effort `fs::remove_dir_all(&temp_dir)` on
  those two earlier failure paths (log a warning via `tracing::warn!` if the
  cleanup itself fails — do not let a cleanup failure mask the original
  error; the original error must still propagate via `?`/`Err(...)`
  unconditionally). Verify this doesn't touch the swap-failure branches,
  which must keep `temp_dir` on disk exactly as documented in the (updated)
  error messages.
- **`FjallUserDirectory::open`'s empty-`users`-store creation** when the
  snapshot lacks a `users` store: this is cosmetic materialization (creates
  an empty store where none existed in the snapshot). Just add a one-line
  doc comment at the call site (`restore.rs:152`) noting this side effect —
  do NOT change the behavior; a restored server needs SOME `users` store to
  open regardless, so an empty one is the correct outcome for a
  users-store-less snapshot, this is purely an undocumented-behavior gap,
  not a bug.

## Part 2 (N-7, LOW) — `create_cursor`'s `drain_all` failure handling

### Problem

`cursor_handlers.rs`'s `create_cursor` (search for `drain_all` — currently
around line 980) does:

```rust
if let Err(e) = repo.drainer().drain_all(&repo).await {
    tracing::warn!(?e, db = db_name, repo = %repo_name, "create_cursor: drain_all failed");
}
```

...then proceeds to serve the cursor's ENTIRE lifetime from the pinned
snapshot regardless. The surrounding doc comment (a few lines above this
call) states the drain is what makes "the pinned version's `AsOf` reads
coherent for the cursor's whole lifetime" — if that claim is literally
true, silently continuing past a failed drain risks serving incomplete/
incoherent pages for the cursor's WHOLE lifetime with nothing but a log
line for the operator to notice by. This is flagged as "the worst of both
worlds": claimed load-bearing, but ignorable in practice.

### Fix — investigate first, then resolve ONE of two ways (both are acceptable, pick based on what you find)

Read `read_temporal.rs`'s `read_as_of`/`current_stream_with_tombstones` (CR-B1's
territory) and whatever overlay-merge logic the temporal read path actually
uses, to determine: does an `AsOf` read at the pinned version ALREADY see
data through some overlay-aware path even when `drain_all` hasn't run (i.e.
is the drain actually just a performance optimization — avoiding a
slower/duplicated read path — rather than a correctness requirement)? The
`@fh` review's own read of the code suggested this might be the case
(`current_stream_impl`'s overlay merge / `get_at_many`'s overlay probes
appear to cover undrained data already) but this was not exhaustively
verified — YOU must confirm this one way or the other before choosing a fix,
since the two fixes are mutually exclusive and cannot both be applied.

- **If drain is confirmed a pure optimization** (undrained data is still
  correctly visible via the overlay-aware path, just slower to read): keep
  the `tracing::warn!` behavior, but REWRITE the doc comment above the call
  to say so plainly — remove the "coherent for the cursor's whole lifetime"
  claim (which would then be inaccurate) and replace it with something like
  "best-effort optimization: drains the repo's in-memory overlay into
  durable history once, upfront, so pinned-version AsOf reads don't pay the
  overlay-merge cost on every FetchNext; if this fails, reads remain
  correct (see `read_temporal.rs`'s overlay-aware path) but pay that cost
  for the cursor's lifetime instead — logged, not fatal."
- **If drain is confirmed load-bearing** (an undrained overlay genuinely
  produces an incoherent/incomplete `AsOf` read at the pinned version): make
  the failure propagate — `create_cursor` must return an error response
  (reuse `wrap_engine_err`, same pattern every other engine-error path in
  this file already uses) instead of logging and continuing. Add a new
  `BatchError` variant or reuse an existing generic query-error shape — your
  call, whichever fits `error_code()`'s existing classification scheme with
  the least new surface.

State clearly in your final report which of the two you found to be true
and which fix you applied — this is exactly the kind of "investigate,
then decide" step the brief cannot pre-resolve for you (the review flagged
it as genuinely ambiguous from a read-only pass).

## Part 3 (N-9, LOW) — hygiene batch

Three small, independent, low-risk cleanups:

1. **Stale comment in `handler.rs`** (currently around line 731-733, in
   `error_code()`'s match arms for `CursorNotFound`/`CursorExpired`/
   `CursorLimitExceeded`): says "actual cap/eviction enforcement (the only
   place these variants are currently constructed) lands in FG-5b" — FG-5b
   landed long ago (this whole cursor feature IS FG-5b, now well past Wave
   D). Remove or reword this comment to stop referencing a not-yet-landed
   future task that has, in fact, already landed — a `// FG-5b: wire error
   codes for the cursor protocol.` one-liner (or just delete the comment
   entirely if the code is self-explanatory without it) is enough.
2. **`fetch_next`/`create_cursor` authorize-vs-resolve-repo order asymmetry**:
   `create_cursor` authorizes (`authorize_cursor_read`) BEFORE resolving the
   repo (`resolve_repo`) — see the sequence a few lines apart in that
   function. `fetch_next` does the OPPOSITE: it resolves the repo
   (`resolve_repo`, currently ~line 1300) BEFORE re-authorizing
   (`authorize_cursor_read`, currently ~line 1329). Functionally harmless
   today (the session already owns the cursor by the time `fetch_next`
   runs, so `resolve_repo`'s error path leaks no additional information),
   but gratuitously asymmetric. Swap `fetch_next`'s order to match
   `create_cursor`'s (authorize first, then resolve the repo) — re-verify
   after the swap that every existing `fetch_next` test still passes
   unchanged (this must be a pure reordering with no behavior change, since
   both calls are independent read-only checks with no side effects on each
   other).
3. **TS `CursorIterator.return()` not chained behind the `pending` promise**
   (`crates/shamir-client-ts/src/core/cursor-iterator.ts`, search for
   `return()`, currently around line 225): a manual driver (calling `.next()`
   and `.return()` directly rather than via `for await...of`) that overlaps
   an in-flight `next()` with a `return()` call can have the in-flight
   `doNext` repopulate the internal buffer AFTER `return()` already cleared
   it — `for await...of` never does this (it always awaits each `next()`
   before requesting the next one), so this is a manual-driving edge case
   only. Fix: have `return()` await the current `pending` promise (if any)
   before doing its own cleanup, mirroring whatever pattern `next()` itself
   already uses to serialize against a concurrent call (check the file for
   an existing `pending`-tracking convention before inventing a new one).
   Related, ACCEPTABLE-AS-IS edge case (do not fix, just add a one-line
   comment noting it's understood): `return()` called while the FIRST
   `next()` is still in flight (before the cursor id is even known yet)
   skips the server-side cancel entirely, leaving that cursor to the
   idle-timeout backstop — this is fine (mirrors the Rust SDK's own
   documented reliance on the idle-timeout reaper for a `Drop`-based early
   abandonment), just undocumented; add the one-line comment.

## Tests (TDD — write failing tests first, where a test can meaningfully cover the change)

- **N-6**: in `crates/shamir-server`'s existing restore test module (find it
  — likely `tests/` alongside `restore.rs` or an integration test file;
  check before creating a new one), add a test that forces the swap-failure
  path where rollback SUCCEEDS (e.g. by making the second rename fail via a
  read-only/locked target while the rollback rename itself is left able to
  succeed — check how existing restore tests simulate rename failures, if
  any do, and mirror that technique) and asserts the NEW, correct message
  (data_dir intact, no manual action needed) — distinct from the existing
  both-failed message. Also a test (or extend an existing one) asserting
  the staged temp dir is removed after a step-3/step-4 failure.
- **N-7**: whichever fix you land on, add/extend a `cursor_handler_tests.rs`
  test that pins the NEW behavior precisely (either: drain failure is
  confirmed harmless and a test proves reads stay correct even when
  simulated to fail, with a doc-comment-only diff — no behavior test needed
  in that case, just confirm existing tests stay green; OR: drain failure
  now produces a clean `CreateCursor` error response, with a test forcing
  the failure and asserting the error).
- **N-9.2**: extend an existing `fetch_next` test (or the ACL-revoked-
  mid-scroll test already covering `authorize_cursor_read`'s enforcement) to
  confirm behavior is unchanged after the reorder — this should require NO
  new test if an existing one already exercises `fetch_next`'s auth-denial
  path; just confirm it still passes.
- **N-9.3** (TS): add a test in the TS SDK's cursor-iterator test suite
  (`crates/shamir-client-ts`'s relevant `*.test.ts` — locate it before
  writing) simulating an overlapping manual `next()` + `return()` and
  asserting the buffer is not repopulated after `return()` completes.

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

Plus, for the TS-only N-9.3 fix, whatever the TS SDK's own test/lint
commands are (check `crates/shamir-client-ts/package.json` scripts —
likely `npm test`/`npm run lint` or similar; run them the same way earlier
Wave B/C tasks touching the TS SDK did, e.g. CR-B6).

All must pass before returning. Primary code area: `shamir-server`
(`restore.rs`, `cursor_handlers.rs`, `handler.rs`, their tests),
`shamir-client-ts` (`cursor-iterator.ts`, its tests). Do NOT touch
CR-D1/D2/D3/D4's pagination-mode, numeric-comparison, or serialization
logic — this task is a disjoint polish batch.
