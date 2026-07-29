# Brief for F-57 (#883, P0) — unified online CREATE INDEX lifecycle across all index families

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. An independent readonly review of
snapshot `e145b1d3` (`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`,
section P0-3) found that online `CREATE INDEX` is safe for only ONE of
four index families. **F-56 (#882, landed just before this task, commit
`7fde958e`) already fixed `create_index_v2`**: it corrected the
`WriterDrainBarrier`'s memory ordering (now `SeqCst` throughout — see
`crates/shamir-engine/src/table/writer_drain_barrier.rs`'s doc comment for
the worked proof) and wired `drain_writers()` into `create_index_v2`
(`table_manager_index_mgmt.rs:97`, right after `Index2CreateBarrierGuard::set`
raises `index2_create_barrier`, before the backfill snapshot). **Do not
redo or touch that work** — it is the corrected foundation this task
builds on for the other three index kinds.

### The three remaining gaps (verified by the orchestrator independently)

1. **Regular hash index — ZERO protection.** `create_index`
   (`table_manager_index_mgmt.rs:493-521`) does: `build_index_definition`
   → `collect_all_current_records` → `create_index_from_records`. No
   intent flag, no lock, no drain. A write landing between the snapshot
   and registration is in neither the backfill set nor the live
   `index_manager` write hook (`needs_write_barrier()` never even
   considers regular indexes — only `index_manager.has_unique_indexes()`,
   `index2_create_barrier`, `schema_activation_barrier`). This is the
   worst gap: not "check-then-act", but no check at all.

2. **Unique index — has a lock, but writers can bypass it entirely.**
   `create_unique_index` (`table_manager_index_mgmt.rs:539-547`) DOES take
   `unique_write_lock` across the whole snapshot→backfill→register
   sequence — but `TableManager::needs_write_barrier()`
   (`table_manager.rs:762-770`) decides whether a writer takes that lock
   via `self.index_manager.has_unique_indexes()`. **Before THIS create
   call registers the first unique index, `has_unique_indexes()` is
   `false`.** So every concurrent fast-path writer reads
   `needs_write_barrier() == false` and proceeds LOCK-FREE — completely
   unaware the DDL is holding `unique_write_lock` at all. The lock only
   protects writers *for the table's second and later* unique-index
   creates; the FIRST one races every concurrent writer with no
   coordination whatsoever, which can both miss postings and let a
   duplicate slip past the uniqueness backfill proof. This needs the SAME
   intent-flag + drain treatment `index2_create_barrier` uses — a
   `unique_index_create_barrier` flag that `needs_write_barrier()` also
   consults, raised BEFORE `collect_all_current_records`, with a
   `drain_writers()` call before the snapshot (mirroring
   `create_index_v2`'s now-corrected pattern exactly).

3. **Sorted index — registers before backfill, explicitly unsafe under
   cancellation.** `create_sorted_index_with_include`
   (`table_manager_sorted_index.rs:24-...`) has NO write barrier of any
   kind, and its own doc comment (`:10-16`) already admits: "cancel-safe:
   NO ... cancellation after register but before/during the backfill loop
   leaves a registered sorted index with partial entries ... Do NOT call
   under `tokio::select!` / `tokio::time::timeout`." Read the full
   function before deciding an approach.

## What to do

**Priority order — do NOT skip #1 or #2 for #3.** The regular-hash and
unique-index gaps are structurally identical to the index2 gap F-56 just
fixed (a missing/bypassable barrier around snapshot→backfill→register),
so they should be the highest-confidence, most mechanical part of this
task. Sorted index is the hardest (register-before-backfill is a
different shape, not just a missing barrier) and has an explicit,
already-documented interim fallback — do not let it block #1/#2.

1. **Regular hash index (`create_index`).** Add a new intent flag
   (`regular_index_create_barrier: Arc<AtomicBool>` on `TableManager`,
   alongside `index2_create_barrier`/`schema_activation_barrier`) that
   `needs_write_barrier()` also consults (all loads/stores `SeqCst`,
   matching F-56's corrected protocol — do NOT reintroduce
   `Acquire`/`Release` for a new flag, the same cross-atomic dependency
   with `WriterDrainBarrier`'s `active` counter applies here too). Take
   `unique_write_lock` across the create (mirrors `create_unique_index`'s
   existing shape — reusing the SAME lock across all barrier kinds is
   consistent with `needs_write_barrier`'s existing "does ANY of these
   conditions hold" design), raise the flag, call `self.drain_writers().await`
   before `collect_all_current_records`, register, then drop the guard
   (RAII, clearing on every exit path including error returns — mirror
   `Index2CreateBarrierGuard`'s shape).

2. **Unique index (`create_unique_index` / `create_unique_index_locked`).**
   Add `unique_index_create_barrier: Arc<AtomicBool>`, wire it into
   `needs_write_barrier()` the same way, raise it (`SeqCst`) before
   `collect_all_current_records` in `create_unique_index_locked` (which is
   ALSO called by `rename_index` — check that call site still holds
   `unique_write_lock` per its own doc comment, and make sure your new
   flag-raise doesn't double-raise or conflict there), call
   `drain_writers()` before the snapshot, drop on every exit path. Note
   `create_unique_index_locked` currently assumes the caller holds the
   lock — keep that contract; only add the flag + drain inside it.

3. **Sorted index.** Read `table_manager_sorted_index.rs` in full,
   including every caller of `create_sorted_index_with_include`/
   `create_sorted_index`, before choosing an approach. Two acceptable
   outcomes for this task (pick based on what you find — do not silently
   leave it exactly as-is with no change):
   - **(a) Full fix**: give it the same flag+lock+drain treatment as
     above, changed to register-AFTER-backfill (matching the other three
     kinds' shape) if that doesn't conflict with how `SortedIndexManager`
     expects to be used elsewhere, OR keep register-before-backfill but
     make cancellation and concurrent-writer exposure safe via the shared
     guard (drain closes the writer race; a persisted `Building`/`Ready`
     state — reuse whatever state machine already exists for index2's
     lifecycle if one does, do not invent a parallel one — closes the
     cancellation-leaves-partial-index gap).
   - **(b) Documented interim restriction** (the safe-alpha fallback the
     review itself sanctions for P0-3): restrict
     `create_sorted_index_with_include` to tables with zero live
     readers/writers at call time (e.g. require the caller to prove the
     table has no other in-flight tx / a documented "offline maintenance
     window" precondition) OR add a clear runtime rejection with a
     `DbError` variant if the table isn't empty, with a doc comment
     explaining this is a temporary restriction pending a real fix. If you
     choose (b), you MUST say so explicitly in your final summary — do
     not silently ship a stub that looks like a real fix.

4. **Shared guard structure — do not duplicate the RAII pattern four
   times if it can reasonably be shared.** `Index2CreateBarrierGuard`
   (`table_manager_index_mgmt.rs`) already has the exact
   set-under-lock/clear-on-drop shape needed for both new flags. Consider
   whether a small generic `IndexCreateBarrierGuard<'a>` (parameterized
   over which `AtomicBool` it wraps) can replace three near-identical
   structs — but ONLY if this is a clean, mechanical refactor; do not
   force an abstraction that costs more clarity than it saves. If you
   introduce it, keep `Index2CreateBarrierGuard`'s existing name/call
   sites working (or update them consistently) — don't leave the codebase
   half-migrated.

5. **Tests, per index kind, reusing this repo's existing pattern**
   (`index2_create_barrier_tests.rs`'s `PostBarrierPreWriteHook` +
   `tokio::sync::Notify` seam, and F-56's new
   `f56_create_index_v2_drains_inflight_fast_path_writer` as the most
   recent template):
   - regular hash index: an in-flight fast-path writer must block the
     create's drain (or, symmetrically, the create's flag must force the
     writer onto the slow path — assert whichever direction reflects your
     implementation) and the writer's row must end up in the new index's
     backfill.
   - unique index: SPECIFICALLY test the "first unique index on this
     table" case (`has_unique_indexes()` is `false` going in) — this is
     the exact scenario the review flagged as bypassed today. Confirm a
     concurrent fast-path writer is now forced onto the slow path even
     though no unique index exists yet.
   - sorted index: a test matching whichever of (a)/(b) you chose —
     either a drain-based regression test (same shape as the others) or a
     test proving the interim restriction actually rejects a non-empty
     table.

## What NOT to do

- Do NOT touch `create_index_v2`, `Index2CreateBarrierGuard`, or
  `writer_drain_barrier.rs`'s corrected SeqCst orderings — F-56 already
  fixed those; re-verify they still pass, don't re-derive them.
- Do NOT touch F-55/F-58/F-59/F-60/F-61 (other tasks from the same
  review).
- Do NOT invent a NEW ordering scheme for the new flags — reuse F-56's
  established SeqCst protocol verbatim (all four cross-atomic operations
  per flag: writer's counter increment, writer's flag load, drainer's flag
  store, drainer's counter load, all `SeqCst`).
- Do NOT silently skip sorted index — either fix it or explicitly restrict
  it with a clear error and a documented reason (option 3b above).

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-db -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- TDD: each new regression test should fail against the pre-fix code path
  and pass after.
- Clean up any scratch/debug files created in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-db -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
```

Plus a personal red-then-green reproduction of at least the regular-hash
and unique-index fixes (the two highest-confidence, most mechanical
parts).

When done, give your final summary as plain text: the new flag(s) added
and their exact SeqCst call sites (file:line), whether a shared guard
struct was introduced, which of (a)/(b) was chosen for sorted index and
why, the new tests added, and confirmation fmt/clippy/tests are clean.
