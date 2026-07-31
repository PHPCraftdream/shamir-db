# F-77 (#904) — MirroredStore::transact: deliver visibility atomicity or stop claiming it

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## Background — what F-59 (#885, commit `a66d68d6`) already fixed

F-59 gave `MirroredStore::transact` (`crates/shamir-storage/src/
storage_mirrored.rs`, currently ~line 560) correct whole-batch **ERROR**
atomicity: the mirror backend's own `transact` commits FIRST (all-or-
nothing on a real transactional backend like fjall), and primary is only
ever touched after that succeeds. A mirror failure now leaves NEITHER the
ephemeral nor the durable subset applied to primary — an honest "nothing
happened" `Err` for the whole batch.

## What remains — VISIBILITY atomicity is still broken

Primary is mutated by TWO separate PER-OPERATION loops: the ephemeral
subset (currently ~lines 585-598) and the durable subset (~lines 600-613),
each calling `self.primary.set(k, v)`/`self.primary.remove(k)` one key at a
time. A concurrent reader can observe the primary store mid-loop — HALF a
committed batch, with some keys updated and others not yet.

**This contradicts the trait contract.** `Store::transact`'s default doc
(`crates/shamir-storage/src/types.rs`, ~line 189) states: "When a backend
overrides this with a transactional impl, partial state is never
observable." `MirroredStore` DOES override `transact` (it is not using the
default per-op loop), so by the trait's own contract it is ADVERTISING
real atomicity while not providing it — worse than either honest
alternative (no override = clearly non-atomic, or a real atomic override).

## Step 1 — MANDATORY audit before picking a fix

Before choosing a resolution, determine whether this is a LATENT
CORRECTNESS BUG for a real call site or ONLY a contract-honesty problem.
Grep every current caller of `MirroredStore` (production, not tests —
`crates/shamir-engine/src/repo/repo_types.rs` is confirmed as the sole
production construction site as of this writing, used for the
metadata/catalogue in-memory-mirrored-by-disk pattern) and every caller of
`.transact(...)` reachable through it. For each, answer: does this caller
(or a concurrent reader elsewhere in the same process) rely on observing
either the FULL pre-batch state or the FULL post-batch state — i.e. would
observing a half-applied batch produce a WRONG answer, or is every reader
of this store single-key-at-a-time in a way that a half-applied batch is
harmless? **Write this audit's finding explicitly into your commit
message** — it decides whether this task's severity should be escalated
above P1 (if a real caller can observe a wrong multi-key read today) or
stays a contract-hygiene fix (if no current caller is exposed).

## Step 2 — pick ONE of the three resolutions, based on the audit

1. **Implement real visibility atomicity.** `InMemoryStore` (the usual
   primary here) is currently `Arc<scc::TreeIndex<RecordKey, Bytes>>` — a
   lock-free B+ tree with per-key concurrent mutation, no whole-store
   snapshot-swap primitive today. An RCU/snapshot-swap fix would need
   either: (a) wrapping the whole store in an `ArcSwap<TreeIndex<...>>`
   and building a full COW replacement tree per transact call (correctness
   is straightforward; cost is O(current tree size) per batch — evaluate
   whether that is acceptable for this store's actual usage pattern, given
   it backs metadata/catalogue tables which the audit above should have
   characterized as small/rarely-mutated or not), or (b) a narrower
   overlay/staging structure that publishes only the batch's own keys
   atomically (a small COW map holding just this batch, swapped into a
   read path that checks the overlay before the base tree) — evaluate
   both, pick the one that fits `InMemoryStore`'s actual read/write
   pattern, and justify the choice.
2. **Add an explicit capability flag and stop claiming atomicity.** e.g.
   `fn supports_atomic_transact(&self) -> bool` on the `Store` trait
   (default `false`; overridden `true` only by backends that actually
   deliver it), with `MirroredStore` correctly reporting `false` (or
   `true` only if a real disk-backed primary is used, if that variant
   exists — check). Update the trait-level contract doc at `types.rs`
   to state plainly that visibility atomicity is opt-in and callers MUST
   check this flag before relying on it, not assume every override is
   atomic.
3. **Split the API** into `apply_batch` (non-atomic, explicitly documented
   as such) and `transact` (atomic — `MirroredStore` would then need to
   either implement option 1's fix to keep offering `transact`, or NOT
   override `transact` at all and inherit the default per-op-atomic-only
   loop, making the type system itself carry the honest distinction).

Whichever you pick, the resolution must leave NO code path where a caller
can reasonably read `MirroredStore::transact`'s doc comment / trait
contract and conclude it delivers atomicity it does not.

## Definition of done

- The audit's finding is written explicitly into the commit message
  (latent bug vs. contract-honesty-only), with the specific call sites
  checked named.
- If you chose option 1 (real atomicity): a concurrent-reader test that,
  against TODAY's code (before your fix), CAN observe a mid-batch partial
  state (prove this red first, exactly as the task's own zero-trust
  convention requires), and CANNOT after your fix (green). Use this
  codebase's established `TEST_*` pause-seam convention to park the
  transact loop between the two per-op loops (or mid-loop) and issue a
  concurrent read deterministically — no sleeps.
- If you chose option 2 or 3 (stop claiming atomicity): a test asserting
  the capability flag / API split is HONEST — i.e. a caller that checks
  `supports_atomic_transact()` (or uses `apply_batch` vs `transact`) gets
  an accurate answer for `MirroredStore`, and the trait doc no longer
  overpromises.
- `cargo fmt -p shamir-storage -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-storage --full` green (extend to
  `-p shamir-engine` too if your fix touches call sites there, e.g. for a
  capability-flag check added to `repo_types.rs`'s usage).
- Do not touch F-59's already-correct error-atomicity ordering (mirror
  commits before primary is touched at all) — this task is scoped to the
  VISIBILITY question only.
- Do not run this task concurrently with any other task touching
  `storage_mirrored.rs` or `types.rs`'s `Store` trait.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
