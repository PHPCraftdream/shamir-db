# Fix silent lost-update: `execute_update_tx` must merge over already-staged tx bytes

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context — a confirmed, real, silent data-corruption bug

Discovered during release-audit campaign cleanup (task #695, investigation
completed — read that task's findings; this brief repeats the essential
parts). **This is not a hypothetical or edge case — it is a confirmed
mechanism for silently losing a write, with no error, no log.**

**The bug, precisely:** `execute_update_tx`
(`crates/shamir-engine/src/table/write_exec.rs:371-499`+) collects matched
rows as `matched: Vec<(RecordId, Bytes)>` by scanning the COMMITTED store
(`lookup_records_via_index` or `list_stream` — both explicitly documented
as blind to this SAME tx's own uncommitted staged writes, see
`table_manager_streaming.rs:165-175`'s "KNOWN LIMITATION" comment). Then,
per matched row (the loop starting at `write_exec.rs:521`):

```rust
for (id, old_bytes) in &matched {
    let new_bytes = merge_storage_bytes(old_bytes, set_map)?;   // line 524
    let changed = new_bytes.as_ref() != old_bytes.as_ref();      // line 529
    ...
    self.update_tx_bytes(*id, old_bytes, new_bytes.clone(), &mut *tx).await?;  // line 586-587
```

`old_bytes` here is ALWAYS the pre-tx, committed-store snapshot — even if
THIS SAME transaction already staged a DIFFERENT value for this exact row
(e.g. via FK `ON UPDATE CASCADE`/`SET NULL` fan-out,
`apply_fk_update_plan` in `crates/shamir-engine/src/query/batch/
query_runner.rs`, applied BEFORE `execute_update_tx` runs for a later op in
the same batch — see lines ~1037/1073). `update_tx_bytes`
(`crates/shamir-engine/src/table/table_manager_tx_ops.rs:846-935`+)
eventually calls `stage_mutation` → `StagingStore::set`
(`crates/shamir-tx/src/staging_store.rs:135-137`), which does an
**unconditional map insert** — whatever the tx already staged for that key
is silently REPLACED, not merged.

**Concrete failure scenario**: two tables A/B, FK from A to B with
`ON UPDATE CASCADE`/`SET NULL`. A batch contains (1) an UPDATE on A that
changes the referenced value, cascading a `PendingMutation::UpdateField` to
a row R in B, staged into this tx's `write_set`; (2) a SEPARATE UPDATE on B
in the SAME batch whose own `WHERE` also matches row R. Op (2)'s
`execute_update_tx` scans R from the COMMITTED store (pre-cascade), merges
its own `set_map` on top of that stale snapshot, and calls
`update_tx_bytes` — which overwrites op (1)'s already-staged cascade result
entirely. The caller gets a success response; one of the two intended
mutations to R has vanished with no trace.

**No existing test covers this.** `crates/shamir-engine/src/query/batch/
tests/fk_on_update_tests.rs`'s
`self_ref_on_update_self_loop_overlapping_parent_and_child`
(lines ~1613-1707) was deliberately redesigned to AVOID this exact overlap
rather than exercise it — read it for context, then read task #695's full
investigation notes (via `TaskGet` on task #695, or ask the orchestrator to
paste them) for the complete reasoning trail.

## Chosen fix direction (decided by the orchestrator — implement exactly this)

Make `execute_update_tx`'s per-row loop **tx-staging-aware**: before
merging/staging a row, check whether THIS tx already has a staged value for
that row's key, and if so, treat THAT as the "old" state for both the merge
and the downstream index-delta planning — never the stale committed-store
scan.

1. **Resolve the effective "old bytes" per row, inside the loop**
   (`write_exec.rs`, right at the top of the `for (id, old_bytes) in
   &matched` loop, before the `merge_storage_bytes` call at line 524):
   ```rust
   // Read this tx's own staging for this row FIRST — a prior op in the
   // SAME batch/tx (e.g. FK cascade fan-out) may have already staged a
   // different value here that the `matched` scan (committed-store only,
   // blind to this tx's write_set — see table_manager_streaming.rs's
   // "KNOWN LIMITATION" doc) never saw.
   let staged = tx.write_set.get(&self.table_token())
       .and_then(|staging| staging.staged_op(id.to_bytes().as_ref()));
   let effective_old: std::borrow::Cow<'_, [u8]> = match staged {
       Some(shamir_tx::staging_store::StagedKind::Set(staged_bytes)) => {
           std::borrow::Cow::Owned(staged_bytes.to_vec())
       }
       Some(shamir_tx::staging_store::StagedKind::Removed) => {
           // This tx already staged a DELETE for this row (e.g. a prior
           // cascade DELETE, or a same-batch DELETE op). Decide the
           // correct semantics here — see "Removed-row semantics" below —
           // do NOT assume; investigate and justify your choice.
           /* ... */
       }
       None => std::borrow::Cow::Borrowed(old_bytes.as_ref()),
   };
   let new_bytes = merge_storage_bytes(&effective_old, set_map)?;
   let changed = new_bytes.as_ref() != effective_old.as_ref();
   ```
   Check the EXACT method names/types before using them —
   `StagingStore::staged_op` (`staging_store.rs:127-132`) returns
   `Option<StagedKind>`; `StagedKind::Set(Bytes)` / `StagedKind::Removed`
   (`staging_store.rs:63-68`); confirm how `StagedRow`/`Bytes` convert to
   `&[u8]`/`Vec<u8>` for the `Cow` construction (check `StagedRow::as_bytes`
   at `staging_store.rs:36-38` — `staged_op` already unwraps this into a
   `Bytes`, so `.to_vec()` or a `Bytes`-aware `Cow` variant, whichever is
   idiomatic here — check how `read_one_tx_bytes`
   (`table_manager_streaming.rs:482-540`, the existing read-your-own-write
   precedent for POINT reads — read this function in full, it is the
   closest existing pattern to mirror) handles the `Bytes` type to keep
   this consistent).
2. **Use `effective_old` (not the raw `old_bytes` scan result) EVERYWHERE
   else in the rest of the loop body** that currently reads `old_bytes` —
   this includes:
   - the validator block's `RecordView::new(old_bytes)` (line ~552) and the
     `old_qv` it builds (line 557) — a validator running on a row that was
     ALREADY modified by a same-tx cascade should see the cascade's result
     as "old", not the pre-cascade committed value;
   - the `self.update_tx_bytes(*id, old_bytes, new_bytes.clone(), &mut
     *tx)` call (line 586-587) — **this is the most important one**:
     `update_tx_bytes` uses its `old_bytes` parameter to compute the
     INDEX-DELTA ops (`plan_update_ops_ref`/`plan_legacy_update_ops_ref`,
     table_manager_tx_ops.rs:846-935 — read this function in full) and
     unique-constraint validation. If this receives the STALE committed
     snapshot instead of the tx's own already-staged value, the index delta
     it computes/stages would span BOTH changes at once (committed →
     final) — but a PRIOR call to `update_tx_bytes` for the SAME row
     earlier in this SAME tx (from the cascade's own apply path) already
     staged index ops for committed → cascade-result. Feeding the stale
     `old_bytes` again here would double-count / conflict with those
     already-staged index ops. Feeding `effective_old` (the cascade result)
     instead makes THIS call's index delta correctly incremental
     (cascade-result → final), composing correctly with the earlier call.
   - the RETURNING-record-building code further down the function (read
     past line 600 to find it) — it should build the returned "old" record
     from the SAME `effective_old` state for consistency, if it currently
     re-reads `old_bytes`.
3. **Removed-row semantics (investigate, don't assume)**: if THIS tx
   already staged a `Remove` for this row's key (e.g. a same-batch DELETE,
   or a cascade `ON UPDATE ... ` path that somehow also deletes — check
   whether that's even reachable for an UPDATE op specifically, or only
   relevant for the DELETE-side equivalent of this bug class, which is OUT
   OF SCOPE for this brief), the correct behavior is almost certainly to
   treat the row as NOT MATCHED by this UPDATE (the row no longer exists
   from this tx's point of view) — i.e. skip it entirely (don't merge, don't
   stage, don't count it in `affected`). Confirm this against any existing
   test that already covers "DELETE then UPDATE the same row in one batch"
   (grep for it) before deciding; if no such test exists, add one alongside
   your fix proving the skip behavior.
4. **Do NOT touch `StagingStore::set`'s unconditional-overwrite semantics**
   — that is correct and necessary in general (a tx's LATEST intent for a
   key should always win when explicitly staged); the bug is entirely in
   `execute_update_tx` feeding it a STALE base to merge from, not in the
   staging store's overwrite behavior itself. Do not add merge logic to
   `StagingStore::set` — keep the fix scoped to `execute_update_tx`'s own
   per-row resolution of what "old" means.
5. **Check `execute_delete_tx` and any other tx-mutating op in this same
   file/module for the SAME class of bug** (scans committed store, computes
   a delta, stages unconditionally) — report what you find, but do NOT fix
   anything beyond `execute_update_tx` in this task unless it's a trivial,
   obviously-identical one-line fix; a broader sweep across every write path
   is a separate task if warranted.

## The mandatory regression test — reproduce the EXACT scenario from the investigation

Add a new test (in `crates/shamir-engine/src/query/batch/tests/
fk_on_update_tests.rs`, alongside the existing self-loop test, or a new
file if that one is already large — check its current size first) that:
- Defines two NON-self-referential tables A and B, with an FK from A to B
  declared `ON UPDATE CASCADE` (or `SET NULL` — pick whichever is simpler
  to set up given the existing test helpers in that file).
- Inserts a row in B (row R) and a row in A that references R.
- Runs a SINGLE batch/tx containing BOTH: (a) an UPDATE on A that changes
  the referenced value (triggering the cascade fan-out to update some
  field on R), AND (b) a SEPARATE UPDATE on B whose own `WHERE` ALSO
  matches R, setting a DIFFERENT field on R.
- Asserts that AFTER the batch commits, row R has BOTH the cascaded field
  change AND the directly-set field change present — proving neither
  mutation was lost. **Before your fix, this test must FAIL** (reproducing
  the bug) — confirm this yourself (e.g. by temporarily checking out your
  fix's absence, or reasoning through why the current code would fail it)
  before finalizing; after your fix, it must PASS.
- Also add the "DELETE then UPDATE same row in one batch" test from step 3
  above if you couldn't find an existing one covering it.

## Verification (MANDATORY before you report done)

- The new regression test(s), demonstrated to fail without the fix and
  pass with it.
- `./scripts/test.sh -p shamir-engine -p shamir-query-types --full` green
  — run it TWICE (this session's flake-triage discipline: a transient
  full-suite flake unrelated to your diff should reproduce as a PASS on a
  second run). Every existing FK cascade/UPDATE/DELETE test must still pass
  UNCHANGED.
- `cargo fmt -p shamir-engine -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.
- Report what you found in step 5 (the sweep for the same bug class in
  `execute_delete_tx`/other write paths) even if you didn't fix it.
- Report your decision + reasoning for the "removed-row" semantics
  (step 3).

## Out of scope

- Do NOT change `StagingStore::set`'s semantics — see point 4 above.
- Do NOT fix `execute_delete_tx` or any other write path beyond
  `execute_update_tx` unless it's a trivial one-line mirror of this exact
  fix — report findings instead, per step 5.
- Do NOT touch tasks 8a-8f's artifacts (Этап 8, already landed and
  unrelated) or task #715 (redb README rewrite, unrelated).
