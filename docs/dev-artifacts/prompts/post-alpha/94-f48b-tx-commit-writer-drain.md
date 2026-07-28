# Brief for F-48b (#867, P0, follow-up to F-48/#859) — wire the writer-drain barrier into the tx-commit prelock

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-48 (commit `c2cb6e13`) built a reusable `WriterDrainBarrier`
(`crates/shamir-engine/src/table/writer_drain_barrier.rs`) and wired it
into `TableManager`'s 4 **non-transactional** writer methods
(`table_manager_crud.rs`) plus `admin_schema.rs`'s schema-activation DDL
guard — closing the check-then-act race described by the 2026-07-28
review's P0-3 finding **for direct (non-tx) `TableManager` API use only**.

**Read `crates/shamir-engine/src/table/writer_drain_barrier.rs` in full
first** — the primitive already exists and works; this task REUSES it,
it does not redesign it. Read its doc comment carefully, especially the
"Why bump BEFORE the flag check" and "Slow-path writers do NOT stay in
the drain set" sections — both invariants apply identically here.

**The gap this task closes**: `crates/shamir-engine/src/tx/pre_commit.rs`'s
`pre_commit_prelock` function, Phase 2.5 (read the WHOLE function first,
it's not long — the relevant loop is ~line 337-358):

```rust
let mut unique_tokens: Vec<u64> = tx.unique_guards.iter().map(|g| g.table_token).collect();
for table_id in tx.write_set.keys() {
    if let Some(tbl) = repo.table_by_token_if_live(*table_id).await {
        if tbl.needs_write_barrier() {
            unique_tokens.push(*table_id);
        }
    }
}
unique_tokens.sort_unstable();
unique_tokens.dedup();
let mut uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>> = Vec::with_capacity(unique_tokens.len());
for token in &unique_tokens {
    if let Some(tbl) = repo.table_by_token(*token).await? {
        uwl_guards.push(tbl.unique_write_lock().lock_owned().await);
    }
}
```

This has the EXACT SAME check-then-act shape F-48 fixed for the non-tx
path: `needs_write_barrier()` is read ONCE per table (line ~340); if
`false` at that moment, the table's token never enters `unique_tokens`,
so `unique_write_lock` is never taken for it, and the transaction's
ACTUAL data write (which happens much later, at Phase 5c inside
`materialize`) proceeds with no further check. Per
`table_manager_index_mgmt.rs`'s own doc comment ("Every client
INSERT/UPDATE/DELETE/SET runs through an implicit or interactive tx"),
this prelock is how essentially ALL production client writes actually
reach the engine — F-48 alone does NOT close the schema-activation race
for normal traffic, only for the rarely-used raw `TableManager` API path.

## What to do

### 1. Add a thin `TableManager` accessor

`writer_drain` is currently a `pub(super)` field on `TableManager`
holding a `WriterDrainBarrier`. Add a method (mirroring `drain_writers`'s
existing shape) that lets `pre_commit.rs` (a different module) enter the
drain set:

```rust
pub(crate) fn enter_writer_drain(&self) -> super::writer_drain_barrier::WriterDrainGuard {
    self.writer_drain.enter_writer()
}
```

(Adjust visibility/naming to whatever is minimal-but-sufficient for
`pre_commit.rs` to call it — check what visibility `pre_commit.rs`
already needs for `unique_write_lock()`/`needs_write_barrier()` and
match that pattern.)

### 2. Wire the drain into Phase 2.5

For EVERY table in `tx.write_set` (the same loop at ~line 338-344),
**enter the drain set BEFORE calling `needs_write_barrier()`** — this
ordering is load-bearing, see the primitive's own doc for why (the flag's
coherence chain must carry the happens-before edge to the drainer's
load). Then:

- If `needs_write_barrier()` is `true` (this table is about to get a
  `unique_write_lock` guard, same as today): **drop the drain guard
  immediately** before proceeding to the lock acquisition. This mirrors
  F-48's own "slow-path writers do NOT stay in the drain set" rule —
  reasoning: if this tx kept its drain guard AND then blocked waiting for
  `unique_write_lock` (held by a schema-activation DDL that is itself
  calling `drain_writers()` and waiting for this exact guard to clear),
  it deadlocks. The lock alone already provides the exclusion this table
  needs; the drain guard would be redundant AND dangerous to keep here.
- If `needs_write_barrier()` is `false` (this table does NOT get a lock,
  same as today — the table is untouched by any active barrier): **keep
  the drain guard alive** — this is the whole point, so a DDL that raises
  the barrier AFTER this check (and calls `drain_writers()`) genuinely
  waits for this tx to finish.

### 3. Thread the kept-alive drain guards through to the actual write (materialize)

The kept guards (one per `false`-flag table) must stay alive until this
tx's ACTUAL data write for that table has landed — i.e. through Phase 5c
inside `materialize`. Investigate how `uwl_guards` (the existing
`Vec<tokio::sync::OwnedMutexGuard<()>>`) is threaded to find the exact
shape to mirror:

- `PreLockResult` (pre_commit.rs:126-146, returned by
  `pre_commit_prelock`) carries `uwl_guards`.
- `commit.rs`'s `commit_tx_inner` (~line 600) destructures
  `PreLockResult { uwl_guards }` and passes it into EITHER
  `commit_tx_inner_legacy_async` (the AsyncIndex path) OR
  `commit_tx_lockfree` — **both are LIVE production paths, wire both**.
- From there it flows through `pre_commit_locked`/`pre_commit_locked_validate`
  into `PreCommit`/`ValidatedPreCommit` and finally into `materialize`
  (`materialize.rs:63`, dropped at `materialize.rs:195`) or
  `materialize_async_tail` (`commit_phases.rs:245`, dropped at
  `commit_phases.rs:276`).

Pick ONE of these shapes (investigate which is more surgical given the
actual call graph, state your reasoning):

- **(a) Parallel `Vec<WriterDrainGuard>`** threaded alongside
  `uwl_guards` through every one of the same structs/functions
  (`PreLockResult`, `PreCommit`, `commit_tx_inner_legacy_async`,
  `commit_tx_lockfree`, `pre_commit_locked`, `pre_commit_locked_validate`,
  `materialize`, `materialize_async_tail`) — more call sites touched, but
  each change is a mechanical "also add this field/param" alongside the
  existing `uwl_guards` one.
- **(b) A combined guard type**, e.g.
  `enum WriteSerializationGuard { Lock(tokio::sync::OwnedMutexGuard<()>), Drain(WriterDrainGuard) }`,
  replacing `Vec<tokio::sync::OwnedMutexGuard<()>>` with
  `Vec<WriteSerializationGuard>` everywhere `uwl_guards` currently flows —
  fewer distinct threading points (one renamed/rewrapped `Vec` instead of
  two parallel ones), but touches the TYPE at every site that mentions
  `uwl_guards`'s concrete type.

**Scope-limiting decision (read this before choosing)**: this session
independently confirmed (see `docs/dev-artifacts/research/2026-07-28-new-wave-readonly-review.md`
§5 P2-1, and this session's own verification while landing F-40b) that
`crates/shamir-engine/src/tx/group_commit.rs`'s `run_leader` has **NO
production call site** — `commit_tx_inner` only ever calls
`commit_tx_inner_legacy_async` or `commit_tx_lockfree`, never
`group_commit`'s path. **Do NOT wire the drain guards through
`group_commit.rs`** — it's dead code (tracked separately as F-54, whose
job is to decide whether to revive or remove it). If your chosen shape
(especially option (b), the combined-type rename) would otherwise force
you to touch `group_commit.rs` just to keep it compiling (since it also
holds a `Vec<tokio::sync::OwnedMutexGuard<()>>` of the same shape),
either: keep `group_commit.rs`'s own local type as
`Vec<tokio::sync::OwnedMutexGuard<()>>` unchanged (if its guards
genuinely never receive drain guards, since IT is never called from the
live prelock path) and add a narrow conversion/adapter only where
`group_commit.rs` genuinely interoperates with the live path, or note in
your summary exactly what minimal compile-compatibility change was
needed and why it doesn't affect `group_commit.rs`'s (currently inert)
behavior.

### 4. Adversarial red test FIRST

Prove the CURRENT gap: a transaction whose Phase 2.5 reads
`needs_write_barrier() == false` for some table, parks (via a test-only
pause seam — this session's established style, see `commit.rs`'s
`TEST_POST_VALIDATE_PRE_PUBLISH_HOOK` from F-46 commit `57382bab`,
`fk_reverse_cache.rs`'s `TEST_POST_GENCHECK_PRE_PUBLISH_HOOK` from F-47
commit `408ffc97`, and `table_manager_crud.rs`'s
`TEST_POST_BARRIER_PRE_WRITE_HOOK` from F-48 commit `c2cb6e13` — read all
three as templates for the exact one-shot-`armed`/`reached`-poll/`resume`-
`Notify` shape used throughout this session) strictly after Phase 2.5's
flag-check for that table and before its Phase 5c materialize write.
While parked: a schema-activation DDL raises `schema_activation_barrier`,
takes `unique_write_lock`, calls `drain_writers()` (which — on the
CURRENT unfixed code — sees zero in-flight writers, incorrectly, since
this tx never entered the drain set), reads `count() == 0`, and stamps
`keyset_safe = true`. Then release the parked tx; its write lands,
violating the just-stamped proof. Confirm this reproduces on the CURRENT
(F-48-as-landed) code before implementing the fix.

### 5. Implement the fix, make the red test pass

Verify: the existing `schema_activation_barrier_tests.rs` suite (F-37/F-48's
tests) still passes unchanged; `index2_create_barrier_tests.rs` still
passes unchanged (this task doesn't touch that mechanism, though as a
side effect it likely ALSO closes an analogous pre-existing tx-commit-path
gap for `index2_create_barrier` — since Phase 2.5's `needs_write_barrier()`
check is barrier-agnostic (it ORs both flags) — note this in your summary
if you confirm it, but do not go looking for extra work beyond verifying
the existing test suites still pass).

### 6. Documentation

Check `docs/guide-docs/KNOWN_LIMITATIONS.md` for any existing claim about
the schema-activation write barrier's tx-commit-path coverage (as of F-48
landing, a grep for "schema_activation" found NO entry there at all — if
that's still true, no correction is needed; if F-48 or a concurrent
change added one that overclaims tx-commit-path coverage, correct it
narrowly). Do not do a broader sweep — that's F-51's job.

## Constraints

- Do NOT touch `group_commit.rs`'s actual dead-code behavior (F-54's
  job) — only whatever minimal compile-compatibility is unavoidable per
  §3 above.
- Do NOT touch `index2_create_barrier`'s own wiring in
  `table_manager_index_mgmt.rs` (F-50's job).
- Do NOT redesign `WriterDrainBarrier` itself — reuse it as-is. If you
  find its API insufficient for this call site (e.g. you need a way to
  enter/exit without the exact RAII shape it provides), extend it
  minimally rather than replacing it.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.
- Timebox: if the guard-threading proves substantially more invasive than
  expected once you're actually in the code, prefer the SIMPLER shape
  (option (a), parallel vec) even if it touches more call sites
  mechanically — correctness and low risk over an elegant single-type
  refactor, matching F-48's own stated timebox principle.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- schema_activation_barrier
./scripts/test.sh -p shamir-engine -- index2_create_barrier
./scripts/test.sh -p shamir-engine --full
```

When done, give your final summary as plain text: which guard-threading
shape you chose and why, exactly which functions/structs were touched
(list them), the red test's proof (what it demonstrated on the unfixed
code, with actual test output), how `group_commit.rs` was handled
(touched minimally / not at all, and why that's safe given it's dead
code), whether index2_create_barrier's tx-commit-path coverage was
incidentally also verified/improved, full test run output, and
confirmation fmt/clippy are clean.
