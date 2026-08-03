# Brief — P0-1: WriteBarrierGuard does not support multiple owners of the same bit

Task: #957 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-new-wave-readonly-review.md` §P0-1, verified against the actual source before filing this task. Task #960 (P0-4, an unrelated corruption bug) was already fixed and committed separately — do not touch `shamir-index`'s unique-posting code.

## Bug

`crates/shamir-engine/src/table/table_manager.rs`:

- `begin_write_barrier` (lines ~763-778):
  ```rust
  pub async fn begin_write_barrier(
      &self,
      bit: u8,
  ) -> (WriteBarrierGuard, tokio::sync::OwnedMutexGuard<()>) {
      let guard = WriteBarrierGuard::set(self.write_barrier_flags.clone(), bit);
      self.drain_writers().await;
      let lock_guard = self.unique_write_lock.clone().lock_owned().await;
      (guard, lock_guard)
  }
  ```
- `WriteBarrierGuard` (lines ~1035-1051):
  ```rust
  pub struct WriteBarrierGuard {
      flags: crate::index::write_barrier_flags::WriteBarrierFlags,
      bit: u8,
  }
  impl WriteBarrierGuard {
      fn set(flags: ..., bit: u8) -> Self {
          flags.set(bit);
          Self { flags, bit }
      }
  }
  impl Drop for WriteBarrierGuard {
      fn drop(&mut self) {
          self.flags.clear(self.bit);
      }
  }
  ```
- The underlying bits themselves (`crates/shamir-index/src/legacy/write_barrier_flags.rs`, `WriteBarrierFlags::set`/`clear`) are bare `fetch_or`/`fetch_and` on a shared `AtomicU8` — there is NO reference count. Confirmed by reading the file: `set(bit)` is `self.bits.fetch_or(bit, SeqCst)`, `clear(bit)` is `self.bits.fetch_and(!bit, SeqCst)`.

### The race

Two concurrent DDL operations of the same family (e.g. two `CREATE INDEX` on the same table, both regular, so both use `REGULAR_INDEX_CREATE`):

1. DDL-A calls `begin_write_barrier(REGULAR_INDEX_CREATE)`: sets the bit, drains in-flight writers, takes `unique_write_lock`.
2. DDL-B calls `begin_write_barrier(REGULAR_INDEX_CREATE)` concurrently: `fetch_or` on an already-set bit is a no-op on the atomic value, B also drains (cheap, nothing new in flight), then blocks on `unique_write_lock` (already held by A).
3. A finishes its backfill/snapshot work and drops its `WriteBarrierGuard` → unconditional `clear(REGULAR_INDEX_CREATE)`. The bit is now 0, even though B's DDL is about to start its own (still in-flight) backfill.
4. B acquires `unique_write_lock` (A released it), but B's own `WriteBarrierGuard` never re-sets the bit (it was already "set" from B's perspective — B's constructor called `flags.set(bit)` too, but that's irrelevant now: the bit is currently 0 and nothing re-raises it going into B's actual critical section).
5. A brand-new writer (not one of the drained ones) calls `needs_write_barrier()` sometime while B is mid-backfill, sees `false` (bit is 0), takes the fast (unlocked) path, and can now race B's still-in-progress snapshot/backfill — the exact hazard the barrier exists to prevent. B already did its drain BEFORE the lock, based on the writer population at drain time; it will not drain again for writers that start now.

Same hazard applies to any two concurrent operations sharing a bit: two `CREATE UNIQUE INDEX`, two `CREATE INDEX ... SORTED`, two index2 creates, two schema-activation sequences.

The existing F-70 lock-order-inversion tests (`crates/shamir-engine/src/table/tests/f70_lock_order_inversion_tests.rs`) model a DIFFERENT hazard (the deadlock from acquiring raise→drain→lock in the wrong order) — read that file for the test harness/mocking conventions used in this area, but note it does NOT cover two guards racing on the same bit. Do not assume passing F-70 tests say anything about this bug.

## Required fix

Per the review, the recommended fix is to stop treating the shared bit as an ownership flag. Concretely:

1. Add a per-table `ddl_admission: tokio::sync::Mutex<()>` (or equivalent) to `TableManager`, acquired **before** the existing `raise bit → drain_writers → unique_write_lock` sequence in `begin_write_barrier`. This serializes concurrent DDL operations that would otherwise race on the same bit — DDL-B now simply waits for DDL-A's ENTIRE `begin_write_barrier`-guarded critical section (not just the `unique_write_lock` portion) before it starts raising its own bit.
   - Regular writers (the fast/slow-path predicate in `needs_write_barrier()`) must NOT take this new mutex — only DDL callers of `begin_write_barrier` do. This preserves the F-70 fix (writers never block behind DDL-held locks in a way that could deadlock) while closing the multi-owner race.
   - Where to acquire it: study `begin_write_barrier`'s current three-step body and every call site (`table_manager_index_mgmt.rs`, `table_manager_sorted_index.rs`, `pre_commit.rs`, `writer_drain_barrier.rs` — grep `begin_write_barrier` to enumerate all of them) to decide whether the mutex belongs inside `begin_write_barrier` itself (simplest — one change, all callers automatically serialize) or needs to be threaded through differently. Prefer putting it inside `begin_write_barrier` unless you find a concrete reason a caller needs finer control.
2. After acquiring the new admission mutex (and before/while doing raise→drain→lock), consider whether a defensive re-check of intent + re-drain is warranted right after `unique_write_lock` is acquired, per the review's suggestion — evaluate whether the admission mutex alone already makes this redundant (since only one DDL op can be inside the guarded region at a time now) or whether a genuine race window remains; document your reasoning either way in the final report.
3. Do NOT change `WriteBarrierFlags`'s ordering or the bit constants — the SeqCst single-atomic-load fix from F-69 stays as is; this task is about serializing DDL admission, not about the flag word's memory-ordering guarantees.

## Required tests

Add a new test file (follow this crate's `tests/` layout convention — see `crates/shamir-engine/src/table/tests/mod.rs` for how `f70_lock_order_inversion_tests` and `f72_planner_invisibility_tests` are wired in; add your new module the same way, re-exported from `tests/mod.rs`, no inline `#[cfg(test)] mod tests { ... }` blocks per CLAUDE.md).

Deterministic (not sleep-based — use the same paused/synchronization primitives the F-70 test file uses, e.g. explicit channels/barriers/`tokio::sync::Notify` to control exact interleaving) tests proving:

- Two concurrent DDL operations racing the SAME bit: DDL-A starts and is paused mid-critical-section (after acquiring the barrier, before finishing); DDL-B is launched concurrently and must NOT be able to proceed past its own admission point until DDL-A fully completes (including its `WriteBarrierGuard` drop). Assert the actual ordering (e.g. via a shared log/sequence counter both DDL tasks append to) rather than just asserting no panic/deadlock.
- After DDL-A completes and DDL-B is admitted and running, a NEW writer (started after DDL-A's guard dropped but while DDL-B is still active) must observe `needs_write_barrier() == true` and take the barrier path — it must NOT be able to fast-path past DDL-B's still-in-progress work.
- Repeat the "two concurrent DDL same bit" scenario for at minimum: schema activation, regular index create, unique index create, sorted index create. (index2 create if time permits — same principle, lower priority since F-72's planner-invisibility fix already provides a partial mitigation there for NEW readers, though this admission race is a different layer.)
- A regression test confirming F-70's original deadlock scenario (wrong lock order) still does not reproduce with the new admission mutex in place — i.e. the new mutex must not reintroduce the cycle F-70 closed. Re-run the existing `f70_lock_order_inversion_tests.rs` suite as-is (it should still pass unmodified) plus add one new test specifically exercising the interaction between the new admission mutex and the existing `unique_write_lock`/writer-drain order.

## Scope discipline

- Touch ONLY: `TableManager`'s `begin_write_barrier` (and the new admission mutex field/its initialization in whatever constructs `TableManager`), plus new tests. Do NOT touch `WriteBarrierFlags`/`write_barrier_flags.rs` (F-69's fix stays as-is), do NOT touch anything in `shamir-index`, do NOT touch tx reconciliation, DROP/RENAME protocols, or perf-gate config — those are separate tasks (#958, #959, #961-#963) worked in later, separate `crush` sessions.
- Run ONLY the centralized test entry point: `./scripts/test.sh -p shamir-engine` (narrow with `-- <filter>` for your new test names first, then the full crate). Raw `cargo test` is blocked by this repo's perimeter guard.
- `cargo fmt -p shamir-engine -- --check` and `cargo clippy -p shamir-engine --all-targets -- -D warnings` (and `--workspace --all-targets` if you touch anything visible across crate boundaries) must be clean before you declare done. Note any PRE-EXISTING unrelated clippy failures rather than fixing them inline.
- If loom tests exist for this barrier area (`crates/shamir-engine` mentions loom in its CI per the review — grep for `loom` in this crate), check whether your change needs a loom-model update; if genuinely out of scope/too large, say so explicitly in your report rather than silently skipping.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Do NOT run `git commit` or `git add` either — the orchestrator verifies your diff and the test run, then commits. Only edit files and run read-only/build/test commands. Clean up any stray log files you create in the repo root by deleting them directly (`rm <file>.log` is fine — that is not a git command), don't leave them for the orchestrator if you can trivially remove them yourself; if you do leave any, say so in your report.

## What to report back

End your turn with: exactly which files changed and why, the design decision on WHERE the admission mutex lives and why, whether you added a defensive re-check after acquiring `unique_write_lock` and why/why not, what each new test proves (not just its name), and the exact `cargo fmt` / `cargo clippy` / `./scripts/test.sh` commands you ran with real pass/fail counts and exit codes — not paraphrased.
