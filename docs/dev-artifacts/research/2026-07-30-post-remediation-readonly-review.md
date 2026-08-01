# S.H.A.M.I.R. — post-remediation readonly review (F-55 … F-67)

**Date:** 2026-07-30
**Mode:** read-only. No build, no test, no bench, no git mutation. Every claim
below is derived from reading the landed diffs and the current tree.
**Snapshot:** `28d39f31` (working tree clean except untracked docs).
**Range reviewed:** `e145b1d3..28d39f31` (23 commits).
**Baseline for "what was supposed to be fixed":**
`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md` +
`docs/dev-artifacts/roadmap/2026-07-29-pre-alpha-remediation.md`.

> A second, independently-produced review of the same range exists untracked at
> `docs/dev-artifacts/research/2026-07-30-new-wave-readonly-review.md`. I read
> only its section headings, after forming my own findings, to check for
> convergence. Where we converge I say so; the three P0s below include two that
> that review does **not** contain.

---

## 1. Verdict

**The remediation wave is real work, correctly aimed, and mostly correctly
executed.** Every one of F-55…F-67 addresses the thing the original review
actually flagged — none of them is a comment-only or test-only pretend fix. The
F-56 memory-model proof in particular is genuinely correct (I re-derived it
independently, including the `flag` true→false transition case the written
proof glosses over) and the honesty about loom's scope is exemplary. F-59's
reordering is right and minimal. F-55 and F-65 are exactly the right shape.

**But the codebase is still NO-GO for a first public tag**, because closing
those seven P0s exposed — and in two cases *created* — three new defects of the
same severity class:

| # | Severity | One line |
|---|---|---|
| **N-1** | **P0** | The AsOf index-seek gate reads `0` for any index after a restart, after `CREATE INDEX`, and after `RENAME INDEX` → silently wrong AsOf pages. F-67 *widened* this. |
| **N-2** | **P0** | `needs_write_barrier()` is a non-atomic read of six independent atomics whose **first** term is a `Relaxed` load; a writer can take the lock-free fast path on a table that already has a live unique index. |
| **N-3** | **P0** | `pre_commit_prelock` holds `WriterDrainGuard`s across `unique_write_lock` acquisition → a genuine 3-party deadlock (`CREATE INDEX` ↔ committer ↔ committer). F-57 made this reachable in practice. |

N-1 and N-2 are silent-wrong-answer / silent-constraint-violation. N-3 is a
hard hang (the `TIMEOUT` class `CLAUDE.md` explicitly says never to tolerate).

New findings: **3 × P0, 6 × P1, 8 × P2.** Detail below.

---

## 2. Did we do it right? — per-task verdict

| ID | Verdict | Notes |
|---|---|---|
| **F-55** `f9eed337` | ✅ Correct | Both sites (`fk_reverse_cache.rs:502`, `fk_on_update.rs:743-756`) now propagate. `get_or_build_by_parent`'s `build().await?` genuinely aborts the CAS-publish. Residual: see **N-8** (concurrent `DROP TABLE` now produces a spurious hard error). |
| **F-56** `7fde958e` | ✅ Correct, proof verified | See §2.1. Wiring into `create_index_v2` is real (`table_manager_index_mgmt.rs:97`). Loom scope disclaimer is honest and accurate. Minor build-hygiene nits: **N-12**. |
| **F-57** `fcaae001` | ⚠️ Correct as far as it goes | All four families now barrier+lock+drain. But it is **CREATE-only** (`DROP`/`RENAME` unprotected — **N-6**), and it converted `create_index` from "unsafe but non-blocking" to "safe but blocks every writer for an O(N) full materialize" (**N-7**). It is also the change that made **N-3** reachable. |
| **F-58** `15b5a729` | ✅ Correct | Symmetric post-scan re-check (`read_asof_seek.rs:293`) is the right minimal closure, and the bump-vs-apply analysis in the module doc is accurate. Does **not** fix the gate's *initial value* problem (**N-1**). |
| **F-59** `a66d68d6` | ✅ Correct | Mirror-first-then-both-subsets is the right ordering; the pinning test was correctly flipped rather than deleted. Residuals **N-13**. |
| **F-60/61/62/63** | ✅ / out of scope | CI hardening. One code-level consequence noted in **N-14**. |
| **F-65** `28d39f31` | ✅ Production code correct, ⚠️ tests weak | All four `_ => continue` sites fixed correctly. `fk_restrict.rs:126-128` correctly left alone (in-memory `HashMap::get`, not I/O). But 2 of 3 tests are weak/invalid oracles — **N-9**. |
| **F-66** `829f1227` | ✅ Correct, ⚠️ one behaviour change | The lock is gone and the API is preserved. But `append_ri_barrier_deps` iteration order became nondeterministic — **N-10**. |
| **F-67** `e7a8c707` | ❌ **Introduced a correctness regression** | The scope narrowing is the right *idea* and the per-index proof transfers — but keying by `name_interned` with a default of `0` broke the gate for newly-created and renamed indexes. See **N-1**. |

### 2.1 F-56's proof — independently re-derived

The written proof (steps 1–5 on `writer_drain_barrier.rs:95-113`) is sound, and
I confirm the underlying claim: `Release`/`Acquire` cannot carry a
cross-atomic dependency where the reader's load returns the *old* value, and
`SeqCst`'s single total order can. Two gaps in the *written* argument that do
not change the conclusion but should be stated:

1. **Step 3 assumes `flag` only ever transitions `false → true`.** It does not:
   `IndexCreateBarrierGuard::drop` (`table_manager_index_mgmt.rs:1113`) stores
   `false`. So a writer's `flag.load == false` could be reading the
   *post-lower* `false`, in which case `flag.load ≻ flag.store(true)` in `S`
   and the chain in step 4 does not close. The conclusion survives, because in
   that case the DDL has already completed its entire guarded region (the guard
   spans the whole `create_*`), so the writer is legitimately post-DDL. Worth
   one sentence in the doc — as written, a future reader can't tell whether the
   author considered it.
2. **The proof covers only the flag atomics, not the first term of the
   disjunction.** `needs_write_barrier()`'s first operand is
   `has_unique_indexes()`, a `Relaxed` load on a *seventh* atomic that the
   proof never mentions. That omission is exactly **N-2**.

---

## 3. New findings — P0

### N-1 (P0). The AsOf seek gate defaults to `0`, so it is disabled after restart, after CREATE INDEX, and after RENAME INDEX

**Where:**
- `crates/shamir-index/src/legacy/sorted_index_manager.rs:227-232`
  (`last_mutation_version` → `None => 0`)
- `crates/shamir-index/src/legacy/sorted_index_manager.rs:191` (map constructed
  empty; `load()` does not seed it)
- `crates/shamir-index/src/legacy/sorted_index_manager.rs:314-331` (`register`
  does not touch the epoch)
- `crates/shamir-index/src/legacy/sorted_index_manager.rs:383-404`
  (`rename_definition` does not carry the epoch across `old_id → new_id`)
- `crates/shamir-engine/src/table/table_manager_sorted_index.rs:134`
  (backfill calls `on_record_created(&id, &record, 0)` — literal version `0`)
- Read sites: `crates/shamir-engine/src/table/read_temporal.rs:108`,
  `crates/shamir-engine/src/table/read_asof_seek.rs:293`

**The invariant the gate is supposed to enforce:** "the *current-state* sorted
index provably mirrors the postings as they were at `pinned_version`". It is
enforced as `last_mutation_version(idx) <= pinned_version`. That is only sound
if the counter is a true high-water over *every* mutation the index has ever
seen. It isn't:

**(a) Restart.** `SortedIndexManager` is reconstructed with an empty map
(`:191`); nothing persists or reseeds the epochs, and `persist_defs` saves only
definitions. After any clean restart every index reads epoch `0`. A client
issuing an `as_of_version(V)` + `ORDER BY indexed_field` + keyset query then
passes the gate for **every** `V >= 0`, and the seek walks a current-state index
that reflects mutations made after `V`. Rows whose indexed value changed after
`V` appear at the *new* ordinal position (wrong order, and dropped entirely if
that position falls outside the page window); rows deleted after `V` have no
posting at all and are silently omitted. `concurrent_modified` cannot catch
either case — that is precisely the argument F-58's own module doc makes.
*This one is pre-existing (F-53b), not introduced by F-67 — but it is
unmitigated and it is a P0.*

**(b) CREATE INDEX after mutations.** `register` (`:314`) leaves the new
`name_interned` absent → epoch `0`. The backfill then calls
`on_record_created(..., 0)`, which inserts the entry with value `0`. So an index
created today over a table mutated for months reads epoch `0`. Before F-67 the
manager-wide counter was already at the latest commit version and the gate
correctly declined; after F-67 it passes. **This variant is a regression
introduced by F-67.**

**(c) RENAME INDEX.** `rename_definition` (`:383`) swaps `name_interned`
in-place and does not move the epoch entry. Post-rename the index reads `0`
while its physical postings carry the full mutation history. **Also a
regression introduced by F-67.**

`try_plan_keyset_seek` (`read_planner.rs:504-524`) resolves the index by *field
path* and returns `def.name_interned`, so a drop-and-recreate under a different
name is transparently picked up with a fresh, wrong epoch.

**Failure scenario (b), concrete:** table `t`, sorted index `by_score` on
`score`. Row `r` has `score = 10`; snapshot version `V = 100`. At `V+1`, `r` is
updated to `score = 9999`. At `V+2`, `DROP INDEX by_score` + `CREATE INDEX
by_score2 (sorted) ON t(score)`. A query `AS OF VERSION 100 ORDER BY score ASC
AFTER (5) LIMIT 10` now takes the seek path (epoch(`by_score2`) = 0 ≤ 100), walks
from `score = 5` forward, and never reaches `r` (its posting is at 9999) — even
though at version 100 `r` sat at `score = 10` and belongs in the page. The page
is short by one row, with `index_used: sorted_idx_*_asof_keyset` in the stats
and no error.

**What a fix looks like:**
- Give the manager a `structural_floor: AtomicU64` and make
  `last_mutation_version(idx) = max(per_index_epoch, structural_floor)`.
- Seed `structural_floor` at construction/`load()` with the repo's current
  commit version (the value the engine already has when it builds the
  `TableManager`), so a restart cannot re-enable the seek for pre-restart pins.
- In `register`, seed the new index's epoch with that same current version
  (the engine caller `create_sorted_index_with_include` has access to it and
  should pass it in rather than the literal `0` at `:134`).
- In `rename_definition`, carry the epoch: `note_mutation_at_version(new_id,
  last_mutation_version(old_id))`.
- Add a test that pins a version, mutates, restarts (and separately: recreates
  the index), and asserts the seek **declines**.

If seeding cleanly is not achievable this cycle, the honest alternative — which
the remediation plan already sanctioned for F-58 — is to disable the AsOf
index-seek arm (`read_temporal.rs:105-115`) for the first tag and keep the
correct full-scan path.

---

### N-2 (P0). `needs_write_barrier()` is a torn read whose first term is `Relaxed` — a writer can miss a freshly-registered unique index

**Where:** `crates/shamir-engine/src/table/table_manager.rs:820-836`;
`crates/shamir-index/src/legacy/index_manager.rs:172-177` (store, `Release`)
and `:191-193` (load, **`Relaxed`**).

```rust
pub(crate) fn needs_write_barrier(&self) -> bool {
    self.index_manager.has_unique_indexes()          // <-- Relaxed load, evaluated FIRST
        || self.index2_create_barrier.load(SeqCst)
        || self.schema_activation_barrier.load(SeqCst)
        || self.regular_index_create_barrier.load(SeqCst)
        || self.unique_index_create_barrier.load(SeqCst)
        || self.sorted_index_create_barrier.load(SeqCst)
}
```

F-56's proof establishes an ordering relationship between the *flag* atomics and
the drain counter. It says nothing about `has_indexes_unique`, and `||`
short-circuits left-to-right, so `has_unique_indexes()` is read **before** any
flag.

**Failure scenario:** table `t` has no unique index. `CREATE UNIQUE INDEX u ON
t(email)` runs: raise `unique_index_create_barrier` → `drain_writers()` →
persist → `collect_all_current_records` → `create_unique_index_from_records`
(this is where `has_indexes_unique` becomes `true`) → return → guard drop clears
the flag → lock released.

Writer `W` calls `insert`:
1. `enter_writer()` — but this happens *after* the DDL's `drain_writers()` has
   already observed `active == 0` and returned, so the drain does not hold it.
2. `has_unique_indexes()` → `false`. This is **correct at that instant**: the
   index is not registered yet.
3. `W` is descheduled (ordinary tokio/OS preemption; nothing here is atomic).
4. The DDL finishes: registers the index, clears the flag, releases the lock.
5. `W` resumes and reads all five flags → all `false`.
6. `needs_write_barrier() == false` → `W` takes the **lock-free fast path**
   (`table_manager_crud.rs:165-172`) on a table that now has a live unique
   index.

`W` therefore performs its uniqueness check and its posting write without
`unique_write_lock`. Two such writers claiming the same `email` can both pass
validation and both publish — the exact duplicate the lock exists to prevent,
and the exact class A9/F-57 claim to have closed. Note this needs no weak-memory
argument at all: it is a plain torn read across a preemption point.

**What a fix looks like (and it is also the best perf fix in this file):**
collapse all six conditions into **one** atomic word on the `TableManager`, e.g.
`Arc<AtomicU32> write_barrier_bits`, with one bit per DDL kind plus a
`HAS_UNIQUE_INDEX` bit that `IndexManager` sets/clears through the same word.
`needs_write_barrier()` becomes a single `SeqCst` load; the F-56 proof then
covers the *entire* predicate instead of five sixths of it, and the writer hot
path drops from six loads across six separately-allocated `Arc` cache lines to
one. A minimal stopgap that fixes only correctness: move
`has_unique_indexes()` to the **last** operand and make its load `Acquire` — then
"all flags false" either means pre-raise (the drain protects us) or post-lower
(the `SeqCst` flag store→load pair gives a synchronizes-with edge, so the
subsequent `has_indexes_unique` load is guaranteed to observe `true`).

---

### N-3 (P0). Deadlock: `pre_commit_prelock` holds writer-drain guards while blocking on `unique_write_lock`

**Where:** `crates/shamir-engine/src/tx/pre_commit.rs:452-489`.

The classification loop (`:454-475`) enters the drain set for **every** table in
`tx.write_set`, keeps the guard for tables whose barrier is down, and only
*after* the whole loop takes the `unique_write_lock`s (`:485-489`). So a
committer can be blocked on table `B`'s lock while holding table `A`'s drain
guard. The module's ABBA-freedom argument (`:476-480`) covers lock ordering
only — drain-set membership is a second, unordered resource acquired outside
that ordering and held across the lock wait.

`create_index` / `create_sorted_index_with_include` / `create_unique_index` /
`create_index_v2` all now do: take `unique_write_lock` → raise flag → `await
drain_writers()`. That is "hold lock, wait for drain" — the exact counterpart
that closes the cycle.

**Failure scenario (one DDL, two committers):** table `B` has a unique index
(so `needs_write_barrier()` is permanently `true` for it); token(`B`) <
token(`A`).

- `T1` (`write_set = {A, B}`): classifies `A` **before** the DDL raises `A`'s
  flag → keeps `A`'s drain guard; classifies `B` → barriered → blocks on
  `B.unique_write_lock`.
- `T2` (`write_set = {A, B}`): classifies **after** the DDL raised `A`'s flag →
  both barriered → sorted acquisition takes `B.unique_write_lock`, then blocks on
  `A.unique_write_lock`.
- `CREATE INDEX ON A`: holds `A.unique_write_lock`, is inside
  `drain_writers()` waiting for `T1`'s guard.

Cycle: `DDL_A → T1 → T2 → DDL_A`. Nothing breaks it; there is no timeout on
`WriterDrainBarrier::drain` (`writer_drain_barrier.rs:177-183` spins on
`yield_now` forever). Two concurrent `CREATE INDEX`es on two tables give the
same cycle without needing a unique index anywhere.

The exposure window is large: the drain guards are deliberately kept alive all
the way through Phase 5c materialize (`:448-451`), i.e. across the WAL write and
publish.

This is not *introduced* by this wave — F-48b established the
hold-guards-across-lock shape — but before F-57 only `create_index_v2` and
schema activation took the lock-then-drain shape. F-57 added three more DDL
entry points and three more always-checked flags, which is what moves this from
theoretical to reachable.

**What a fix looks like:** make "never block on `unique_write_lock` while
holding a `WriterDrainGuard`" a hard invariant. Restructure `pre_commit_prelock`
into: (1) classify with a guard that is dropped immediately in **both**
branches; (2) acquire all `unique_write_lock`s in sorted token order; (3)
re-enter the drain set for the fast-path tables and re-read
`needs_write_barrier()`; (4) if any flipped to `true`, drop everything and retry
the whole prelock (bounded — a DDL always terminates). Step (3) preserves F-56's
bump-before-flag-read ordering; step (4) is the liveness escape.
A `debug_assert` that the drain-guard vec is empty at the top of the lock loop
would pin the invariant.

---

## 4. New findings — P1

### N-6 (P1). F-57's "unified lifecycle" is CREATE-only; DROP and RENAME are still unprotected

`drop_index` / `drop_unique_index` (`table_manager_index_mgmt.rs:624-636`),
`drop_sorted_index` (`table_manager_sorted_index.rs:142-148`), `drop_index2`
(`:662+`) and the sorted half of `rename_index`
(`table_manager_index_mgmt.rs:966-973`, `rename_definition` +
`rekey_sorted_prefix`) take **no** lock, raise **no** flag and perform **no**
drain. `SortedIndexManager::drop_index` (`sorted_index_manager.rs:335-371`)
removes the definition via RCU and *then* streams the posting sweep — a writer
holding a pre-removal `load_local()` snapshot can re-post after the sweep has
passed, leaving orphans. `rename_definition` swaps the name and *then* rekeys,
with `rekey_sorted_prefix`'s "settle re-scan" as the only mitigation.

The commit message and the new doc comments say "unified online CREATE INDEX
lifecycle across all index families", which is accurate — but
`docs/.../CREATE INDEX lifecycle` is now the only half that is unified, and the
original review's P0-3 asked for a lifecycle (`Building → Ready`, `Failed`,
`Dropping`), not just a create barrier. `Dropping` does not exist.

### N-7 (P1). `create_index` / `create_unique_index` now stall every writer for an O(N) full materialize

`create_index` (`table_manager_index_mgmt.rs:507-544`) and
`create_unique_index_locked` (`:587-618`) hold `unique_write_lock` across
`collect_all_current_records()` — which (`table_manager_streaming.rs:452-473`)
materializes the **entire table** into a `Vec<(RecordId, InnerValue)>` — and
then across `create_index_from_records`. Before F-57 the regular path took no
lock at all. So the correctness fix converted a silent-corruption risk into a
table-wide write outage proportional to table size *and* a full-table RAM spike
under the lock. (`IndexManager::create_index`'s own doc at
`index_manager.rs:195-203` advertises O(batch) memory via streaming — that
property is defeated by the caller materializing first.)

For a first alpha this may be an acceptable trade, but it should be a
*deliberate* one and it should be documented in the DDL guide, not discovered in
production. The real fix is the two-phase build the original review asked for:
short exclusive window to snapshot a version + register `Building`, streaming
backfill with the lock released, short exclusive window to catch up the delta
and flip to `Ready`.

### N-8 (P1). F-55's fail-closed discovery turns a concurrent `DROP TABLE` into a spurious hard error

`build_reverse_fk_entries` (`fk_reverse_cache.rs:486-519`) snapshots
`repo.list_table_names()` and then `resolve(...)?` each name. A table dropped
between the snapshot and its `resolve` now aborts the whole discovery, so an
unrelated parent `UPDATE`/`DELETE` in that repo fails with a `NotFound` that has
nothing to do with the caller's request. The original review anticipated exactly
this ("разделить `not found because concurrent DROP` и реальную I/O ошибку") and
F-55 did not address it.

Fail-closed is the right default, so this is a P1, not a P0. The fix is a
single bounded retry: on `NotFound` for a name that is no longer in a *fresh*
`list_table_names()`, restart the scan once; any other error, or a `NotFound`
for a name still present, propagates. Same treatment for
`discover_on_update_refs` (`fk_on_update.rs:743-756`).

Secondary: the scan `resolve`s every table in the repo **sequentially**, one
`.await` each, and forces lazy instantiation of every dormant `TableManager`.
On a cold cache in a wide repo that is a latency spike on the first FK mutation
plus a memory step-change. `buffer_unordered` over the name list would be a
cheap win.

### N-9 (P1). Two of F-65's three new tests are weak or invalid oracles

`crates/shamir-engine/src/query/batch/tests/fk_indexed_action_read_error_tests.rs`

1. **`cascade_grandchild_recursion_propagates_read_error` (:396-481) is an
   invalid oracle.** It uses a *self-referential* table (`employees.manager_id →
   employees.id`) and then calls `arm_failure_for_all_rows(&resolver,
   "employees")` — which arms the injected failure for **every** row in the
   table, including the CEO row the batch is deleting. The primary delete's own
   `read_one_tx_bytes` can therefore fail before any FK planning runs, and the
   test's single `assert!(result.is_err())` passes without sites 2 or 3 ever
   being reached. The commit message confirms red-then-green was performed on
   site 1 only — so sites 2 and 3 (`fk_actions.rs:643-655` and `:788-800`) are
   effectively **unverified**.
2. **None of the three tests asserts the fast path was taken.** The oracle is
   only `result.is_err()`. There is no assertion on
   `stats.index_used` / `records_scanned`. This is the same defect the
   orchestrator caught during the session (the ON UPDATE test silently going
   full-scan) — it was fixed by adding a `create_index` call, but the *oracle*
   that would have caught it was never added. A future planner change that
   stops selecting the fast path will leave all three tests green.

Fix: arm the injector only on the *child* rows (use a separate child table for
the recursion test, or arm by explicit `RecordId` rather than "all rows"), and
add a control run in each test that asserts `index_used` is `Some(..)` /
`records_scanned == 0` before arming the failure.

### N-10 (P1). F-66 changed `append_ri_barrier_deps` from deterministic to bucket order

`crates/shamir-tx/src/tx_context.rs:683-690`. The old `TFxSet` (IndexSet) iterated
in **insertion order**; `scc::HashSet::iter_sync` iterates in bucket order,
which depends on hash values and on the insert history of the map. The resulting
`Vec<PredicateDep>` order feeds `predicate_conflicts_batch`, so which conflict
is reported first (and therefore which abort message a client sees) is now
nondeterministic across runs. Almost certainly not a correctness bug, but it is
an observable behaviour change that the commit claims did not happen
("identical public API and observable behavior"). If any test or client asserts
on the first conflict, it will flake. Either sort the tokens before pushing, or
document the order as unspecified.

Also worth confirming: the doc comment at `:282-284` asserts `is_empty()`
"resolves via a bucket-array occupancy check (`scc::HashMap::has_entry`), not a
full traversal". `clippy.toml` bans `scc::HashSet::len` as O(N) but says nothing
about `is_empty`; on scc 3.8.4 this is believed correct, but since the whole
justification for the swap rests on it, it deserves a one-line source citation
in the comment the same way `clippy.toml` cites `hash_map.rs:1400` for `len`.

### N-11 (P1). Stale/contradictory comments left behind by the wave (prior review's P1-7, still open, with new instances)

The original review's P1-7 was deliberately deferred. It is now worse, not
better, and two of the instances are actively misleading to a reader trying to
verify the concurrency protocol:

- `crates/shamir-db/src/shamir_db/execute/admin_schema.rs:126` — still says
  "F-50 will wire the SAME call into `create_index_v2`". F-56 did that. A reader
  checking whether `create_index_v2` drains will conclude it does not.
- `admin_schema.rs:154` and `:161-166` — `raise` is documented as storing the
  flag `Release`; `set_schema_activation_barrier` has stored `SeqCst` since
  F-56, and the whole proof depends on it.
- `crates/shamir-engine/Cargo.toml:113-119` — the `loom` dev-dependency comment
  says it is "Used ONLY under `RUSTFLAGS=\"--cfg loom\"`", which is precisely the
  approach `build.rs` was written to *avoid*.
- `read_asof_seek.rs:91` and `:190` — comments still reference
  `last_mutation_version()` with no argument after F-67 made it take one.
- `table_manager_index_mgmt.rs:60-72` — the "PARTIAL FIX, honestly scoped ...
  this barrier does NOT reach the tx-commit path" block predates F-48b, which
  wired `pre_commit_prelock` into exactly that barrier.

---

## 5. New findings — P2

- **N-12. loom wiring hygiene.** `crates/shamir-engine/build.rs:13-15` emits
  `cargo::rustc-cfg=loom` but never emits
  `println!("cargo::rustc-check-cfg=cfg(loom)")`. With modern Cargo/rustc this
  is the shape that produces `unexpected_cfg_condition_name` warnings on
  `#[cfg(loom)]` in every *non*-loom build — which `-D warnings` turns into a
  failure. Worth adding regardless of whether it currently fires. Separately:
  `loom = "0.7"` is an unconditional `[dev-dependencies]` entry, so it is
  compiled for every `./scripts/test.sh` run of `shamir-engine` whether or not
  the feature is on; and the model is not run by any CI workflow, so its
  regression-guard value is currently zero.
- **N-13. `MirroredStore::transact` residuals.** `storage_mirrored.rs:584-608`:
  steps 2 and 3 apply to `primary` with `?`, resting entirely on a comment
  asserting `InMemoryStore`'s writes are "structurally infallible". The field is
  typed as a trait object at the call site; nothing enforces the assumption. If
  either loop ever returns `Err`, the mirror is already committed and the caller
  gets an `Err` for a batch that *is* durable — the exact inverse of the bug
  F-59 fixed. A `debug_assert!`/`expect` with the invariant named, or narrowing
  the field's type, would make the assumption load-bearing in the type system
  rather than in prose. Also `durable_ops.clone()` (`:582`) is a full vec clone
  on every mixed batch; cheap because `Bytes` is refcounted, but avoidable.
- **N-14. `release.yml` now hard-blocks on a job that cannot run.** F-62 added
  `perf-gate` to every downstream `needs:` (`release.yml:500`, `:614`) targeting
  `runs-on: [self-hosted, shamir-bench]`, a label with no registered machine —
  and `bench-baseline.json` still does not exist in the tree, so even with a
  runner `scripts/bench_gate.sh:339-340` exits 1. Any `v*` tag push therefore
  hangs the entire release pipeline forever. This is intentional per the commit
  message; flagging only because it means "the release workflow is currently
  structurally unable to succeed" is a *code* fact, not just a process one.
- **N-15. `CREATE INDEX ... IF NOT EXISTS` is broken for sorted and index2
  families.** `admin_table_index.rs:339-343` computes `already_exists` from
  `unique_index_exists` / `index_exists` only — the two *hash* families. For
  `op.sorted` or `index_type != "btree"` it is always `false`, so `if_not_exists`
  is a no-op and a duplicate create silently proceeds.
  `SortedIndexManager::register` (`sorted_index_manager.rs:314-331`) is
  documented last-write-wins, so re-creating a sorted index under the same name
  with a *different* field path replaces the definition while leaving the old
  postings in the store. The `"exists"` error code is likewise unreachable for
  those families.
- **N-16. `IndexWriteOp` identity is recovered by byte-sniffing.**
  `decode_sorted_index_name` (`sorted_index_manager.rs:1620-1645`) recovers
  `name_interned` by pattern-matching the first byte against `SORTED_TAG` and
  reading 8 BE bytes. It is defensive and the false-positive direction is
  harmless, but the gate's correctness now depends on a *physical key layout*
  never colliding with another family's prefix, with no test pinning that. Adding
  an `index_id` field to `IndexWriteOp` (or a `SortedPosting` variant) would make
  the identity explicit; at minimum, a test asserting no other family's key can
  start with `SORTED_TAG` and be ≥ 9 bytes.
- **N-17. Per-index epoch entries are never reclaimed.**
  `SortedIndexManager::drop_index` sweeps postings but leaves the
  `last_mutation_version` entry. Bounded by "distinct index names ever created",
  so a slow leak only under repeated create/drop cycles — but note that *keeping*
  the entry is the safe direction (it is exactly what makes drop-then-recreate
  under the *same* name safe, while a different name is unsafe — see N-1).
  If N-1 is fixed by seeding, this becomes purely a leak.
- **N-18. Corrupt records are silently nulled on the AsOf seek path.**
  `read_asof_seek.rs:300-308` collects `corrupt: Vec<CorruptRecordRef>` and then
  drops it on the floor; `apply_select_value_bytes`
  (`read_exec.rs:2669-2675`) substitutes `QueryValue::Null`. So the same corrupt
  row is reported through the full-scan path and silently returned as `null`
  through the seek path — a result that differs by which plan the optimizer
  chose.
- **N-19. `read_one_tx_bytes` returning `Ok(None)` after an authoritative index
  lookup is still silent.** F-65 correctly split `Err` out, but
  `Ok(None) => continue` (`fk_actions.rs:452`, `:646`, `:791`,
  `fk_on_update.rs:462`) means "the index says this row exists, the table says it
  doesn't" is indistinguishable from a benign stale posting. That is the
  signature of index/table divergence and deserves at minimum a counter in
  `QueryStats` or a `tracing::warn`, so a real divergence is observable rather
  than being absorbed into a smaller cascade set.

---

## 6. What is left before a first tag (code, not process)

Ordered by what actually blocks. This deliberately excludes version/tag/CHANGELOG
work per the standing scope decision.

**Blocking (must land):**

1. **N-1** — seed the AsOf seek gate (restart floor + on-register + on-rename),
   or disable the seek arm for the tag.
2. **N-2** — single-word barrier state (or, minimally, reorder + `Acquire`).
3. **N-3** — never hold a drain guard across a lock acquisition; restructure
   `pre_commit_prelock`.

**Strongly recommended before the tag:**

4. **N-9** — repair F-65's oracles, so sites 2/3 are actually covered.
5. **N-6** — either extend the barrier to `DROP INDEX`/`RENAME INDEX`, or
   document those two as offline-only operations for alpha and reject them when
   the table has live writers.
6. **N-8** — one bounded retry so a concurrent `DROP TABLE` does not fail
   unrelated FK mutations.
7. **N-11** — the comment sweep, as its own `docs:` commit. The wave's whole
   review-ability rests on those comments being true; three of them now aren't.

**Deferred wave-4 items reassessed:**

- **P1-1 (coarse FK commit lock)** — unchanged in urgency. Still purely a
  throughput issue; still needs the benchmark set before touching. *Not more
  urgent now.*
- **P1-3 (index2 DDL error/cancellation semantics)** — the roadmap said this
  would be "absorbed by F-57". It was **not**: F-57 added barriers, not
  lifecycle states. `Building → error → recovery-completes-it-anyway` is still
  the behaviour, and F-57's sorted-index doc explicitly re-states the
  cancellation residual (`table_manager_sorted_index.rs:31-33`). **This is now
  more urgent than the roadmap assumed** — it should be re-ticketed rather than
  treated as closed.
- **P1-5 (top-K still O(N))** — unchanged, and now *blocked differently*: with
  N-1 open, the index-driven `ORDER BY … LIMIT` path can't be trusted as the
  answer either. Fix N-1 first.
- **P1-7 (comment hygiene)** — see N-11; escalated from cosmetic to
  correctness-documentation.

---

## 7. DDL — what looks underdeveloped

Reading the current DDL surface rather than restating the prior review:

1. **There is no index lifecycle state for the legacy families.** `index2` has
   `Building`/`Ready`; regular/unique/sorted have nothing. F-57 gave all four a
   *barrier* but only index2 has a *state*. Until a `DESCRIBE INDEX` can answer
   "is this index trustworthy right now", the planner has no way to refuse a
   half-built index and the doctor's `repair()` is the only recovery. This is
   the single highest-value DDL investment.
2. **`DROP INDEX` has no lifecycle at all** (N-6). A `Dropping` state that the
   planner refuses to plan against would make the sweep safe without a lock.
3. **`if_not_exists` / `"exists"` is family-blind** (N-15). The existence check
   should consult all four registries through one helper, not two of them.
4. **Error codes.** `handle_create_index` builds most errors with `code: None`
   (`admin_table_index.rs:301-305`, and every `err(...)` call at `:378`, `:387`,
   `:390`, `:395`, `:401`, `:406`, `:411`). A client cannot distinguish
   "sorted+unique is illegal" from "composite sorted index unsupported" from "the
   backfill hit a storage error" — they are all an untyped string. The
   `err_code` helper exists right next to it and is used for exactly two cases.
5. **Composite sorted indexes are rejected with a `TBD` string**
   (`admin_table_index.rs:393-397`) while `CreateIndexOp.fields` is already a
   `Vec<Vec<String>>` and `SortedIndexDefinition` already carries a
   `field_path: Vec<u64>`. The shape is there; only the encoder and the keyset
   bookmark are missing. This is the most obviously "half-built" DDL feature in
   the tree.
6. **`include` (covering) is validated only for `sorted`**
   (`admin_table_index.rs:389-391`) but nothing validates that the included
   paths are indexable, non-duplicated, or distinct from the key path.
7. `ALTER TABLE`, `VALIDATE CONSTRAINT`, partial indexes, generated columns —
   all still absent, and all still correctly deferred until (1)–(3) exist. The
   prior review's "don't add DDL breadth on top of an unreliable lifecycle" is
   more true after F-57, not less: F-57 proved that adding a fourth barrier flag
   costs a new deadlock edge (N-3), which is exactly the compounding the advice
   warns about.

---

## 8. OQL / read paths — what looks underdeveloped

1. **The AsOf + keyset combination is the least-defended read path in the
   engine.** Its safety rests on one `u64` comparison whose initial value is
   wrong (N-1), a `concurrent_modified` classifier that the module doc itself
   admits cannot see removals, and a post-scan re-check with a documented
   synchronous residual. Everything else about the seek is good engineering;
   the *gate* is a single point of failure and should become a first-class,
   persisted, structurally-seeded epoch rather than a lazily-created map entry.
2. **`try_plan_keyset_seek` is extremely narrow** (`read_planner.rs:477-497`):
   no `WHERE`, single order-by column, single-element seek key, no `GROUP BY`,
   no `DISTINCT`, no aggregates, no `count_total`. Any residual predicate at all
   forces the O(N) path. A residual-filter-after-seek (walk the index, apply the
   filter, keep walking until `limit` is filled) is a small change with a large
   reach, and it is the same shape the FK indexed-action fast path already uses.
3. **`ORDER BY` on a non-selected field** is still unsupported — the prior
   review flagged it and nothing in this wave touched it. It is the most common
   thing a user hits before any of the JOIN work matters.
4. **No `EXPLAIN ANALYZE`.** `QueryStats` already carries `index_used`,
   `records_scanned`, `records_returned`, `execution_time_us` — but there is no
   way to ask "why did the seek decline?". Given that the seek can decline for
   five distinct reasons (plan shape, gate, `concurrent_modified`, post-scan
   re-check, no MVCC store), a `fallback_reason` field in `QueryStats` would pay
   for itself immediately in debugging N-1-class problems, and would have made
   the F-65 test-oracle problem (N-9) impossible to miss.
5. **Corrupt-record reporting differs by plan** (N-18) — a semantics leak
   between plans that should be invisible.
6. JOIN/EXISTS/set-ops/window functions: unchanged from the prior review; all
   correctly behind (1)–(4).

---

## 9. Query builders

`shamir-query-builder` has **not** been extended by this wave at all, and the
engine moved underneath it. Concretely:

1. **`create_index.rs` is entirely stringly-typed and infallible.**
   `index_type`, `fts_tokenizer`, `fts_language`, `functional_op`,
   `vector_metric`, `vector_quantization` are all `impl Into<String>`
   (`ddl/create_index.rs:85-131`), and `build()` returns `BatchOp`, not
   `Result`. Every one of the server-side validations in
   `admin_table_index.rs:386-397` (`sorted && unique`, `include` without
   `sorted`, sorted with ≠1 field) is a *local, statically-decidable* property
   that the builder cheerfully constructs and ships to the server. The engine
   also now has four structurally different create paths behind that one
   builder; a typed `IndexKind` enum (`Hash { unique } | Sorted { include } |
   Fts {..} | Functional {..} | Vector {..}`) would make the illegal
   combinations unrepresentable and map 1:1 onto the four engine paths F-57
   just formalised.
2. **No cursor bookmark type.** `read_asof_seek.rs:313-328` deliberately emits
   `_id` on every row so the client can echo it as `after_id`, and
   `query.rs:223-229` accepts `after_with_id(key, limit, after_id)`. But the
   builder gives the caller no way to *derive* that triple from a previous
   page — the user hand-extracts the order value and the `_id` and reassembles
   them, which is precisely the raw-wire-assembly the repo's own "builder only"
   rule forbids everywhere else. A `Bookmark::from_page(&QueryResult,
   &OrderBy)` → `query.after_bookmark(bm)` would close it, and would be the
   natural place to also carry the query-shape hash so a bookmark from one
   query cannot be replayed against another.
3. **No `as_of` + cursor composition helper.** `create_cursor`
   (`cursor.rs:38-59`) takes a `ReadQuery`, which *can* carry
   `Temporal::AsOf` — but nothing in the builder or its docs indicates that
   AsOf + keyset is the combination that engages the seek fast path, or that it
   silently degrades. Given N-1, a builder-level note is the cheapest way to
   stop users depending on it.
4. **No FK-action surface at all.** F-53c/F-65 built an entire indexed
   CASCADE/SET NULL/ON UPDATE machine; the builder's only exposure is
   `FkAction` inside a schema/validator definition. There is no way to *observe*
   what a cascade did (affected child tables, rows touched, whether the index
   fast path was used) — the engine has the data, the wire has no field for it,
   and the builder has no accessor.
5. **No `describe_index` / `show_index_builds` / `explain_analyze`** — all three
   are gated on §7/§8 items, but the builder should be where they land first
   since they are pure request shapes.
6. **No parity gate.** There is still nothing asserting every `BatchOp` variant
   has both a Rust builder and a TS builder. `ddl/` has 26 files; `BatchOp` has
   more variants than that.

`shamir-query-builder-macros` is not implicated — the gaps above are all plain
builder API surface, and per the prior review's guidance macros should stay out
of lifecycle/DDL side effects.

---

## 10. Performance opportunities observed while reading

1. **Six atomics on the writer hot path, six cache lines** (N-2's fix is also
   this fix). `needs_write_barrier()` (`table_manager.rs:820-836`) now loads
   `has_indexes_unique` + five `AtomicBool`s, each behind its own `Arc`
   allocation — six independent cache lines touched on **every** non-tx write
   and once per table in every commit's prelock. Collapse to one `AtomicU32`
   bitmask: 1 load, 1 line, and the F-56 proof gets simpler.
2. **`create_index`/`create_unique_index` materialize the whole table under an
   exclusive lock** (N-7). `IndexManager` already streams internally; the
   caller should hand it the stream, not a `Vec`.
3. **FK cold-cache discovery is O(T) sequential awaits** and force-instantiates
   every table (N-8). `buffer_unordered(N)` over `list_table_names()`.
4. **Top-K still projects every match.** `read_exec.rs:1034-1069` calls the
   projection *before* `heap.push`, so a scan matching N rows pays N
   projections + N `QueryValue` allocations to keep K. Push `(sort_key,
   RecordId, Bytes)` and project only the surviving K in
   `into_sorted()`. This is the single largest read-path win visible in the
   file, and it is independent of the (harder) index-driven early-stop work.
5. **`MirroredStore::transact` clones the durable op vec** (`:582`) — cheap
   (`Bytes` refcount) but unnecessary; the mirror could take a slice-borrow API
   or the primary application could be driven from the mirror's own accepted
   batch.
6. **`bump_touched_indexes` is O(ops × distinct_indexes)** via
   `SmallVec::contains` (`sorted_index_manager.rs:665-680`). Correct choice for
   ≤10 indexes and explicitly justified — but it now runs on **every** commit's
   Phase 5c for **every** index family's ops, including tables with zero sorted
   indexes, where it pays a full pass over the op vec plus a `decode` per op to
   learn there is nothing to do. An early `if self.indexes.load_local().is_empty()
   { return; }` guard makes the common case free.
7. **`on_records_created_batch` builds `writes: Vec<(RecordKey, Bytes)>` with no
   capacity hint** (`sorted_index_manager.rs:730-745`) while both `defs.len()`
   and the item count are known.
8. **`WriterDrainBarrier::drain` busy-spins on `yield_now`**
   (`writer_drain_barrier.rs:180-182`). Fine when the expected wait is ~µs, but
   with F-57 the drain now runs on four DDL paths and the guards it waits for
   are held across an entire tx materialize (`pre_commit.rs:448-451`). A
   `Notify` woken by the last `fetch_sub` would be both cheaper and — more
   importantly — would give a place to hang a timeout, which is what turns N-3
   from a permanent hang into a detectable error.

---

## 11. Limits of this review

- Nothing was compiled, run, benchmarked or fuzzed. Every scenario above is
  derived from code reading; N-2 and N-3 in particular are argued from control
  flow and memory ordering, not observed.
- The `scc` 3.8.4 cost claims in F-66's doc comment (`is_empty` via
  `has_entry`) were not verified against the crate source.
- CI state (registered runners, branch protection, actual workflow outcomes) is
  not visible from here.
- Findings are scoped strictly to `28d39f31`.
