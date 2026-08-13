# #1110 HIGH — #1107's staleness gate has a genuine false-negative window (Phase 5a/5c commit vs last_committed publish gap)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

Found by a follow-up adversarial `@oh` review of the `#1105`-`#1109` remediation
wave (commit range `59e8c532..HEAD`). Confirmed personally by reading
`crates/shamir-engine/src/tx/materialize.rs` before this brief was written —
this is a real correctness regression in `#1107`'s own fix, not a false
alarm.

File: `crates/shamir-engine/src/tx/pre_commit.rs` (~lines 1879-1892, the
`#1107` gate inside `rederive_stale_value_ops_post_stage`). File:
`crates/shamir-engine/src/tx/materialize.rs` (the phase ordering that breaks
the gate's soundness claim).

## The bug

The gate's own comment claims: *"Conservative: a commit on a DIFFERENT table
still defeats the fast path (false positive), but never wrong (no false
negatives)."* This is **false**.

`tx.snapshot_version` is compared against `repo.tx_gate().last_committed()`.
But `last_committed_version` is only advanced by `version_guard.commit()`
at `materialize.rs:248` (Phase 6) — which runs AFTER:

- **Phase 5a** (data writes, `materialize.rs:76-102`) and **Phase 5c**
  (index/posting writes, `materialize.rs:143-181`) have already landed and
  are MVCC-visible via `mvcc.get_current_bytes()` — the exact read
  `tbl.read_one_tx_bytes(rid, None)` uses. This read is NOT clamped to
  `last_committed`; it reads the LATEST version.
- `drop(uwl_guards)` at `materialize.rs:204` — the per-table unique-write-lock
  is released here, so any other transaction waiting on it can now proceed.
- `apply_vector_delta_phase(tx, repo, commit_version).await` at
  `materialize.rs:226` — a REAL `.await`, meaning genuine wall-clock time
  elapses in this window on real I/O, not just a handful of instructions.

### Concrete reproduction sketch

```
1. tx0: INSERT R{email:"y"}; COMMIT -> v1.
2. T1 BEGIN at v1 (snapshot_version=v1); stages DELETE R
   -> RemovePosting("y", owner=R).
3. T2: UPDATE R SET email="z"; commits -- reaches Phase 5c (data
   {email:"z"} and posting "z"->R written, "y" removed), drops uwl_guards,
   is inside apply_vector_delta_phase().await. last_committed is STILL v1
   (T2 hasn't reached version_guard.commit() yet).
4. T1 enters pre_commit_prelock, reaches the #1107 gate:
   last_committed()==v1==snapshot_version -> INCORRECTLY concludes "quiet
   repo, nothing changed" -> SKIPS rederive_stale_value_ops_post_stage
   entirely.
5. T1's own Step 1 sees the stale RemovePosting("y") with no live claim ->
   checks durable state -> "y" is already free (T2 already removed it) ->
   treated as a harmless no-op, kept as-is.
6. T1 commits, deletes R's row. Nothing ever removes the "z"->R posting T2
   wrote, because the gate skipped the re-derivation that would have
   caught it.
```

Result: a dangling unique posting `"z"->R` for a now-deleted record —
exactly the bug class `#1100`/`#1106` exist to prevent. This interleaving
does NOT require T1 to wait on any lock at all on a table with ONLY regular
indexes (`needs_write_barrier() == false`, no `unique_write_lock` taken) —
there the window is the WHOLE Phase-5c-to-Phase-6 span, not just the
uwl-guard-holding portion.

## Fix direction

Gate on the version **allocation** high-water mark instead of the
**publication** watermark. `RepoTxGate::version_counter`
(`crates/shamir-tx/src/repo_tx_gate.rs`) is bumped by
`assign_next_version_guarded` **before** any write happens, on BOTH the
tx-commit path (`pre_commit.rs`) and the non-tx path
(`mvcc_store/mod.rs`). Comparing `tx.snapshot_version` against
`version_counter`'s current value (NOT `last_committed`) is a genuine "no
writer has even STARTED a commit since I opened" proof — no writer can have
landed Phase 5a/5c writes without first having allocated a version.

Required steps:

1. Read `RepoTxGate` in full first (`repo_tx_gate.rs`) — understand
   `version_counter`'s exact role, every place it's read/written, and what
   ordering guarantees currently exist around it, before touching anything.
2. Add a new read-only accessor exposing `version_counter`'s current value —
   do NOT expose any way to bump it externally, this is a read-only gate
   check.
3. The `fetch_add` on `version_counter` currently uses `Ordering::Relaxed`.
   For the gate's happens-before argument to hold FORMALLY (not just
   "probably works in practice"), this likely needs strengthening to
   `AcqRel`/`Release` on the writer side, with a matching `Acquire` load in
   the new accessor. Investigate what else in the codebase depends on
   `version_counter`'s current ordering before changing it — do not weaken
   any existing guarantee elsewhere. If a stronger ordering is genuinely
   unnecessary for THIS gate's correctness for some reason you can prove,
   say so explicitly with the proof rather than changing it speculatively.
4. Re-derive and write out the happens-before proof explicitly in a comment
   at the gate site, mirroring this codebase's established style for such
   proofs (e.g. `writer_drain_barrier.rs`'s module doc, or `pre_commit.rs`'s
   own existing comments around `#1097`/`#1098`'s ordering fixes) — show WHY
   the invariant holds, don't just assert it.

## Required test

Construct a deterministic reproduction using this codebase's established
pause-seam pattern (e.g. `PostPrelockPreMaterializeHook` in `pre_commit.rs`,
or a NEW seam parking a transaction strictly between Phase 5c landing and
`version_guard.commit()` in `materialize.rs`) that drives the EXACT
interleaving above and proves:

- With a deliberately-reverted-to-`last_committed`-based gate (simulating
  the current bug), the dangling posting reproduces.
- With the fixed (`version_counter`-based) gate, it does not.

This is the standard this session's zero-trust discipline requires — do
not accept "looks right" without a real Red-then-Green mutation test for
this specific race. A test that merely re-checks the existing
`p1100_stale_snapshot_delete_posting.rs` scenarios is NOT sufficient (those
don't hit this specific timing window) — this needs a genuinely new,
timing-controlled reproduction.

Also re-run `p1100_stale_snapshot_delete_posting.rs` (6 tests) in full to
confirm the fix doesn't regress the legitimate fast-path skip (a genuinely
quiet repo — nothing has even started committing — must still skip the
per-row work).

## Also address (LOW, same review pass, same area — fold in since you'll already be touching this bench)

`crates/shamir-engine/benches/p1107_stale_value_gate.rs` updates the
`"score"` field, which is NOT indexed by either index the bench creates
(`idx_email` on `email`, `uniq_id` on `id`) — `plan_record_updated`/
`plan_record_updated_unique` both return EMPTY vectors for this workload,
so the `O(N^2)` dedup loops this whole area exists to bound are NEVER
exercised by this bench at all. This is the real reason `#1107`'s own bench
claim was noise-dominated — not merely "most of the batch's cost is
elsewhere" (the CHANGELOG's current framing), but "the benefit is
unobservable by this bench BY CONSTRUCTION." Fix the bench to update an
INDEXED field (e.g. the unique `id` or the indexed `email` field) so it
actually measures what it claims to. Re-run and report corrected
before/after numbers for the NEW (`version_counter`-based) gate. Correct the
CHANGELOG's `#1107` entry and this bench's own doc comment to state the
real reason plainly.

## Gate

```
cargo fmt -p shamir-engine -p shamir-tx -- --check
cargo clippy -p shamir-engine -p shamir-tx --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-tx --full
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench -p shamir-engine --bench p1107_stale_value_gate
```

(Use forward slashes for `CARGO_TARGET_DIR` on this system — backslashes get
mis-parsed by the shell here.)

All must pass clean. Report: the exact happens-before proof for the new
gate; whether the new pause-seam reproduction test genuinely went red
(pre-fix) then green (post-fix); real gate pass/fail counts; corrected
bench numbers with the fixed indexed-field workload.
