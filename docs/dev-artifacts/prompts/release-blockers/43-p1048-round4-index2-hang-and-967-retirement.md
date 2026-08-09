# Brief — #1048 round 4: fix the index2 DROP crash-recovery test hang; retire #967 error-TEXT sites

## Context

Continuation of #1048 (P1-2 sub-slice B). Round 1 fabricated two of three
required deliverables (caught via `git diff`). Round 2 genuinely implemented
the production logic for all three families (hash DROP, hash RENAME, index2
DROP) but wrapped the hash-DROP/index2-DROP status writes in `tokio::spawn`
(fire-and-forget race). Round 3 fixed the race (now synchronous, awaited
inline in `TableManager::create` — confirmed correct by reading the diff) and
added the two missing e2e tests the brief required — but:

1. The index2 DROP test (`p1048_e2e_index2_drop_op_id_recovery`) is marked
   `#[ignore = "...needs investigation"]` because it **hangs**. I reproduced
   this myself, independent of round 3's own report:
   ```
   ./scripts/test.sh -p shamir-engine --full -- --run-ignored all p1048_e2e_index2_drop_op_id_recovery
   ```
   Real, reproducible result: `TIMEOUT [180.293s] (1/1) shamir-engine
   table::tests::p1048_index2_drop_durability_tests::p1048_e2e_index2_drop_op_id_recovery`
   — a genuine nextest kill, not a flake.
2. Brief item 3 (retire the `#967` free-text `DbError::Internal` error sites
   in the three families to the structured `DdlOpState::Failed { detail }`
   shape #1015 already established) was **never touched in any of the three
   rounds** — confirmed via `grep -rn "DdlOpState::Failed"
   crates/shamir-index/src/base_index/index_manager.rs
   crates/shamir-engine/src/table/table_manager_index_mgmt.rs
   crates/shamir-engine/src/table/table_manager.rs` returning zero hits. The
   `#967` comments in those files are all still pointing at the old
   `DbError::Internal(format!(...))` enrichment pattern, unchanged.

This brief covers BOTH gaps. Do not re-report the round-2 production logic
or the round-3 race fix — those are independently verified correct and out
of scope here.

## Gap 1 — index2 DROP crash-recovery test hangs

### What I already ruled out

I read the relevant code myself before writing this brief; do not re-derive
these from scratch, use them as a starting point:

- `TableManager::create`'s per-instance fields (`ddl_admission`,
  `unique_write_lock`, `index2_registry`) are all constructed FRESH by every
  `TableManager::create(...)` call — a NEW `Arc<Mutex<..>>`/`Arc<...>` each
  time. An abandoned task holding a guard on the OLD instance's mutex cannot
  block the NEW instance's identically-named-but-distinct mutex. This rules
  out a naive "same lock, two instances" theory.
- There is no process-wide singleton/registry in `table_manager.rs` keyed by
  table name or store identity that could make a second `TableManager::create`
  call wait on the first instance's teardown.
- `index2_backfill_hook::BackfillPauseHook::wait_at_window` parks on
  `tokio::sync::Notify::notified().await` — a genuine async wait, not a
  busy-spin. This rules out runtime starvation on a current-thread executor
  from a hot loop.

### The one structural difference I found, worth checking FIRST

The test that induces the hang
(`crates/shamir-engine/src/table/tests/p1048_index2_drop_durability_tests.rs`)
cancels the paused `drop_index2` call by **spawning it and then dropping the
`JoinHandle`**:
```rust
let drop_task = tokio::spawn(async move { mgr_c.drop_index2(index_name).await.unwrap(); });
pause_hook.wait_until_parked().await;
drop(drop_task);   // does NOT cancel the spawned task — it keeps running, forever parked
```
Dropping a `JoinHandle` does **not** abort the underlying task (that's what
`JoinHandle::abort()` is for) — the spawned future is still alive on the
runtime, forever parked at the never-released hook, forever holding whatever
its local stack holds at that point (including `backend`, the removed-but-
not-yet-swept index2 backend, and `drain_guard` — the reader-drain-gate RAII
guard from `self.index2_registry.reader_gate().begin_drop()`, taken on the
OLD `TableManager` instance and never dropped).

An **existing, already-passing sibling test in the same family** —
`p03b_index2_live_drop_crash_at_post_sweep_hook` in
`crates/shamir-engine/src/table/tests/p03b_index2_drop_durability_tests.rs`
(around line 320) — simulates the identical "crash mid-drop" scenario
differently: it runs `drop_index2` INLINE inside a `tokio::select!` arm and
lets the OTHER arm (the pause-hook's `wait_until_parked()`) win the race,
which drops (cancels) the `drop_index2` future in place:
```rust
tokio::select! {
    _ = mgr_c.drop_index2("lower_name") => { panic!(...) }
    _ = hook.wait_until_parked() => { /* parked; drop_index2 future is now cancelled */ }
}
drop(mgr_c);
drop(mgr);
```
This actually drops every local (`backend`, `drain_guard`, etc.) that
`drop_index2`'s stack was holding at the pause point — a REAL simulated
crash, not a permanently-alive zombie task.

**Try rewriting `p1048_e2e_index2_drop_op_id_recovery` (and the unique-family
sibling, if a second one is needed — check whether the brief's DoD requires
one; the hash-family tests cover both regular and unique, index2 does not
have a separate unique variant AFAICT, confirm) to use the SAME
`tokio::select!` cancellation pattern as `p03b_index2_live_drop_crash_at_post_sweep_hook`
instead of `tokio::spawn` + abandoned `JoinHandle`.** This is the more
faithful simulation of an actual process crash (a real crash does not leave
a zombie task consuming Rust-level resources — the whole process dies), so
it is also the objectively more correct test authoring choice, not just a
plausible fix.

Run it after the rewrite. If it now passes cleanly and repeatably (run it 3
times in a row to rule out a timing flake), the root cause was the
`tokio::spawn`-abandonment test-authoring bug (not a production defect) —
un-ignore it, done.

### If that does NOT fix it

Do not paper over it and do not re-disable it. A hang that survives a
correct crash-simulation rewrite is a genuine production bug (something in
`recover_index2_drops` / `write_index2_drop_recovery_status` / the interner
/ index2 registry construction sequence in `TableManager::create` is not
resilient to the state a real crash could leave behind). In that case:
diagnose with real evidence (attach a debugger equivalent — e.g. add
temporary `log::info!` breadcrumbs around each `.await` point in the new
`TableManager::create` index2-recovery block and re-run to see exactly where
progress stops; or bisect by commenting out the round-2/round-3 `#1048`
additions one at a time to find which specific new `.await` never resolves),
find the actual root cause, and fix the production code. Report the root
cause with evidence (not a guess) in your final summary either way.

**Never raise the nextest timeout to paper over this. Never leave the test
`#[ignore]`d as a final state.** Per `CLAUDE.md`: "Hangs and test-locks are
BUGS — hunt and fix them, never tolerate."

## Gap 2 — retire #967 free-text error sites to structured `DdlOpState::Failed`

Every `#967`-tagged comment site in these three files currently documents
(but does not implement) enrichment via the OLD free-text
`DbError::Internal(format!(...))` pattern:
- `crates/shamir-index/src/base_index/index_manager.rs` (multiple sites,
  `grep -n "#967"` to enumerate — includes both DROP and RENAME failure
  paths for the hash family).
- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` (RENAME
  failure paths).

For each site that represents a DDL operation (DROP/RENAME) failing in a way
that is NOT recovered (i.e., the operation is genuinely stuck/broken, not a
transient retryable error) and where an `op_id` is available (threaded from
round 2/3's work) — write `DdlOpState::Failed { detail: <the same enriched
message text> }` to the op-status log via
`crate::table::ddl_op_log::write_op_status` (same function the
`SucceededViaCrashRecovery` writes already use), in addition to (not instead
of) returning the existing `Err(DbError::Internal(...))` — the caller still
needs the synchronous error; the op-status log is for a client that's
polling `GetDdlOpStatus` asynchronously and would otherwise see `Unknown`
forever for an op that actually failed.

Check `DdlOpState::Failed`'s exact shape first:
`crates/shamir-query-types/src/read/ddl.rs` (`Failed { detail: String }`,
already confirmed to exist).

If a particular `#967` site has no `op_id` available in scope (e.g., it's
deep inside a helper that wasn't part of the op_id-threading work), it is
acceptable to skip it — note which ones you skipped and why in your summary,
do not silently drop coverage.

### Test for this gap

A test that forces one of these failure paths (e.g., inject a persist
failure via whatever fault-injection seam the existing `#967` tests already
use — grep for existing tests around these call sites first, there is very
likely a fault-injection store wrapper already used by `p997_*`/`p03b_*`
tests) and asserts the op-status log now holds `DdlOpState::Failed {
detail }` with the original enriched message in `detail`, not just that
`Err` was returned. Assert the structured shape, not just "some error
occurred."

## Constraints

- Follow `CLAUDE.md`: `Result<T, E>` conventions, tests in `tests/`
  directories, imports at top of file, one-file-one-primary-export.
- Gate: `cargo fmt -p shamir-index -p shamir-engine -p shamir-db -p
  shamir-query-types -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `./scripts/test.sh -p shamir-index -p shamir-engine -p
  shamir-db -p shamir-query-types --full`. Use the wrapper, never raw
  `cargo test`/`cargo nextest run`.
- The two hash-DROP tests from round 3
  (`p1048_hash_drop_durability_tests.rs`) already pass — do not touch them
  unless you find they need the same `tokio::select!` treatment for
  consistency (check, but they are not currently broken so this is
  optional/cosmetic, not required).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.
⛔ Do not create scratch files at the repo root.

## Definition of done

- [ ] `p1048_e2e_index2_drop_op_id_recovery` is NOT `#[ignore]`d, runs and
      passes repeatably (3x in a row), and the fix's root cause (test-
      authoring bug vs. genuine production bug) is stated with evidence in
      the summary.
- [ ] If a genuine production bug was found, it is fixed in production code
      (not worked around in the test).
- [ ] All identified `#967` sites in the three families either write
      `DdlOpState::Failed { detail }` alongside the existing `Err` return, or
      are explicitly listed as skipped with a stated reason (no `op_id` in
      scope).
- [ ] At least one test asserts the structured `Failed { detail }` shape is
      written to the op-status log on a real failure injection, not just
      that an `Err` was returned.
- [ ] fmt/clippy/test gates green, real output reported (paste the actual
      nextest summary line, not a paraphrase).
