# Brief for F-50 Step 2 (#870, P0, implement) — extend the index2 ops-plan re-derivation to all backend/mutation kinds + sorted-index

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-50 Step 1 (#869, commit `07076dde`) settled the design and landed a
working prototype for ONE case (functional index2 backend, single-row
INSERT) that closes #538 Part B's "guaranteed miss": a tx that stages an
insert before a new index2 backend is registered (via `create_index_v2`)
and commits after would previously carry zero ops for that backend in
`tx.index_write_set` forever.

**Read `docs/dev-artifacts/research/f50-index-lifecycle-spike.md` in full
first** — it is the settled design memo this step implements against. Its
§5.1 lists Step 2's scope; this brief refines and grounds that list against
the actual current code (a few names/line numbers in the memo were
approximate — this brief corrects them).

**The mechanism, already working (do not re-derive, build on it):**
- `IndexRegistry` (`crates/shamir-index/src/registry.rs`) has a
  `generation: AtomicU64` bumped on every `insert`/`remove_by_id`, plus a
  per-backend insertion-generation tag (`by_id: scc::HashMap<u32, (Arc<dyn
  IndexBackend>, u64)>`). `backends_newer_than(threshold_gen)` returns every
  backend inserted after `threshold_gen`.
- `TxContext` (`crates/shamir-tx/src/tx_context.rs`) has
  `index2_stage_gens: TFxMap<u64, u64>` (table_token → generation captured
  at stage time) and `note_index2_stage_gen(table_token, gen)` to capture it
  (idempotent per-tx via `or_insert` — earliest capture wins).
- `pre_commit.rs`'s new Phase 2.7 (`rederive_index2_ops_post_stage`, called
  from `pre_commit_prelock` right after the F-48b writer-drain wiring,
  before the F-48b test seam) reads each touched table's live generation; if
  unchanged from the tx's captured value, it `continue`s (zero cost); if
  advanced, it diffs the new backends, resolves insert-vs-update-vs-delete
  per staged record via one `data_store.get(key)` (Phase 5a hasn't run yet,
  so the store still holds the pre-tx value), and appends the re-derived ops
  to `tx.index_write_set` — **before** `wal_ops_from_tx` (Phase 4) serializes
  that set into the WAL entry. This ordering is load-bearing for crash
  safety (see the memo §2.4 / the call-site comment in `pre_commit.rs` for
  why) — do not change it.

## Corrections to the memo's §5.1 list (verified against current code)

- **`set_tx` (`table_manager_tx_ops.rs:1049`) is NOT a separate staging
  path** — it is a one-line alias that calls `update_tx`. Fixing `update_tx`
  covers `set_tx` automatically; do NOT add a redundant capture call there.
- **`update_tx_bytes` (`:860`) is real and separate** (the byte-level /
  `RecordView`-lens W3 path) — needs its own capture, as does `update_tx`
  (`:763`) and `delete_tx` (`:977`). The memo's "`update_tx_bytes` /
  `update_tx_ref`" phrasing was imprecise — there is no `update_tx_ref`;
  just `update_tx` and `update_tx_bytes`.
- **Drop the "planner Ready-gating scaffold" item (memo §5.1 item E)
  entirely from this step's scope.** The memo's own §1.2 analysis already
  concludes `create_index_v2` backfills-then-registers, so there is no
  `Building` window today — a backend is effectively Ready the instant it's
  in the registry. Adding a non-persisted `Ready` check now would be
  speculative scaffolding for a state that doesn't exist yet (the project's
  own discipline: don't design for hypothetical future requirements). Leave
  this entirely to Step 3, where the persisted `IndexState` actually
  becomes load-bearing.

## What to implement

### A. Capture the stage generation on the remaining mutation paths (3 sites, `table_manager_tx_ops.rs`)

Mirror the INSERT paths' existing capture (see `insert_tx`,
`insert_tx_many`, `insert_tx_many_bytes` for the pattern: one line, placed
AFTER that path's stage-time `all_backends()`-consuming planner call, i.e.
after `plan_update_ops`/`plan_update_ops_ref`/`plan_delete_ops` has already
run for that record):

1. **`update_tx`** (`:763-848`) — capture after the `plan_update_ops` /
   `plan_insert_ops` branch (the `match &old { ... }` at `:811-817`), before
   `stage_mutation`.
2. **`update_tx_bytes`** (`:860-960`) — capture after `plan_update_ops_ref`
   (the `RecordView` lens branch, `:893-895`) AND after the non-map fallback
   branch's `plan_update_ops` (`:936-938`) — both branches need it since
   they're alternative paths to the same staging call, not both executing.
3. **`delete_tx`** (`:977-1045`) — capture after `plan_delete_ops` (either
   branch, `:1014` or `:1027`), before `stage_mutation`.

Each capture: `tx.note_index2_stage_gen(self.table_token(),
self.index2_registry().generation())`.

### B. Vector backend `staged_vectors` re-derivation (`pre_commit.rs`'s `rederive_index2_ops_post_stage`)

`VectorBackend::plan_insert_tx`/`plan_update_tx`/`plan_delete_tx` are no-ops
on the live graph for a tx — HNSW embeddings route through
`tx.staged_vectors`/`tx.staged_vector_deletes` instead (see
`table_manager_tx_ops.rs`'s `stage_vectors` (`:77-89`), `stage_vector_delete`
(`:102-116`), `stage_vector_deletes_on_update` (`:127-145`) for the existing
stage-time pattern — each iterates `self.index2_registry.all_backends()`
and calls `backend.staged_vector(rid, rec)`, staging via
`tx.stage_vector`/`tx.stage_vector_delete`).

Add a parallel branch in `rederive_index2_ops_post_stage`'s per-record loop:
for each backend in `new_backends` whose `descriptor().kind` matches
`IndexKind::Vector(_)` (see `crates/shamir-index/src/kind.rs`), mirror the
insert/update/delete resolution already computed (you already know
insert-vs-update-vs-delete from the `data_store.get` result used for the
posting-ops branch) by calling the vector backend's `staged_vector`/
`stage_vector`/`stage_vector_delete` equivalent:
- Insert (`NotFound` case): `if let Some(v) = backend.staged_vector(rid,
  &new_rec).await { tx.stage_vector(table_token, rid, v); }`
- Update (`Some(old)` case): mirror `stage_vector_deletes_on_update`'s logic
  (old carried a vector, new doesn't → stage delete; otherwise if new
  carries one → stage insert/replace) PLUS the plain insert-side staging
  for the new value.
- Delete (`KvOp::Remove` case): mirror `stage_vector_delete` — if the OLD
  record carried a vector at this backend's field path, stage a delete.

Write a test in `f50_index_lifecycle_spike_tests.rs` (or a new
`f50_step2_*_tests.rs` file if you prefer — your call, but keep the
existing file's 2 tests intact) proving: stage an insert before a vector
index2 backend is registered, complete `create_index_v2` for a vector
index, commit, then assert the row's embedding IS present in the HNSW graph
(query via the backend's `lookup(IndexQuery::Vector{...})` or equivalent,
mirroring the existing `functional_lookup` helper's pattern for a vector
backend).

### C. FTS `BumpFtsStats` double-count guard test

`IndexWriteOp::BumpFtsStats` is in-memory-only (not a posting;
`commit.rs`'s `wal_ops_from_tx` skips it — confirm this by reading that
function). Add a test confirming the generation filter does NOT double-plan
an FTS backend's stats bump for a tx that already planned against it at
stage time (i.e. the quiescent case for FTS, analogous to the existing
`quiescent_tx_unchanged_generation_indexes_via_stage_plan` test but for an
FTS backend, asserting exactly one `BumpFtsStats`/one doc-count increment,
not two). This is a regression guard, not a new fix — the mechanism should
already be correct (the generation filter's `insertion_gen > stage_gen`
naturally excludes already-planned backends); the test just needs to exist.

### D. Sorted-index residual (`table_manager_sorted_index.rs`)

Read the file's doc comment (~line 7-16, `cancel-safe: NO`) and
`create_sorted_index`/`create_sorted_index_with_include` in full. Also read
`table_manager_tx_ops.rs`'s `plan_legacy_insert_ops`/
`plan_legacy_update_ops`/`plan_legacy_delete_ops` (confirmed in Step 1's
memo to call `self.sorted_indexes.plan_record_created`/etc. at STAGE time
— the SAME staleness mechanism as index2).

Investigate first: does `SortedIndexManager` have (or trivially support) a
generation counter equivalent to `IndexRegistry`'s? If yes, the same
pattern applies (capture generation at stage, re-derive at Phase 2.7 for
new sorted indexes). If adding that abstraction is disproportionate to the
bug, evaluate the memo's alternative — reordering `create_sorted_index` to
register AFTER backfill completes (closing the register-before-ready
window directly, the same shape as index2's Part A fix from #538). Pick
whichever is simpler and actually closes the gap; document your choice and
reasoning in your final summary. Write a red-then-green test mirroring
`f50_index_lifecycle_spike_tests.rs`'s pattern for whichever fix you land.

## What NOT to do (Step 3's scope, per the settled memo)

- Do NOT add a persisted `Building`/`Ready` `IndexState` field to
  `IndexDescriptor` or wire it through `save_index2_metadata`/recovery.
- Do NOT touch crash/restart continuation for an interrupted
  `create_index_v2`.
- Do NOT implement DDL cancellation (`DROP INDEX` mid-build).
- Do NOT add the planner Ready-gating scaffold (see "Corrections" above —
  explicitly dropped from this step).

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-index -p shamir-tx -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- Do not touch the F-48/F-48b writer-drain wiring, the F-46 RI barrier, the
  F-47 FK cache, or the F-49 `MirroredStore::transact` ordering — all
  already-landed and unrelated to this task.
- Keep the existing `f50_index_lifecycle_spike_tests.rs` file's 2 tests
  passing unchanged (they prove Step 1's prototype still works) — add new
  tests alongside/after them rather than modifying their assertions.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-index -p shamir-tx -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-index -p shamir-tx --full
```

When done, give your final summary as plain text: the 3 capture-site
diffs (A), the vector re-derivation branch design and its test's red→green
proof with actual test output (B), the FTS double-count guard test and its
output (C), your sorted-index investigation findings and which fix you
chose + its red→green proof (D), and confirmation fmt/clippy/tests are
clean.
