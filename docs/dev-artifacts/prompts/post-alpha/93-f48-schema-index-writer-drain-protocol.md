# Brief for F-48 (#859, P0) — schema/index DDL writer-drain protocol

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

The 2026-07-28 readonly review
(`docs/dev-artifacts/research/2026-07-28-new-wave-readonly-review.md`, §3
P0-3) found that F-37's `schema_activation_barrier` has a genuine
check-then-act race, and — separately — that the SAME class of race
already exists, honestly documented, in the older
`index2_create_barrier` mechanism. **Read the review's P0-3 section in
full first**, then read these three files in full (they're short, all
directly relevant):

1. `crates/shamir-engine/src/table/table_manager.rs` — `needs_write_barrier()`
   at line 626-634: `self.index_manager.has_unique_indexes() ||
   self.index2_create_barrier.load(Acquire) ||
   self.schema_activation_barrier.load(Acquire)`.
2. `crates/shamir-db/src/shamir_db/execute/admin_schema.rs` — read the
   whole `SchemaActivationBarrierGuard`/`begin_schema_activation_barrier`
   section (~line 105-172) and its two call sites (~line 500-511,
   ~712-717) — this is F-37's fix, and its own comment already says it
   "mirrors the existing unique-index DDL write-barrier pattern exactly."
3. `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` —
   `backfill_index2_backend`'s doc comment (~line 340-414) is the OLDER
   instance of the SAME race class, already candidly documented (its
   "Check-then-act, not a drain" residual at :396-403, referenced by
   `index2_create_barrier_tests.rs`'s own regression test names).

**The gap** (verified by reading `table_manager_crud.rs`'s writer call
sites — `insert_returning_version` :82-93,
`insert_many_returning_version` :174-183, `delete_returning_version`
:317-326, `update`/similar :384-393 — all four share the identical
shape):

```rust
let _guard = if self.needs_write_barrier() {
    Some(self.unique_write_lock.lock().await)
} else {
    None
};
```

This is a **check-then-act**, not a drain: a writer that reads
`needs_write_barrier() == false` proceeds completely lock-free through
its ENTIRE validate→write→index sequence with NO further check. The DDL
side (`begin_schema_activation_barrier` + `SchemaActivationBarrierGuard`)
raises the flag and takes the SAME lock, but a writer that already read
`false` a moment earlier is not waited for — nothing "drains" it. The
concrete interleaving: writer reads `false` (barrier not yet up) → DDL
raises the barrier and takes the lock → DDL reads `count() == 0` → the
still-in-flight writer (which took the lock-free branch) inserts a row →
DDL persists/activates `keyset_safe = true` over a table whose row
history was never actually proven homogeneous, because a row landed
after the count-proof but the DDL has no way to know that.

This is the SAME structural defect for BOTH `schema_activation_barrier`
(F-37) and `index2_create_barrier` (older, already honestly documented as
open) — **this task builds ONE general-purpose drain primitive** that
closes it for schema activation now; F-50 (a separate, already-tracked
task, blocked on this one) will apply the SAME primitive to
`index2_create_barrier`'s residual. Design it so F-50 can mechanically
reuse it — don't hardcode anything schema-activation-specific into the
core mechanism.

## What to settle: pick ONE drain protocol design

Investigate and choose ONE of these (or a variant you find more
appropriate once you've read the actual code — state your reasoning
either way, this is a genuine design decision, not a mandated answer):

1. **Reader/writer epoch protocol with an active-writer counter.** Each
   writer increments a counter on entry, decrements on exit (RAII guard).
   The DDL side, after raising its intent flag, waits until the counter
   drops to zero (or was already zero at a snapshot taken after the flag
   went up) before proceeding — i.e. a genuine drain, not just a lock
   acquisition race.
2. **Seqlock-style optimistic check.** A writer reads an epoch/sequence
   number before its work and re-checks it after; a mismatch means a DDL
   raised the barrier mid-flight, and the writer must retry (or fall back
   to taking the lock the slow way).
3. **Unified async RW barrier.** The DDL takes a write-intent guard; each
   writer takes a cheap read-participation guard for the duration of its
   validate→write→index sequence. The write-intent guard doesn't proceed
   until all outstanding read-participation guards at the moment it was
   requested have dropped.

Whichever you choose, it must satisfy:
- **No false negatives**: a DDL that proceeds past its drain point must
  never race a writer that legitimately started before the barrier went
  up and is still doing its validate→write→index work.
- **Cheap on the writer's hot/no-barrier path**: the overwhelming common
  case (no DDL barrier active) must stay effectively the current
  zero-cost `is_empty`/bool-load check — do not introduce a lock or
  atomic RMW on every single write when no barrier is active.
- **Reusable, not schema-specific**: F-50 needs the same mechanism for
  `index2_create_barrier`'s Part C residual (see
  `table_manager_index_mgmt.rs`'s doc, "Check-then-act, not a drain").
  Structure the primitive (a type, a small set of methods) so F-50 can
  wire it up to that OTHER flag with minimal new design work — a
  genuinely shared drain mechanism, not two near-duplicate
  implementations.

## What to prototype/implement

1. **Adversarial red test FIRST**: prove the CURRENT check-then-act race
   on the unfixed code. Use a test-only pause seam (this codebase's
   established style — see `commit.rs`'s `TEST_POST_VALIDATE_PRE_PUBLISH_HOOK`
   from F-46, commit `57382bab`, and `fk_reverse_cache.rs`'s
   `TEST_POST_GENCHECK_PRE_PUBLISH_HOOK` from F-47, commit `408ffc97` —
   both landed earlier this session in this exact area of the codebase,
   read them as the template) that parks a writer strictly AFTER it reads
   `needs_write_barrier() == false` and BEFORE it actually performs its
   write. The test drives: writer reads `false` and parks → DDL raises
   the barrier, takes the lock, reads `count() == 0` → writer (still
   parked) is released and completes its write → DDL persists
   `keyset_safe = true`. Prove this currently produces an inconsistent
   result (a row exists that violates the just-stamped `keyset_safe`
   proof) — this is the `schema_activation_barrier_tests.rs:64-146`
   coverage gap the review names (those existing tests only start the
   writer AFTER the flag is already up).
2. **Implement the chosen drain protocol**, wire it into
   `needs_write_barrier()`'s call sites in `table_manager_crud.rs` (all 4
   — insert single/many, delete, update) and into
   `begin_schema_activation_barrier`/`SchemaActivationBarrierGuard` in
   `admin_schema.rs`.
3. **Make the red test pass.** Then verify: existing
   `schema_activation_barrier_tests.rs` (all tests, unaffected — still
   pass); the analogous `index2_create_barrier_tests.rs` suite is
   UNAFFECTED by this task (F-50's job to wire the new primitive there) —
   confirm it still passes unchanged, don't touch it.

## Constraints

- Do NOT modify `index2_create_barrier`'s actual wiring in
  `table_manager_index_mgmt.rs` in THIS task — F-50 (blocked on this task
  completing) does that. You MAY read it and design the shared primitive
  with it in mind, but don't touch `create_index_v2`/`backfill_index2_backend`.
- Do NOT change `needs_write_barrier()`'s PUBLIC boolean-check shape in a
  way that breaks callers outside `table_manager_crud.rs` unless you
  update every call site — grep for all callers first.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.
- Timebox: if the chosen protocol proves substantially harder to
  implement correctly than expected, it's acceptable to land a genuinely
  correct but simpler variant (e.g. option 1, the counter-based drain,
  tends to be the most straightforward to reason about) rather than
  chase a more elegant but riskier design — correctness over elegance for
  a P0 concurrency fix.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- schema_activation_barrier
./scripts/test.sh -p shamir-engine -- index2_create_barrier
./scripts/test.sh -p shamir-engine --full
```

When done, give your final summary as plain text: which drain protocol
you chose and why (including why the alternatives were less suitable),
the red test's proof (what it demonstrated on the unfixed code, with
actual test output), the exact mechanism implemented and its shape
(specifically how F-50 can reuse it for `index2_create_barrier`), full
test run output, and confirmation fmt/clippy are clean.
