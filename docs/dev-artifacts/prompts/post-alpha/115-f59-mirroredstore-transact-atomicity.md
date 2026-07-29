# Brief for F-59 (#885, P0) — MirroredStore::transact mixed-batch error atomicity

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. An independent readonly review of
snapshot `e145b1d3` (`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`,
section P0-5) found that `MirroredStore::transact`
(`crates/shamir-storage/src/storage_mirrored.rs:554-606`) does not honor
the atomic-mixed-batch contract the storage `README.md` and the
transactional engine's commit-bundling both rely on.

### The bug, exactly as it exists today

```rust
async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
    // ... partition into ephemeral_ops / durable_ops ...

    // Phase 1 — ephemeral subset → primary (UNCONDITIONAL, runs first)
    for op in ephemeral_ops { /* primary.set/remove, `?` propagates */ }

    // Phase 2 — durable subset → mirror FIRST (fallible!)
    self.mirror.transact(durable_ops.clone()).await?;   // <-- can fail HERE

    // Phase 3 — durable subset → primary (only reached if phase 2 succeeded)
    for op in durable_ops { /* primary.set/remove */ }

    Ok(())
}
```

If `self.mirror.transact(...)` fails at Phase 2, the function returns
`Err` — but **Phase 1's ephemeral mutations already landed in `primary`
and are visible to concurrent readers.** The caller receives an
observable `Err` even though part of the batch is already committed. This
is pinned by an EXISTING test,
`transact_ephemeral_applied_but_durable_aborted_on_mirror_failure`
(`crates/shamir-storage/src/tests/storage_mirrored_tests.rs:596-644`),
which currently asserts this AS THE EXPECTED (i.e., accepted-as-current,
not necessarily correct) behavior — you will need to update it to assert
the corrected behavior instead (see step 3).

The storage `README.md` describes `transact` as an atomic mixed-op batch,
and the transactional engine relies on that contract for commit
bundling. An `Err` after a partial, externally-visible mutation
contradicts "atomic" — this is an observable semantics bug, not a
documentation nit.

### Why the fix is a minimal, already-justified reorder (verified, not assumed)

`storage_mirrored.rs:216-217` confirms `MirroredStore.primary` is
concretely typed `InMemoryStore` (not a generic `dyn Store`). The SAME
file's own doc comment (the "Residual — reverse-direction divergence"
section directly above `transact`, lines ~511-528) already establishes,
for this exact concrete type, that `InMemoryStore::set` and
`InMemoryStore::remove` are **structurally infallible at the `DbResult`
level** — no `?` propagation path, no `Err` return; a genuine allocation
failure would `panic!`/abort, not surface as `Err`. This is the existing
justification for why Phase 3 (durable → primary, running AFTER the
fallible mirror commit) is safe today. The SAME argument extends cleanly
to Phase 1 (ephemeral → primary) if it is moved to run AFTER the mirror
commit too — since it targets the same concrete `InMemoryStore`, it
cannot fail there either.

**The fix is: move the ephemeral-ops-to-primary loop (current Phase 1) to
run AFTER `self.mirror.transact(durable_ops.clone()).await?` (current
Phase 2), not before it.** The corrected sequence:

1. Partition ops into `ephemeral_ops` / `durable_ops` (unchanged).
2. `self.mirror.transact(durable_ops.clone()).await?` — durable mirror
   commit, atomic, fallible. **If this fails, return `Err` immediately —
   `primary` has NOT been touched at all yet (neither subset).**
3. Apply `ephemeral_ops` to `primary` (moved from the old Phase 1).
4. Apply `durable_ops` to `primary` (old Phase 3, unchanged position
   relative to the mirror commit).

This achieves genuine all-or-nothing semantics for the WHOLE mixed batch,
observable to the caller: `Err` now means nothing landed in `primary`;
`Ok` means everything did. This picks the review's option (b)
("commit durable-mirror first and make BOTH subsets' primary application
infallible after that point") — the option the review itself flags as
viable when the concrete backend supports it, which this codebase's
`InMemoryStore` already does per the existing, un-refuted investigation.

## What to do

1. **Reorder `transact`** exactly as described above — this should be a
   small, surgical diff (move one loop below the mirror-commit call),
   not a rewrite.
2. **Update the doc comment** above `transact` (lines ~479-553): the
   "Concurrent-reader residual" section can stay (it's about a DIFFERENT,
   still-open issue — partial visibility of the ephemeral loop's
   individual ops to a concurrent reader mid-loop, not the error-atomicity
   bug this task fixes). But the numbered phase description (1/2/3) and
   any text implying Phase 1 (ephemeral) runs before the mirror commit
   must be corrected to describe the NEW order. Do not leave stale
   phase-numbering that no longer matches the code.
3. **Update `transact_ephemeral_applied_but_durable_aborted_on_mirror_failure`**
   (`storage_mirrored_tests.rs:596-644`) to assert the CORRECTED
   behavior: on a mirror-transact failure, assert `store.get(ephemeral)`
   is now `Err` too (primary was NEVER touched for either subset) — the
   opposite of its current assertion at lines 622-626. Rename the test if
   its name no longer matches (e.g.
   `transact_neither_subset_applied_on_mirror_failure` or similar —
   pick a name that describes the NEW, correct behavior). Update the
   test's own doc comment/inline comments to match.
4. **Add a positive test** proving the happy path still works
   end-to-end: a mixed ephemeral+durable batch with NO mirror failure
   must result in BOTH subsets visible in `primary` afterward (this may
   already be covered by an existing test — check
   `storage_mirrored_tests.rs` before writing a new one; only add one if
   the coverage gap is real).
5. **Check for other callers/tests that assert the OLD (buggy) ordering**
   — `rg "transact"` across `crates/shamir-storage/src/tests/` and
   anywhere else `MirroredStore::transact` behavior is asserted — and
   update anything that encoded the old, now-incorrect partial-failure
   expectation.

## What NOT to do

- Do NOT touch the "Concurrent-reader residual" issue (partial visibility
  of the ephemeral loop's ops mid-application to a concurrent reader) —
  that is a separate, larger, explicitly out-of-scope problem per this
  same file's own doc comment (it would need a global write lock or a
  snapshot/isolation layer in `InMemoryStore` — not this task).
- Do NOT change `MirroredStore.primary`'s type or add a generic `dyn
  Store` abstraction — the fix specifically relies on `primary` being
  concretely `InMemoryStore`; do not weaken that.
- Do NOT touch F-55/F-56/F-57/F-58/F-60/F-61 (other tasks from the same
  review).
- Do NOT pick options (a)/(c)/(d) from the review (forbid mixed batches /
  introduce an MVCC root-swap primary / drop the atomicity claim) — this
  brief has already selected option (b) based on the verified
  infallibility argument; only deviate if you find that argument is
  actually wrong (e.g. `primary`'s type changes, or `InMemoryStore::set`/
  `remove` turn out to be fallible after all) — if so, stop and report
  rather than picking a different option unilaterally.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-storage -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- TDD: the updated/new tests should genuinely distinguish old vs new
  behavior — confirm the updated test fails against the PRE-fix ordering
  (assert what used to be true is now false) before restoring the fix.
- Clean up any scratch/debug files created in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-storage -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-storage --full
```

Plus a personal red-then-green reproduction of the reordering fix.

When done, give your final summary as plain text: the exact diff
(file:line), the renamed/updated test and its new assertions, any other
stale test expectations found and fixed, and confirmation
fmt/clippy/tests are clean.
