# Brief for F-66 (#892, P1) — remove `std::sync::Mutex` from `TxContext::ri_barrier_tokens`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. An independent readonly review of
snapshot `e145b1d3` (`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`,
section P1-2) flagged `TxContext::ri_barrier_tokens` (`crates/shamir-tx/src/tx_context.rs:279`)
as a `std::sync::Mutex<TFxSet<u64>>`, which directly violates this
project's NORMATIVE code ideology (see this repo's `CLAUDE.md`, pillar 1
and the concurrency-invariants table): `std::sync::Mutex` is BANNED on
hot paths, permitted only as a low-frequency/setup-only fallback WITH an
inline comment naming the contention model. The commit path is a hot
path, and `std::sync::Mutex` additionally carries poisoning semantics — a
panic anywhere under the guard permanently breaks every later commit that
shares the same `TxContext`.

**Verified mitigating facts (do not need to re-derive, but keep them in
mind while choosing a fix):**

- The guard is never held across an `.await` in the current code
  (`record_ri_barrier`, `ri_barrier_tokens_is_empty`,
  `append_ri_barrier_deps` — all in `tx_context.rs:658-679` — lock, do a
  synchronous op, drop the guard, return).
- Contention is expected to be low (recorded by at most a handful of FK
  reverse-check scans per transaction).
- This is a correctness-of-*style* / robustness issue (banned primitive +
  poisoning risk), not a known live deadlock — that is why it is P1, not
  P0.
- The one call site checked so far (`fk_restrict.rs:146`, inside
  `for parent_val in values_for_field { child_has_reference(&child_table,
  &rref.child_field, parent_val, tx).await ... }`) calls
  `child_has_reference(..., tx: &TxContext)` **sequentially in a loop**,
  not concurrently (no `join_all`/`FuturesUnordered` fan-out over `tx`
  visible at that site) — this is a hint that threading `&mut TxContext`
  through might be more tractable than it first looks, but you must
  verify this holds at EVERY call site before relying on it, including
  the ones in `fk_on_update.rs` and `fk_actions.rs` (`child_has_reference`
  appears in both, plus the cascade/on-update FK scan entry points
  `tx.record_ri_barrier(...)` is called from — see `grep -n
  "record_ri_barrier\|child_has_reference" -r crates/` for the full call
  graph, spanning `fk_restrict.rs`, `fk_on_update.rs`, `fk_actions.rs`).

## What to do

1. **Read `crates/shamir-tx/src/tx_context.rs:255-280` and `:645-679`**
   for the full doc-comment context and the three methods
   (`record_ri_barrier`, `ri_barrier_tokens_is_empty`,
   `append_ri_barrier_deps`) that touch this field. Also read the sibling
   `footprint_tokens: TFxSet<u64>` field just above (`:126` /
   `index2_stage_gens`'s doc comment references it) — it's the same
   *kind* of per-tx accumulator but NOT wrapped in a `Mutex`, because
   every site that touches it holds the tx by `&mut`. That's the
   reference shape to aim for if option (a) below works out.
2. **Trace every call site** that reaches `record_ri_barrier`,
   `ri_barrier_tokens_is_empty`, or `append_ri_barrier_deps` — both the
   FK reverse-check scans (`fk_restrict.rs::child_has_reference`,
   `fk_on_update.rs::child_has_reference`, `fk_actions.rs`'s equivalent
   probes — grep for `tx.record_ri_barrier`) and the commit-path readers
   (Phase 2-bis / commit-lock acquisition in `crates/shamir-engine/src/tx/`
   — grep for `ri_barrier_tokens_is_empty` and `append_ri_barrier_deps`).
   For each, determine: does the caller hold `tx` by `&TxContext` or
   `&mut TxContext`? Is it called concurrently with any sibling call that
   also needs `tx` access (i.e. would `&mut` create an aliasing
   conflict)?
3. **Pick a fix, in this preference order** (from the task description —
   confirm your choice against what step 2 found, and explain in your
   summary why the earlier options were or weren't viable):
   - **(a) Change the FK-scan API to take `&mut TxContext`** so no
     interior mutability is needed at all — cleanest if the borrow
     checker (and the concurrency shape found in step 2) permits. This
     would make `ri_barrier_tokens` a bare `TFxSet<u64>` exactly like
     `footprint_tokens`.
   - **(b) Collect dependencies in operation-local state and merge at
     staging time** — if (a) doesn't work because some caller genuinely
     needs shared/concurrent access to `tx`, have each scan function
     return (or accumulate into a caller-local `Vec<u64>`/`TFxSet<u64>`)
     the tokens it touched, and have the staging entry point (which DOES
     own `&mut TxContext` — confirm this) merge them into
     `ri_barrier_tokens` in one place, once.
   - **(c) If shared interior mutability is genuinely unavoidable**, use
     a lock-free structure per this repo's pillar 5 (e.g. `scc::HashSet`
     or equivalent) instead of `std::sync::Mutex`. Do NOT simply swap to
     `tokio::sync::Mutex` — that primitive is the sanctioned exception
     for guards held across `.await`, which is confirmed NOT the
     situation here; using it anyway would just trade one wrong-tool
     choice for another.
4. **Implement the fix**, keeping the public behavior of
   `record_ri_barrier` / `ri_barrier_tokens_is_empty` /
   `append_ri_barrier_deps` (or their renamed/reshaped equivalents)
   observably identical from the commit pipeline's point of view — same
   set of tokens recorded, same predicate deps produced, same
   zero-overhead empty-check fast path. This is a structural/robustness
   change, not a behavior change.
5. **Update or add tests.** `crates/shamir-engine/src/query/batch/tests/fk_ri_barrier_tests.rs`
   already exercises this mechanism — read it first and confirm your
   change keeps it green, adding a new test only if the refactor
   introduces a new observable seam worth locking down (e.g. if you go
   with option (a) and want to prove the borrow-checker-enforced
   exclusivity actually prevents a class of bug the old `Mutex` masked).

## What NOT to do

- Do NOT touch F-55/F-56/F-57/F-58/F-59/F-60/F-61/F-62/F-63/F-65 (other
  already-landed or in-flight tasks from the same review) or F-67 (#893,
  a separate pending task).
- Do NOT swap to `tokio::sync::Mutex` — confirmed wrong tool for this
  case (see step 3c above).
- Do NOT change the commit pipeline's actual conflict-detection
  semantics (Phase 2-bis / `predicate_conflicts_batch` / the RI barrier
  concept itself) — this task is about the STORAGE PRIMITIVE
  (`Mutex<TFxSet<u64>>` vs. something else), not the RI-barrier
  mechanism's design.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-tx -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- TDD: if you add a new test, write it first, confirm it's meaningful
  (fails against a deliberately-reverted old shape if applicable), then
  make the fix, confirm green.
- Clean up any scratch/debug files created in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-tx -p shamir-engine -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-tx -p shamir-engine --full
```

Plus a personal re-read of the full diff and a check that no call site
was missed (i.e. `grep -rn "ri_barrier_tokens\|record_ri_barrier"` across
`crates/` shows a consistent picture, no stray old-shape leftover).

When done, give your final summary as plain text: which fix option you
chose and why (referencing what the call-site trace in step 2 actually
found), the exact diff shape, which tests changed/were added, and
confirmation fmt/clippy/tests are clean.
