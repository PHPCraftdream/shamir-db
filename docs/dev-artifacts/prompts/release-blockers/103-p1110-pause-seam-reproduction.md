# #1110 follow-up — deterministic pause-seam reproduction for the version-allocation gate fix

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

`#1110`'s main fix already landed (committed): `crates/shamir-engine/src/tx/pre_commit.rs`'s
staleness gate in `rederive_stale_value_ops_post_stage` now compares
`tx.snapshot_version` against `repo.tx_gate().version_allocation_high_water_mark()`
instead of `last_committed()`, closing a false-negative window where a
concurrent committer's Phase 5a/5c writes could be MVCC-visible before
`last_committed` advanced (which only happens at Phase 6). The memory
ordering was also corrected (`assign_next_version`'s `fetch_add` is now
`AcqRel`, not `Relaxed`, in `crates/shamir-tx/src/repo_tx_gate.rs`) — read
both files' updated doc comments in full for the exact happens-before
argument already written.

**What's missing**: the original brief for this fix required a
deterministic pause-seam reproduction test proving the OLD
(`last_committed`-based) gate genuinely produces a dangling posting under a
specific timing window, and the NEW (`version_allocation_high_water_mark`-based)
gate does not. This was not built — this task is specifically to build it.
Do not re-do the gate fix itself; it is already correct and committed. Your
job is ONLY the reproduction test.

## The interleaving to reproduce

```
1. tx0: INSERT R{email:"y"}; COMMIT -> v1.
2. T1 BEGIN at v1 (snapshot_version=v1); stages DELETE R
   -> RemovePosting("y", owner=R).
3. T2: UPDATE R SET email="z"; commits -- reaches Phase 5c (data
   {email:"z"} and posting "z"->R written, "y" removed), then PARKS
   strictly after Phase 5c / drop(uwl_guards) and strictly before
   version_guard.commit() (Phase 6) -- i.e. somewhere inside or around
   apply_vector_delta_phase()'s .await in materialize.rs.
4. T1 enters pre_commit_prelock, reaches the gate. With the seam parking T2
   exactly here, last_committed() is STILL v1 (unchanged) -- the OLD gate
   would incorrectly skip. version_allocation_high_water_mark() is ALREADY
   > v1 (T2's assign_next_version ran back in Phase 4, well before Phase
   5a/5c) -- the NEW gate correctly does NOT skip.
5. T1 commits. With the OLD gate: nothing removes the "z"->R posting T2
   wrote (dangling posting bug reproduces). With the NEW gate: T1's
   re-derivation correctly catches and removes it.
6. Resume T2 to let it finish committing.
```

## What to build

1. **A new test-only pause seam in `crates/shamir-engine/src/tx/materialize.rs`**,
   following the EXACT established pattern already in this codebase (read
   `pre_commit.rs`'s `PostPrelockPreMaterializeHook`/
   `TEST_POST_PRELOCK_PRE_MATERIALIZE_HOOK`/
   `fire_post_prelock_pre_materialize_test_hook` in full first — lines
   ~21-94 — and mirror its shape exactly: a `#[cfg(test)]` `OnceLock<Arc<Hook>>`
   global, a `reached: AtomicUsize` + `resume: Notify` + `armed: AtomicBool`
   one-shot handshake, zero cost when unset).
   - Name it something like `PostPhase5cPreCommitHook` /
     `TEST_POST_PHASE5C_PRE_COMMIT_HOOK` (pick a name consistent with this
     file's own phase-numbering comments — read the module doc at the top
     of `materialize.rs` first for the established Phase 5a/5b/5c/6
     terminology).
   - Fire it in `materialize()` (currently ~line 59-320) at the EXACT point
     needed: after `drop(uwl_guards); drop(drain_guards);` (~line 204-205)
     and before `version_guard.commit()` (~line 248) — the precise
     placement matters, since the whole point is to park a committer AFTER
     its writes are MVCC-visible but BEFORE `last_committed`/the OLD gate's
     signal would advance. A reasonable spot: immediately before
     `apply_vector_delta_phase(...).await` (~line 226), or immediately
     after it and before `version_guard.commit()` — either works for this
     repro since both are inside the target window; pick whichever is less
     invasive to wire in given the existing crash-seam (`maybe_crash`) calls
     already at similar points in this function as a wiring template.
2. **A new regression test** in
   `crates/shamir-engine/src/tx/tests/p1100_stale_snapshot_delete_posting.rs`
   (the established home for this bug family's tests) that:
   - Sets up the exact scenario above (tx0 insert, T1 begin, T2 update+commit
     parked at the new seam via the hook, T1 commits while T2 is parked,
     resume T2).
   - Asserts the fixed (current, `version_allocation_high_water_mark`-based)
     gate correctly removes the dangling `"z"->R` posting — i.e. asserts the
     BUG DOES NOT REPRODUCE on the current code. This is the "Green" half.
3. **Mutation-test it yourself** (this is the part that actually proves the
   test is discriminating, not vacuous): temporarily revert JUST the gate
   condition in `pre_commit.rs` back to `gate.last_committed() ==
   tx.snapshot_version` (the pre-`#1110` shape) while KEEPING your new pause
   seam and test in place, run the new test, and confirm it genuinely FAILS
   (the dangling posting reproduces) — this is the "Red" half. Then restore
   the fixed gate condition and confirm the test passes again ("Green").
   Report BOTH outcomes with real test runner output, not a paraphrase. If
   your test does NOT go red on the reverted gate, your seam placement or
   test setup is wrong — fix it until it genuinely discriminates, do not
   ship a test that passes either way.

## Gate

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
```

All must pass clean on the FINAL (fixed-gate, seam-in-place) state. Report:
the exact seam placement and why; the new test's real pass (green) output;
the real mutation-test output showing it genuinely fails (red) when the
gate is reverted to the pre-`#1110` condition; full gate pass/fail counts.
