# Brief — fix R0-B's own regression (F-1) + finish R0-D's fail-closed policy (F-2)

## Context

S.H.A.M.I.R. Database, `crates/shamir-engine` + `crates/shamir-index`. An
adversarial review of the just-landed R0 correctness-freeze wave
(`docs/dev-artifacts/research/2026-08-06-r0-wave-adversarial-review.md`)
found that commit `a73c57c6` (R0-B, instance provenance for tx-plan
reconcile) introduced a **live-path regression of the exact class it was
built to fix** — confirmed and REPRODUCED by the reviewer with an actual
revert-and-test cycle, not just read. This brief fixes that (F-1) plus a
second, independent gap the SAME wave's R0-D commit (`5935b346`) left half-
closed (F-2). Both are tracked as tasks #1027/#1028.

## F-1 (CRITICAL) — batch INSERT never stamps index2 provenance, silently loses postings

### The defect, confirmed and reproduced by the review

`crates/shamir-index/src/write_ops.rs:12-37` defines `index2_provenance()` as
a PLACEHOLDER (`instance_epoch: 0`) that every index2 backend's `plan_*`
method stamps onto its ops — the doc comment explicitly lists which callers
MUST overwrite it with the real live epoch via `stamp_index2_provenance`/
`stamp_index2_ops_provenance`: "`TableManager::plan_insert_ops`/
`plan_update_ops`/`plan_delete_ops` (stage time) and
`rederive_index2_ops_post_stage` (commit time)". **That list is incomplete.**

Two sites in `crates/shamir-engine/src/table/table_manager_tx_ops.rs` inline
the `backend.plan_insert_tx(...)` loop directly (to amortize the
`all_backends()` snapshot across a whole batch) and never call
`stamp_index2_provenance`:

- `insert_tx_many` (currently around `:592-605` — grep for the function, line
  numbers may have shifted).
- `insert_tx_many_bytes` (currently around `:773-786`).

Compare against the four sites that DO call it correctly (`plan_insert_ops`,
`plan_update_ops`, `plan_update_ops_ref`, `plan_delete_ops` in the same
file) — the fix is structurally identical to what those four already do.

**`insert_tx_many_bytes` is not a rare path**: it's what `execute_insert_tx`
and `execute_set_tx` (in `crates/shamir-engine/src/table/write_exec.rs`) call
— i.e. every transactional INSERT/UPSERT through the query executor,
including a single-row insert.

### Exact failure sequence (already reproduced by the reviewer — you can
reproduce it yourself the same way before fixing, to see it fail first)

1. Table has a functional/FTS index2 index `idx_a` at some live epoch (say 1).
2. A tx stages an INSERT via `insert_tx_many`/`insert_tx_many_bytes` — the
   resulting index2 op carries `Provenance { instance_epoch: 0, .. }` (the
   never-overwritten placeholder).
3. Between stage and commit, ANY index2 DDL happens on the same table (even
   an unrelated CREATE of a different index) — `IndexRegistry.generation`
   advances.
4. At commit, the generation gate opens; `rederive_index2_ops_post_stage`
   adds ops for the newly-registered backend and builds
   `live_index2 = {(idx_a, 1), (idx_new, <fresh epoch>)}`; the shared
   retractor runs.
5. The staged op's `(idx_a, 0)` does not match anything in `live_index2`
   (epochs start at 1, confirmed in `write_ops.rs`'s own doc) — it is
   RETRACTED.
6. The row commits to the table successfully, but its posting in `idx_a` is
   permanently missing. `IndexWriteOp::BumpFtsStats` carries no provenance
   (never retracted), so FTS stats get incremented for a row with no
   posting — a lasting stats/posting desync on top of the missing row.

This is a REGRESSION relative to pre-R0-B: before R0-B, index2 had no
retraction at all, so this exact staged op would have survived to commit.

### Fix

1. Add `self.stamp_index2_provenance(&backend, &mut ops).await;` inside both
   batch loops in `table_manager_tx_ops.rs`, in the same relative position
   the four correct call sites use (immediately after `plan_insert_tx`
   returns, before extending into the outer `index_ops`/accumulator vec).
2. Fix the doctrine gap that let this happen: `write_ops.rs:12-37`'s doc
   comment lists SPECIFIC FUNCTION NAMES as "must overwrite" — that's a list,
   not an invariant, and lists get out of sync with the codebase (exactly
   what happened here). Rewrite it to state the actual invariant precisely:
   "ANY code path that adds an index2 `IndexWriteOp` to `tx.index_write_set`
   (directly or via a staging helper) MUST have overwritten this placeholder
   first — grep for every call site that pushes into `tx.index_write_set`
   or an accumulator later merged into it, for index2 ops specifically, and
   confirm each one stamps." Do this grep yourself now and confirm there are
   no OTHER unstamped sites beyond the two named above (check `update`/
   `delete` batch paths too, if they exist with the same inlining pattern —
   the review only confirmed the INSERT batch paths, verify the update/delete
   batch equivalents, if any exist, are not similarly affected).
3. If you can make this class of bug structurally impossible without a
   large redesign (e.g. a newtype wrapper around a stamped `IndexWriteOp` that
   only `stamp_index2_provenance`/`stamp_index2_ops_provenance` can produce,
   so an unstamped op can't be pushed into `tx.index_write_set` by
   construction) — do it, but ONLY if it's a small, surgical change; if it
   would require touching many unrelated call sites, skip it and rely on (2)
   plus the test in (4) instead. Do not let this balloon the fix.
4. **Test**: a batch-path twin of whatever single-record provenance test
   already exists in `crates/shamir-engine/src/table/tests/p1008_instance_provenance_tests.rs`
   (grep for a test proving a single INSERT's index2 posting survives an
   unrelated concurrent index2 CREATE) — same scenario, but driven through
   `insert_tx_many`/`insert_tx_many_bytes` instead of the single-record path.
   Confirm it FAILS against the current (unfixed) code before applying the
   fix, then confirm it passes after.

## F-2 (HIGH) — regular index backfill still silently skips corrupt records

### The defect

`crates/shamir-index/src/base_index/index_manager.rs` (currently around
`:1141-1150` — grep for the backfill loop) has the SAME silent
`Err(_) => continue` pattern R0-D (#1023, commit `5935b346`) already fixed
in `create_unique_index`'s backfill
(`crates/shamir-index/src/base_index/index_manager_unique.rs`, currently
around `:379-401`) — but only fixed there. The regular (non-unique)
backfill loop, two files over, in the SAME commit's scope, was left
untouched.

### Exact failure sequence

Table `t` has one record `r_bad` whose stored bytes fail to decode via
`InnerValue::from_bytes` (corrupted block, or written by a different codec
version). `CREATE INDEX idx_name ON t(name)` silently skips `r_bad` during
backfill and marks the index `Ready`. A later
`SELECT * FROM t WHERE name = <r_bad's value>` gets planned through the
index (`try_plan_and_index_scan`) — the row is NEVER returned, even though
it physically exists and a full scan would find it. This is a silently
wrong query result — worse than the unique-index case R0-D already fixed
(that one allows a duplicate; this one silently drops rows from query
results).

Sorted backfill (`crates/shamir-engine/src/table/table_manager_sorted_index.rs`,
currently around `:201-210`) is ALREADY fail-closed (`return Err(...)`), so
after this fix, no family will remain inconsistent.

### Fix

The exact same pattern as the already-fixed unique-index site: replace
`Err(_) => continue` with `.map_err(|e| ...DbError::Codec(...))?` — copy the
error message tone/structure from `index_manager_unique.rs`'s existing fix
(grep for the fail-closed message it constructs; mirror it, adjusted for
"regular" instead of "unique" wording). Write a test-twin of whatever test
`crates/shamir-index/src/base_index/tests/index_manager_tests/unique_tests.rs`
already has for the unique-backfill abort (`backfill_aborts_on_malformed_key...`/
`..._undecodable_value...`, or similarly named — grep to confirm exact
names), adapted to the regular-index backfill path. Confirm it fails against
the current code, passes after the fix.

## Constraints

- Follow `CLAUDE.md` conventions (test files under existing `tests/`
  directories, no inline `#[cfg(test)] mod tests {}`, Fx-hash collections
  where applicable — neither fix should need new collections).
- Gate: `cargo fmt -p shamir-index -p shamir-engine`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-index -p shamir-engine --full`, and
  `./scripts/test.sh @oracle` must all be clean.
- Do NOT touch F-3/F-4/F-5/F-8 (tracked separately as #1029/#1030/#1031) —
  stay scoped to exactly the two defects above.
- Both fixes are small and surgical — if either grows beyond a focused
  change (a few lines + a test), stop and report why, don't improvise a
  larger redesign.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or
any git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Definition of done

- [ ] `insert_tx_many` and `insert_tx_many_bytes` both call
      `stamp_index2_provenance` on their index2 ops.
- [ ] `write_ops.rs`'s doctrine comment states the actual invariant, not a
      function-name list, and you've personally confirmed (via grep) no
      other unstamped index2-op site exists in a live path.
- [ ] A batch-path regression test proves an index2 posting survives an
      unrelated concurrent index2 CREATE between stage and commit —
      confirmed to fail against the pre-fix code.
- [ ] Regular index backfill aborts (not silently skips) on a malformed key
      or undecodable value, mirroring the already-fixed unique-index site.
- [ ] A regression test for the regular-backfill fix, confirmed to fail
      against the pre-fix code.
- [ ] fmt/clippy/tests green (report exact commands and pass/fail).
