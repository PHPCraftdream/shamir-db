# Brief — #1039: unique-index validation must detect two guards claiming the same key within one open transaction

## Context

S.H.A.M.I.R. Database. Found during #1035's investigation, independently
documented in the repo (not a regression from this session). Both
unique-index validation points check ONLY against durable committed
state — never against other not-yet-committed guards from the SAME
transaction:

- **Stage-time** (`crates/shamir-index/src/base_index/
  index_manager_unique.rs`, `check_unique_key` ~L200-220, called from
  `validate_unique_for_create_with_defs` ~L58-78): `self.info_store.get(index_key)`
  only — genuinely optimistic by design (its own doc comment at
  `pre_commit.rs:578` confirms: "Stage-time `validate_unique_*` is
  optimistic — it reads pre-commit state").
- **Commit-time authoritative re-check** (`crates/shamir-engine/src/tx/
  pre_commit.rs`, Phase 2.6, ~L590-602): `for g in &tx.unique_guards { ...
  tbl.info_store().get(g.index_key) ... }` — same durable-state-only
  check, run under the per-table `unique_write_lock` that correctly
  excludes ALL other concurrent writers. But it never compares one guard
  in this loop against ANOTHER guard in `tx.unique_guards` — a plain
  `Vec<UniqueGuard>` (`crates/shamir-tx/src/tx_context.rs:251`) with no
  dedup, appended via `record_unique_guard` (`tx_context.rs:485`).

**Consequence**: two operations within the SAME open transaction that
both claim the same unique key both pass BOTH checks (the durable store
shows `NotFound` for both, since neither has actually written its
posting yet — that happens later, in Phase 5c). Whichever one's posting
write lands last in Phase 5c silently wins with a last-writer-wins
overwrite — no `TxError::UniqueViolation`, no signal at all. Confirmed
by reading Phase 2.6's loop directly: it is genuinely a per-guard,
independent check with no cross-guard awareness.

**Pre-existing intra-batch dedup does NOT cover this.**
`table_manager_tx_ops.rs`'s `insert_tx_many_bytes` (~L718-736) has a
`batch_seen: TFxSet<(u64, Vec<u8>)>` — but it is a LOCAL variable, scoped
to that ONE call. It catches duplicates within a single
`insert_tx_many_bytes` invocation (e.g. one `Batch` insert op with
multiple rows) but NOT across separate calls within the same
transaction (e.g. two distinct `Batch` ops, or separate `for_each`
iterations each triggering their own `insert_tx_many_bytes`). Already
flagged as "STILL OPEN" in existing test comments: `crates/shamir-engine/
src/query/batch/tests/for_each_tests.rs:204-214,645-690` and `crates/
shamir-client/tests/batch_for_each_e2e.rs:159-202` — read these for the
exact scenario they already describe (distinct from #987, which fixed a
related but different timing issue).

## Already investigated — recommended fix location, verify before implementing

**Recommendation: fix this centrally in Phase 2.6** (`pre_commit.rs`,
~L590), not by extending stage-time validation. Reasoning, verify or
refute:

- Phase 2.6 is the SINGLE authoritative choke point every unique guard
  passes through regardless of HOW it was staged (single insert, one
  `Batch` op, multiple `Batch` ops, `for_each` iterations, mixed
  insert/update) — fixing here closes the gap universally, matching the
  task's own scope note ("затрагивает ЛЮБОЙ транзакционный batch с >1
  операцией"). It already runs under the per-table `unique_write_lock`
  (held since Phase 2.5), so no new locking is needed.
- A stage-time fix (extending `validate_unique_for_create` to also see
  `tx.unique_guards`) would need `TxContext` threaded into a currently
  tx-unaware function, and — being stage-time — is inherently optimistic
  per the codebase's own existing doc comment; it would not uniformly
  cover every call pattern the way a single Phase 2.6 check does.

**Design sketch** (verify/adjust after reading the current code, this is
a starting point not a mandate): within the existing `for g in
&tx.unique_guards` loop (or a pass immediately before it), track a
`seen: TFxMap<(u64, Bytes), RecordId>` keyed by `(table_token,
index_key)` → the FIRST guard's owner seen for that key. For each guard:
if its key is already in `seen` with a DIFFERENT owner → this IS an
intra-tx collision, return `TxError::UniqueViolation` (same error type
the durable-state check already returns, for a consistent client-facing
contract) BEFORE even touching `info_store`. If already in `seen` with
the SAME owner (e.g. an update re-validating its own key twice) → not a
conflict, proceed to the existing durable-state check as today. If not
yet in `seen` → insert it, proceed to the existing durable-state check.
Order matters: run the intra-tx check first (cheaper, no I/O) — an
intra-tx collision doesn't need a store round-trip to know it's wrong.

## What to implement

1. **TDD: write the failing test FIRST.** A transaction (or a `Batch`
   with `for_each`, or two separate insert ops in one tx — cover more
   than one construction shape per the "Tests" section below) that
   stages two records claiming the SAME unique-index key within the same
   open transaction. Confirm it currently commits successfully with a
   silent last-writer-wins overwrite (the CURRENT, buggy behavior) before
   writing the fix — this is the red step.
2. **Implement the Phase 2.6 (or wherever your own investigation lands)
   intra-tx dedup check**, returning `TxError::UniqueViolation` for a
   genuine same-tx collision.
3. **Re-run the red test — it must now fail correctly at commit time**
   with `TxError::UniqueViolation`, not succeed with a silent overwrite.
4. Update the "STILL OPEN" comments in `for_each_tests.rs` and
   `batch_for_each_e2e.rs` (cited above) to reflect the fix, converting
   whatever placeholder/skip logic they currently carry into real
   assertions of the new correct behavior.

## Tests

- The TDD red→green test itself (per step 1-3 above), covering at least:
  a `for_each` batch with two iterations claiming the same key (the
  originally-documented scenario), AND a plain two-op `Batch` (not
  `for_each`) claiming the same key across DISTINCT `insert_tx_many_bytes`
  calls (the broader scope the task description calls out) — do not ship
  a fix that only closes the `for_each` instance and leaves the general
  case open.
- A negative/non-regression test: an UPDATE that re-writes a record's OWN
  existing unique value (guard's owner == the record already owning that
  key) must still succeed — don't accidentally turn the "same owner,
  same key" self-write case into a false-positive violation.
- A test with TWO DIFFERENT unique indexes each independently claimed
  once in the same tx — must succeed (proves the fix keys on
  `(table_token, index_key)`, not just `index_key`, so no cross-index
  false collision).
- Confirm the existing intra-batch (`batch_seen`) dedup test coverage
  still passes unchanged (no regression to the already-working
  single-call case).

## Constraints

- Follow `CLAUDE.md`: `Result<T, E>` conventions, tests in `tests/`
  directories, imports at top of file, TDD discipline (red test
  committed conceptually before green, even if landed in one commit —
  the PROCESS matters, show your work in the report).
- This is a **correctness fix in the transaction commit hot path** — be
  precise about performance: the added check must not introduce O(N²)
  behavior for large batches beyond what's already implied by
  `tx.unique_guards`' size (a `TFxMap`/`TFxSet` lookup is O(1) amortized,
  a linear re-scan of the guard list per guard would be O(N²) and is NOT
  acceptable for a large batch — use a hash-keyed structure, not nested
  loops).
- Gate: `cargo fmt -p shamir-engine -p shamir-tx -p shamir-index --
  --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-engine -p shamir-tx -p shamir-index -p
  shamir-client --full`. Use the wrapper, never raw `cargo test`/`cargo
  nextest run`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.
⛔ Do not create scratch files at the repo root.

## Definition of done

- [ ] Verified (or refuted, with a clear counter-argument) the
      recommended Phase 2.6 fix location above.
- [ ] TDD: failing test written and confirmed red against the CURRENT
      (buggy) code before implementing the fix.
- [ ] Intra-tx dedup check implemented at O(1)-amortized cost per guard,
      not O(N²).
- [ ] Fix covers BOTH `for_each`-style and plain multi-op `Batch`
      scenarios, not just the originally-documented `for_each` case.
- [ ] Self-write (same owner, same key) explicitly NOT treated as a
      violation — tested.
- [ ] Cross-index independence explicitly tested (no false collision
      across different unique indexes).
- [ ] "STILL OPEN" comments in the two named test files updated to
      reflect the closed gap.
- [ ] fmt/clippy/test gates green, real output reported.
