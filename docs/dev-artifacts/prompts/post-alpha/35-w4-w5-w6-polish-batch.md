# Brief: W-4 + W-5 + W-6 — polish batch (#790)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

This brief bundles three independent LOW-severity findings from the `@fh`
Wave D review (`docs/dev-artifacts/research/2026-07-24-wave-d-review.md`,
findings W-4, W-5, W-6). They touch disjoint files — land all three, but
do not let one block the others; if one proves larger than expected,
finish the other two and say so in your final report.

## Part 1 (W-4, LOW) — CR-D2's null probe escapes the RI-15 admission gate

### Problem — confirmed by reading `cursor_handlers.rs` directly

`create_cursor`'s CR-D2 null-detection probe
(`order_by_column_contains_null`, called around line 1101) runs BEFORE
`reserve_page_budget_upfront` (called around line 1137) — i.e. before the
RI-15 global in-flight response-byte budget is reserved for this cursor's
first-page read. The probe's own read (`table.read_with_encoding(&probe,
ctx, ...)` inside `order_by_column_contains_null`) materializes the FULL
null-matching result set internally before its `LIMIT 1` pagination
applies — this cost class is already documented in CR-D2's own doc comment
("`Temporal::AsOf`'s read path... is a full tombstone-inclusive streaming
scan of the table regardless of `limit`... this probe is therefore the SAME
cost class as `create_cursor`'s own first-page read"). Since it's a
ONE-TIME cost per cursor creation (not per-page), the risk is bounded, but
it currently runs with ZERO admission-gate protection — a burst of
concurrent `CreateCursor` calls against a keyset-eligible query could all
run this probe simultaneously, unthrottled by the very budget mechanism the
first-page read right after it IS subject to.

### Fix — pick ONE of two acceptable resolutions, based on what you find

Read `order_by_column_contains_null` and `reserve_page_budget_upfront`
carefully, then decide:

- **Preferred, if it doesn't complicate CR-D2's own control flow**: move
  the `reserve_page_budget_upfront` call to run BEFORE the null probe,
  holding the guard across both the probe and the first-page read (both
  reads happen inside the same overall `create_cursor` call, so one
  upfront reservation covering both is a legitimate widening of what it
  protects — check whether `enforce_page_budget`'s later shrink-to-actual-
  size step still works correctly when the guard was acquired earlier;
  it should, since `shrink_to`/`grow_unchecked` only care about the FINAL
  measured size, not when the reservation started).
- **If reordering turns out to complicate CR-D2's "probe before deciding
  mode, mode before running the first query" control flow more than this
  polish task's scope warrants**: leave the ordering as-is, but add a
  clear one-line comment at the probe's call site explicitly documenting
  that this ONE-TIME, per-cursor-creation probe is deliberately exempt from
  the RI-15 admission gate (bounded cost, not a per-page repeat), so a
  future reader doesn't mistake this for an oversight.

State which you chose and why in your final report.

## Part 2 (W-5, LOW) — two `restore.rs` residuals CR-D5/N-6 didn't cover

CR-D5 (#786, N-6) added best-effort staged-temp-dir cleanup on step-3
(copy) and step-4 (invalidate) failures, and split the step-5 swap-failure
error into two variants — but its own brief only enumerated steps 3/4 for
cleanup, missing a step-5 sub-case, and didn't address a pre-existing
TOCTOU.

### W-5(a): step-5's FIRST rename failure orphans `temp_dir` with no pointer to it

`restore.rs`'s step-5 atomic swap (currently around line 200-230) does:

```rust
fs::rename(data_dir, &backup_sibling)?;   // <-- if THIS fails...
match fs::rename(&temp_dir, data_dir) { ... }
```

The FIRST rename (`data_dir` → `backup_sibling`) uses a bare `?` — if IT
fails (before the second rename is even attempted), the error propagates as
a plain `RestoreError::Io` with ZERO reference to `temp_dir` anywhere in
the message. `temp_dir` is the FULLY-STAGED (copied, tickets-invalidated)
snapshot at this point — orphaned on disk with no discoverable pointer to
it, the EXACT class of gap N-6 already fixed for steps 3/4, just missed for
this specific step-5 sub-case (N-6's own brief enumerated only "steps
3/4").

Fix: on this specific failure (the first rename's `Err`, BEFORE the second
rename runs), best-effort clean up `temp_dir` the same way N-6's
`cleanup_staged_temp_dir` helper already does for steps 3/4 (reuse that
same helper function — do not duplicate it) before propagating the
original error. This is DISTINCT from the swap-failure cases already
handled by `SwapFailedRollbackSucceeded`/`SwapPartialFailure` (those are
about the SECOND rename failing, where `temp_dir`'s survival is
load-bearing for the error message) — this is specifically the FIRST
rename's failure, where nothing has been staged for a rollback yet and
`temp_dir` genuinely has no further use.

### W-5(b): TOCTOU between the staged-dir existence check and its creation

Step 3 currently does:

```rust
if temp_dir.exists() {
    return Err(RestoreError::Io(std::io::Error::new(ErrorKind::AlreadyExists, ...)));
}
fs::create_dir_all(&temp_dir)?;
```

This is a classic check-then-act race (contrived in practice — two
`restore()` calls against the same `data_dir` in the same second — but
worth closing cheaply if it's a clean drop-in). Investigate: does swapping
`fs::create_dir_all(&temp_dir)` for `fs::create_dir(&temp_dir)` close this
atomically? `fs::create_dir` (unlike `create_dir_all`) fails with
`ErrorKind::AlreadyExists` if the target already exists — a single atomic
syscall, no window between check and create. Verify `temp_dir`'s PARENT
directory is always already guaranteed to exist at this point in the
function (it should be — `parent` is derived from `data_dir.parent()`,
which necessarily already exists for any real `data_dir`) before making
this swap, since `create_dir` (unlike `create_dir_all`) does NOT create
missing parent directories. If parent existence is guaranteed, replace the
`exists()`-check-then-`create_dir_all` pattern with a single
`fs::create_dir(&temp_dir)` call whose `Err(e) if e.kind() ==
ErrorKind::AlreadyExists` arm produces the same operator-facing message the
current explicit check does (adjust the error-construction code
accordingly — keep the message text, just change the mechanism that
detects the condition).

## Part 3 (W-6, LOW) — undisclosed behavior change from CR-D5's `fetch_next` reorder

### Problem

CR-D5 (#786, N-9) reordered `fetch_next` to authorize before resolving the
repo, matching `create_cursor`'s order, characterizing it as "a pure
reshuffle, no behavior change." This is not QUITE true: if the underlying
db was DROPPED mid-scroll (between `CreateCursor` and a later
`FetchNext`), the NEW order causes `authorize_cursor_read` to run first
against a now-nonexistent db — and the fail-closed `resource_meta` handling
in `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs` (~lines
850-865, check the exact lines when you open the file) turns a missing/
errored resource lookup into `access_denied` for a non-admin actor, rather
than the OLD order's `unknown_db` (which came from `resolve_repo` running
first and hitting the db-not-found case directly). This is a DEFENSIBLE
outcome (arguably MORE correct — fail-closed on a resource that can't be
resolved is the safer default than a bare not-found), but it was untested
and undisclosed as a behavior change.

### Fix

This is a documentation + test task, NOT a behavior-reversal task — the
review explicitly calls the new behavior "defensible." Do NOT revert
CR-D5's reorder.

1. Add a one-line code comment at the `fetch_next` authorize-then-resolve
   site (where CR-D5's own reorder comment already lives) noting: "a db
   dropped between `CreateCursor` and this `FetchNext` now surfaces as
   `access_denied` (fail-closed `resource_meta`), not `unknown_db` — a
   deliberate, accepted consequence of authorizing before resolving,
   documented here since it was not explicitly called out when the reorder
   landed (CR-D5/N-9)."
2. Add a test pinning this EXACT behavior: open a cursor, drop the
   underlying db (or repo, whichever is more natural to construct in the
   existing test harness — check `cursor_handler_tests.rs`'s existing
   fixtures for how a db/repo gets dropped mid-test, if any test already
   does this for a related purpose), then call `FetchNext` and assert the
   response is `access_denied` (not `unknown_db`) — a REGRESSION GUARD for
   the now-understood-and-accepted behavior, not a bug fix.

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning. Primary code area: `shamir-server`
(`cursor_handlers.rs`, `restore.rs`, their tests). Do NOT touch
W-1/W-2/W-3's numeric-comparison or bookmark-typing fixes, or CR-D1..D5's
own logic beyond the narrow additions this brief describes — this task is
a disjoint polish batch.
