Task: HIGH-perf — `execute_update_tx` unconditionally de-interns
old/new record bytes into `QueryValue` maps on EVERY changed row, even
when zero Update validators are registered, just to call
`run_validators_qv` which returns instantly with no validators
(audit top-5 #3, `docs/dev-artifacts/audits/2026-07-06-perf-hot-paths.md` §1.3).

## Where

`crates/shamir-engine/src/table/write_exec.rs`, `execute_update_tx`
(the per-row loop starting ~line 508, the `if changed { ... }` block
at ~521-560), and the analogous MERGE branch in `execute_set_tx`
(~894-899, 942-969 per the audit — confirm exact current line numbers,
they may have drifted since 2026-07-06).

```rust
if changed {
    // Build old_qv (full RecordView de-intern) + new_qv (deep clone +
    // overlay) UNCONDITIONALLY...
    let old_view = RecordView::new(old_bytes)...;
    let old_qv = record_view_to_query_value(&old_view, interner)?;
    let new_qv = {
        let mut m = match old_qv.clone() { ... };   // deep clone
        // ... overlay resolved_set onto it
        QueryValue::Map(m)
    };
    self.run_validators_qv(
        WriteOp::Update, Some(&new_qv), Some(&old_qv), &Actor::System, Some(tx), resolver,
    ).await.map_err(validator_failure_to_db_error)?;
    // ...
}

if wants_records {
    // ... builds ANOTHER old_view/base_qv from old_bytes AGAIN
    // (second full de-intern of the SAME bytes, when RETURNING is requested)
}
```

Compare with `execute_delete_tx` (same file, ~line 620-635), which
ALREADY does this correctly:

```rust
// Check whether any Delete validators are registered BEFORE the scan
// so we know whether to carry bytes alongside ids.
let has_delete_validators = {
    let bindings = self.validator_bindings();
    bindings.iter().any(|b| b.ops.contains(&WriteOp::Delete))
};
let keep_bytes = has_delete_validators || wants_records;
```

`run_validators_qv` (`crates/shamir-engine/src/table/table_manager_validators.rs:150`)
returns instantly when zero validators are bound for the given
`WriteOp` — so on a table with no Update validators (the common case),
`execute_update_tx` still pays for a full `RecordView`-based de-intern
(String allocation per key + owned values) PLUS a deep `.clone()` of
the resulting map, on EVERY changed row, purely to call a function that
does nothing.

## Why this is HIGH

Per the audit: 2-3 full record materializations per row that are
completely wasted when there are no Update validators (the common
case). On a mass UPDATE (thousands of rows), this is pure overhead.
When `wants_records` (RETURNING) is ALSO true, `old_bytes` gets
de-interned a SECOND time (once for validators, once for the result) —
even though the SAME bytes were already de-interned moments earlier.

## Fix

1. **Hoist a `has_update_validators` check before the per-row loop**,
   mirroring `execute_delete_tx`'s `has_delete_validators` exactly:
   ```rust
   let has_update_validators = {
       let bindings = self.validator_bindings();
       bindings.iter().any(|b| b.ops.contains(&WriteOp::Update))
   };
   ```
2. **Gate the old_qv/new_qv construction + `run_validators_qv` call on
   `has_update_validators`** — only do that work (and only call
   `run_validators_qv`) when there's at least one registered Update
   validator. When `has_update_validators` is false, skip straight to
   staging the write (`self.update_tx_bytes(...)`).
3. **When RETURNING is also requested (`wants_records`) AND validators
   ran** (so `old_qv` was already built), **reuse that `old_qv`** for
   the result-record construction instead of re-deriving it from
   `old_bytes` via a second `RecordView::new` + `record_view_to_query_value`
   call. When validators did NOT run (no update validators bound) but
   RETURNING is still requested, you still need exactly ONE de-intern
   for the result — that's unavoidable and already correctly scoped
   to only `wants_records` rows.
4. **Apply the analogous fix to the MERGE/upsert branch in
   `execute_set_tx`** (the audit calls out lines ~942-969 in the same
   file — re-locate the exact current lines; the shape is the same:
   an unconditional validator-prep de-intern that should gate on
   whether Update validators exist for the upsert-update path).

Do NOT change the CHANGE-DETECTION logic (`changed = new_bytes.as_ref()
!= old_bytes.as_ref()`) — that's unrelated and already cheap (byte
comparison, no de-intern). Only the validator-prep + RETURNING
de-intern paths are in scope.

## TDD requirement

1. **Red**: write a test in `crates/shamir-engine/src/table/tests/`
   (check existing `write_exec`/`update`-related test module structure
   first, e.g. `table_manager_tests.rs` or similar per this project's
   test-organization convention) that:
   - Confirms CORRECTNESS is unchanged: an UPDATE on a table with a
     registered Update validator still runs it (validator sees
     correct old/new values) — do not silently break validator
     enforcement while optimizing the no-validator path.
   - Confirms CORRECTNESS on a table WITHOUT Update validators:
     `UPDATE ... RETURNING` still returns the correct post-update
     field values (proves the reused/single de-intern path produces
     the same result as before).
   - If feasible, add or extend a bench-visible regression: check
     whether `crates/shamir-db/benches/engine_perf.rs` (or another
     already-normalized bench in this workspace — this session
     normalized ALL benches to `bench-scale-tool`, ≤10ms/call) already
     has a mass-UPDATE workload; if one exists, confirm it still
     passes and note any measured improvement in your report. Do NOT
     add a NEW bench workload for this — the audit notes "массовый
     UPDATE... engine_perf покрывает только точечный update_by_id",
     so a full new mass-update bench is a bigger undertaking than this
     task's scope; a plain correctness unit test proving the
     optimization doesn't break validator semantics is the primary
     deliverable here.
2. **Green**: implement the fix.
3. Confirm existing `write_exec`/UPDATE-related tests still pass.

## Test scope command

```
./scripts/test.sh -p shamir-engine
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- Whether `has_update_validators` was hoisted in both `execute_update_tx`
  and the `execute_set_tx` MERGE branch (or just one — state why if
  only one).
- Whether the RETURNING path was changed to reuse the validator-built
  `old_qv` when validators ran (vs. always re-deriving it).
- The correctness tests you wrote/extended and their pass/fail
  before/after.
- Whether any existing bench exercises a mass-UPDATE path and what it
  shows (if applicable) — or confirm none exists and this wasn't
  measured quantitatively.
- Gate results (exact commands + pass/fail).
