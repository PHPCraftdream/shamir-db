# F-40b Step 1 — RI barrier mechanism design spike (#855)

**Status:** spike complete. **Recommendation: adopt option 2 (isolation-
independent "RI barrier") from the F-40 memo**, with the two open design
questions settled: **Q1 — flat `TFxSet<u64>` of table-tokens** (the simpler
shape); **Q2 — reuse the existing `PhantomConflict` variant / `"tx_conflict"`
wire code** (no distinct error code). The prototype proves the mechanism: the
race test passes deterministically, the quiescent trial shows 0/50 spurious
aborts, and the existing 63 FK tests are unaffected.

The prototype code described in §2 is committed alongside this memo as a
clearly-scoped spike artifact. Step 2 (#856) will extend it to all recording
sites and all commit-pipeline guard sites per §5.

---

## 1. What this spike settled

### 1.1 Open question 1 — token shape: flat `TFxSet<u64>` (DECIDED)

**Decision: flat `TFxSet<u64>` of `table_token`s, mirroring `footprint_tokens`'
shape exactly.** Richer `PredicateDep::IndexRange` values are NOT needed.

**Investigation and reasoning:**

1. `fk_restrict.rs::child_has_reference` (the RESTRICT reverse-check scan,
   `:292-373`) does **not** route its scan through an indexed predicate-
   recording path. It uses `table.list_stream_tx(Some(tx), batch_size)`
   (`:353`), and `list_stream_tx` (`table_manager_streaming.rs:243-248`)
   records a coarse `PredicateDep::TableScan { table_token }` **unconditionally
   at scan entry** when `tx.isolation == Serializable` — never an `IndexRange`.

2. There IS an index-aware predicate-recording path —
   `filter_stream_tx` (`table_manager_streaming.rs:271-300`), which calls
   `predicate_to_index_range` to compute tighter `IndexRange` deps and falls
   back to `TableScan` only when no sorted index serves the filter. But the FK
   reverse-check scans do NOT use `filter_stream_tx` — they do a raw
   `list_stream_tx` full-scan and match the FK field manually in the stream
   body (`record_field_matches_by_id`). So the EXISTING Serializable path
   already accepts the coarse `TableScan` dep for these same scans, never an
   `IndexRange`.

3. **Critically**, `child_has_reference` has an **index fast-path** (`:311-333`)
   that short-circuits *before* `list_stream_tx`: if a single-field index covers
   the FK field, it does a direct `lookup_by_index` and returns early (no scan
   at all). In that case the Serializable `TableScan` predicate is **never
   recorded** under the existing path — the index lookup itself records no
   predicate dep. A well-formed FK requires a supporting index
   (`admin_schema.rs::validate_fk_indexes` enforces this at DDL time), so the
   common case (indexed FK, RESTRICT check finds no child) hits the fast-path
   and records nothing under Serializable today. The barrier recording at
   function ENTRY (this spike's prototype, §2.1) **closes this gap too** — it
   fires regardless of which sub-path (index fast-path or scan fallback) is
   taken.

4. Conclusion: the existing Serializable predicate_set path gets no tighter dep
   than a coarse `TableScan` for these scans (and often nothing at all for the
   indexed case). Matching that coarseness with a flat `TFxSet<u64>` is
   consistent, correct, and simpler than threading `IndexRange` values. It also
   mirrors `footprint_tokens` exactly, keeping the two isolation-independent FK
   token sets parallel in shape.

**Interior mutability note:** `footprint_tokens` is a bare `TFxSet<u64>`
(mutated via `&mut self` at staging time), but the barrier's recording site
(`child_has_reference`) holds the tx by shared reference (`&TxContext`). The
field is therefore `Mutex<TFxSet<u64>>` — the same `std::sync::Mutex`
rationale as `predicate_set`'s own `Mutex<Vec<_>>`
(`predicate_set.rs:10-14`: never held across `.await`, tx-scoped, uncontended).
`std::sync::Mutex` is not in `clippy.toml`'s `disallowed-methods` /
`disallowed-types`. The inner type matches `footprint_tokens` exactly.

### 1.2 Open question 2 — error-code contract: reuse `"tx_conflict"` (DECIDED)

**Decision: reuse the existing `TxError::PhantomConflict` variant, which maps
to the `"tx_conflict"` wire code at every mapping site. No distinct error code
for RI-barrier aborts.**

**Investigation and reasoning:**

1. The codebase has an established convention: ALL SSI-class commit conflicts
   map to the SAME `"tx_conflict"` wire code. Verified at every mapping site:
   - `batch_execute.rs:654-656` — `SsiConflict`, `PhantomConflict`, `Wounded`
     all → `"tx_conflict"`.
   - `repo_instance.rs:1099-1101` — `PhantomConflict` and `SsiConflict` →
     `"tx_conflict"`.
   - `db_tx.rs:253-257` — `SsiConflict`, `PhantomConflict`, `Wounded` →
     `"tx_conflict"`.

2. `CommitError` is a type alias for `TxError`
   (`tx/mod.rs:20: pub use commit::{..., TxError as CommitError}`). So reusing
   `PhantomConflict` needs **zero** new error plumbing — no new enum variant, no
   new wire mapping, no new `thiserror::Error` entry.

3. The only commit conflict with a DISTINCT code is `CasConflict` →
   `"version_conflict"` (`batch_execute.rs:664-665`), and that distinctness
   exists ONLY because CAS has its own pre-existing staging-time error code
   (`"version_conflict"`) that the commit-time failure must align with. The RI
   barrier has no such pre-existing code to align with — it is brand-new.

4. A distinct code would require: a new `TxError` variant, new wire mappings at
   3+ sites, AND would NOT be covered by any existing client retry logic —
   forcing every client to learn a new code for zero behavioral benefit. Both
   RI-barrier aborts and generic phantom conflicts have identical retry
   semantics (re-run against a fresh snapshot), so distinguishing them provides
   no actionable information to a client.

5. The `dep` string in the `PhantomConflict { dep }` variant is a free-form
   diagnostic — the barrier's deps are formatted identically to existing
   predicate deps (`format!("{:?}", deps[idx])`), so diagnostics are not lost.
   A client that wants to know "was this an RI barrier" can inspect the error
   message string, but the wire CODE stays `"tx_conflict"`.

6. `interactive_tx.rs::commit_interactive_tx` (`:131-136`) returns
   `Result<TxOutcome, CommitError>` — a plain `repo.commit_tx(tx).await`. It has
   no coded-error convention of its own; it surfaces whatever `TxError` variant
   the commit pipeline produced. Reusing `PhantomConflict` means an explicit-tx
   client already handling `tx_conflict` (which it must, if it ever opens a
   Serializable tx) picks up RI-barrier aborts for free.

---

## 2. What was prototyped

### 2.1 Prototype code (committed alongside this memo)

Files changed:

- **`crates/shamir-tx/src/tx_context.rs`** — new field
  `ri_barrier_tokens: Mutex<TFxSet<u64>>`, initialized empty in `TxContext::new`.
  Three new methods: `record_ri_barrier(&self, table_token)` (records
  regardless of isolation, via interior mutability),
  `ri_barrier_tokens_is_empty(&self)` (the zero-overhead commit-path gate), and
  `append_ri_barrier_deps(&self, deps)` (builds `TableScan` deps under a single
  lock for Phase 2-bis).

- **`crates/shamir-engine/src/query/batch/fk_restrict.rs`** — ONE recording
  site: `child_has_reference` (`:292`) now calls `tx.record_ri_barrier(
  table.table_token())` at function ENTRY (before the index fast-path), so it
  fires regardless of isolation AND regardless of which sub-path (index lookup
  or scan fallback) is taken.

- **`crates/shamir-engine/src/tx/pre_commit.rs`** — ONE guard-widening site:
  the main Phase 2-bis check in `pre_commit_locked_validate` (`:~460-467`)
  widened from `Serializable && !predicate_set.is_empty()` to ALSO fire when
  `!tx.ri_barrier_tokens_is_empty()`. The deps slice merges Serializable
  `predicate_set` deps with RI barrier `TableScan` deps, then calls the same
  `gate.predicate_conflicts_batch` verbatim. The `PhantomConflict` error variant
  is reused unchanged.

- **`crates/shamir-engine/src/tx/commit.rs`** — `:~742` commit-lock acquisition
  widened to ALSO take `gate.commit_lock()` when `ri_barrier_tokens` is
  non-empty. A Snapshot tx that recorded an FK-child scan needs the same
  validate→publish window serialization Serializable already relies on (CRIT-4
  / #438), so the barrier's `predicate_conflicts_batch` scan sees a stable
  commit window. This IS load-bearing for the mechanism's correctness: without
  it, a concurrent committer could publish (advance `last_committed`) between
  this tx's Phase 2-bis check and its own publish, slipping past the barrier.
  The widened guard does not affect any Snapshot tx with an empty
  `ri_barrier_tokens` (the `is_empty()` check short-circuits before any lock
  work).

- **`crates/shamir-engine/src/query/batch/tests/fk_ri_barrier_spike_tests.rs`**
  — new file, 2 tests (the race harness + quiescent trial).

- **`crates/shamir-engine/src/query/batch/tests/mod.rs`** — registers the new
  test module.

### 2.2 The harness

Adapts `fk_race_closure_tests.rs`'s `RaceInjectingResolver` shape (identical
struct: `DbInstance`-backed, shared `ValidatorRegistry`, `resolve_repo`-ordinal
injection, separate `TxTestResolver` for the writer) but with an EXPLICIT
`Snapshot` transaction as the outer operation:

1. Seed parent row (plain autocommit insert).
2. Open an explicit `Snapshot` tx (`open_interactive_tx`).
3. Execute the DELETE inside the open tx (`execute_in_open_tx`) — the RESTRICT
   scan runs and records the RI barrier token. At `resolve_repo` ordinal **2**
   (`plan_cascade` → `discover_action_refs`, which runs strictly AFTER
   `check_fk_restrict` has fully returned — the explicit-tx DELETE arm has only
   2 `resolve_repo` calls, vs the implicit arm's 4, because the explicit arm
   does NOT call the isolation-upgrade hook), the injected writer fires: a
   complete autocommit child INSERT referencing the parent, committed to full
   visibility.
4. Commit the explicit tx (`commit_interactive_tx`) — Phase 2-bis fires for the
   Snapshot tx (widened guard), detects the writer's footprint on the child
   table, aborts with `PhantomConflict`.

The `resolve_repo`-ordinal injection seam is identical to
`fk_race_closure_tests.rs`'s; only the ordinal (2 vs 4) and the outer tx shape
(explicit `open_interactive_tx` / `execute_in_open_tx` / `commit_interactive_tx`
vs implicit `execute_batch`) differ.

### 2.3 Results (run 2026-07-28)

```
./scripts/test.sh -p shamir-engine -- fk_ri_barrier_spike
```

| Test | Result | What it proves |
|---|---|---|
| `explicit_snapshot_restrict_race_closed_via_ri_barrier` | **PASS** (0.163s) | Explicit-Snapshot parent DELETE racing a concurrent child INSERT: the barrier catches it — `commit_interactive_tx` returns `Err(PhantomConflict)`. Parent still exists (count=1), child still exists (count=1) post-abort. No dangling reference. |
| `quiescent_explicit_snapshot_restrict_delete_does_not_spuriously_abort` | **PASS** (0.280s) | **50/50 trials, 0 spurious aborts.** No concurrent writer — the explicit-Snapshot FK-parent delete never spuriously aborts via the barrier. The quiescent delete commits cleanly every trial. |

Full FK suite (all existing tests + 2 new spike tests):

```
./scripts/test.sh -p shamir-engine -- fk
```

**Summary: 65 tests run: 65 passed, 1644 skipped.** Zero regressions — the
existing 63 FK tests (including the implicit-path race closure tests and the
implicit-path quiescent trial) all pass unchanged with the widened guards.

### 2.4 Why the commit-lock widening is load-bearing

The F-40 memo §4.2 point 4 states the commit-lock must be widened. This spike
confirms it is load-bearing for correctness, not just consistency:

- The lock-free commit path (`commit_tx_lockfree`) takes `commit_lock` only for
  Serializable / CAS txs. A Snapshot tx that recorded an RI barrier token runs
  Phase 2-bis (which walks `commit_write_log`'s `(snapshot, last_committed]`
  window) WITHOUT the lock by default.
- Without the lock, a concurrent committer could advance `last_committed`
  (via `version_guard.commit()` in `materialize`) between this tx's Phase 2-bis
  scan and its own `record_commit_writes` + publish — the barrier scan would
  use a stale `last_committed` that doesn't include the concurrent committer's
  record, missing the conflict.
- With the widened guard, the barrier tx takes the lock, serializing its
  validate→publish window exactly as Serializable already does (CRIT-4 / #438).

The widened guard does NOT affect any Snapshot tx with empty
`ri_barrier_tokens` (the common case): `is_empty()` is a single check before
any lock acquisition, so non-FK-parent Snapshot deletes/inserts are
byte-identical in behavior.

---

## 3. What this spike did NOT do (Step 2's job)

1. **The 2 remaining recording sites** — `fk_actions.rs` (cascade probes) and
   `fk_on_update.rs` (on-update probes) still record nothing into
   `ri_barrier_tokens`. The prototype only instruments `fk_restrict.rs` (the
   smallest of the three, per the brief's scope).

2. **The 2 remaining commit-pipeline guard sites** —
   `pre_commit.rs:614` (the legacy AsyncIndex path's Phase 2-bis) and
   `group_commit.rs:184-202` (the inter-batch phantom check for grouped
   commits) are NOT widened. Only the main lock-free path's Phase 2-bis
   (`pre_commit_locked_validate`) is widened.

3. **The retry wrapper for explicit txs** — `interactive_tx.rs`'s commit path
   surfaces the `PhantomConflict` (as `CommitError`, which maps to
   `"tx_conflict"`) directly to the caller. No automatic retry is added for
   explicit txs (the client owns the retry decision). This is the settled
   design (§1.2): reuse the existing error code, let the client retry on
   `"tx_conflict"` if it wants.

4. **`KNOWN_LIMITATIONS.md`** — NOT updated (Step 2's job, once the full
   implementation lands).

5. **End-to-end tests for the remaining actions** (CASCADE, SET NULL,
   on-update) via the explicit-tx path — Step 2 adds these alongside the
   remaining recording sites.

---

## 4. Decision summary

| Question | Decision | Rationale |
|---|---|---|
| Q1: token shape | Flat `TFxSet<u64>` of table-tokens | FK reverse-check scans use `list_stream_tx` (coarse `TableScan`), never `filter_stream_tx` (`IndexRange`). The existing Serializable path gets no tighter dep. Flat set mirrors `footprint_tokens`. |
| Q2: error code | Reuse `PhantomConflict` → `"tx_conflict"` | Established convention (all SSI conflicts → `tx_conflict`). Zero new plumbing. Identical retry semantics. No behavioral benefit to a distinct code. |

---

## 5. Implementation plan for Step 2 (#856)

Step 2 extends the prototype to the full production implementation. Each touch
point is a mechanical mirror of the prototype's pattern (record at scan entry
regardless of isolation; widen the commit guard's condition by
`\|\| !tx.ri_barrier_tokens_is_empty()`).

### 5.1 Remaining recording sites (2 sites)

1. **`crates/shamir-engine/src/query/batch/fk_actions.rs`** — the cascade
   probes in `plan_cascade` / `discover_action_refs` that scan child tables for
   CASCADE / SET-NULL actions. Each child-table scan entry point needs
   `tx.record_ri_barrier(table.table_token())` at scan entry, mirroring
   `fk_restrict.rs::child_has_reference`. Look for every
   `table.list_stream_tx(Some(tx), ...)` call site in this file and add the
   recording before it.

2. **`crates/shamir-engine/src/query/batch/fk_on_update.rs`** — the on-update
   probes in `plan_fk_on_update` / `discover_on_update_refs` that scan child
   tables for on-update CASCADE / SET-NULL / RESTRICT. Same pattern: each
   `list_stream_tx` entry point gets `tx.record_ri_barrier(...)`.

### 5.2 Remaining commit-pipeline guard sites (2 sites)

3. **`crates/shamir-engine/src/tx/pre_commit.rs:614`** — the legacy AsyncIndex
   commit path's Phase 2-bis check (in `pre_commit_locked`). Widen identically
   to the prototype's `:460-467` widening: merge `predicate_set` deps with
   `ri_barrier_tokens` deps, call `predicate_conflicts_batch`. This path is
   opt-in (`CommitVisibility::AsyncIndex`) but reachable by a Snapshot FK-parent
   delete that opts in.

4. **`crates/shamir-engine/src/tx/group_commit.rs:184-202`** — the inter-batch
   phantom check for grouped commits. Widen the condition to also fire when
   `ri_barrier_tokens` is non-empty, merging the barrier deps into the batch's
   predicate-deps slice. (Note: `group_commit.rs:635` already maps
   `TxError::PhantomConflict` → `DbError::Conflict(...)` → `"tx_conflict"`, so
   the error code is consistent.)

### 5.3 Additional Step 2 work

5. **End-to-end explicit-tx race tests** for CASCADE, SET NULL, and on-update
   actions, mirroring the prototype's RESTRICT test in
   `fk_ri_barrier_spike_tests.rs`. These can be added to the same file (rename
   it from "spike" to a permanent name, or fold into
   `fk_race_closure_tests.rs`).

6. **Update `KNOWN_LIMITATIONS.md:139-145`** — rewrite the "Residual scope"
   bullet from "open for explicit Snapshot" to "CLOSED for explicit Snapshot
   via the RI barrier", mirroring how the F-28 Step 5 entry already reads for
   the implicit path.

7. **Decide on a client-retry contract for explicit txs** — whether
   `interactive_tx.rs` should gain an optional bounded-retry wrapper for
   `tx_conflict` (like the implicit path's `retry_on_tx_conflict`), or leave
   retry entirely to the client. This spike's settled Q2 (§1.2) means the wire
   code is already `"tx_conflict"`, so this is a product/UX decision, not a
   correctness one. Recommendation: document that explicit-tx clients should
   retry on `"tx_conflict"` if they want transparent race resolution, and leave
   the engine's explicit-tx API retry-free (the client owns the lifecycle).

8. **Dep-dedup micro-optimization** — when a Serializable tx has BOTH a
   `predicate_set` `TableScan` dep and an `ri_barrier_tokens` entry for the same
   table_token (which happens when `child_has_reference`'s scan fallback runs
   under Serializable), the merged deps slice has a redundant dep. Harmless
   (`predicate_conflicts_batch` short-circuits on the first conflict), but Step
   2 can dedup the token sets before building the slice if profiling warrants.

---

## 6. Exact commands to reproduce

```
# Compile check:
cargo check -p shamir-engine --tests

# Run the spike's race harness (2 tests):
./scripts/test.sh -p shamir-engine -- fk_ri_barrier_spike

# Run the full FK suite (65 tests, includes spike + existing closure tests):
./scripts/test.sh -p shamir-engine -- fk

# Full integration/e2e scope:
./scripts/test.sh -p shamir-engine --full

# Gate checks:
cargo fmt -p shamir-engine -p shamir-tx -- --check
cargo clippy -p shamir-engine -p shamir-tx --all-targets -- -D warnings
```

Expected: `2 tests run: 2 passed` for the spike invocation; `65 tests run: 65
passed` for the full FK suite; `0` spurious aborts over the 50-trial quiescent
test.
