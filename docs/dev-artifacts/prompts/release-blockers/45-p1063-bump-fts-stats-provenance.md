# Brief 45 — #1063: BumpFtsStats without provenance corrupts BM25 with 2+ FTS indexes

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Context

`IndexWriteOp::BumpFtsStats { doc_len, sign }` (`crates/shamir-tx/src/index_write_op.rs:111`)
carries no `Provenance`, unlike `SetPosting`/`RemovePosting`. `set_provenance`
for it is a no-op (`index_write_op.rs:136`). At tx-commit,
`retract_stale_provenance_ops` (`crates/shamir-engine/src/tx/pre_commit.rs:1309`)
always keeps `BumpFtsStats` — `IndexWriteOp::BumpFtsStats { .. } => return true, // No provenance to check`.
`apply_index_ops_at_commit` (`crates/shamir-index/src/write_ops.rs:161-165`)
broadcasts every in-memory op to **all** of the table's index2 backends. Each
`FtsRankedBackend::plan_insert/update/delete` (`crates/shamir-index/src/fts_ranked_backend.rs:169,208-215,235`)
emits its own bump, and `apply_in_memory` (`:262-273`) applies every bump it
receives with no ownership check.

**Failure with 2 FTS indexes on the same table, one insert**: each backend
plans its own `BumpFtsStats(+1)` → tx-commit collects both → both are
broadcast to both backends → each backend's `doc_count` gets `+2` instead of
`+1`. With N FTS indexes: N² applications instead of N. **Worse than a
counter bug**: two FTS indexes on *different fields* have different
`doc_len`, so `sum_doc_len`/`avgdl` gets polluted with the wrong field's
length — this corrupts BM25 ranking structurally, not just doc_count scale.

**ABA variant**: a tx stages a write for FTS instance A; concurrently the
index is dropped and recreated with the same name as instance B; stale
posting ops for A are correctly dropped by the provenance filter, but the
stale bump for A survives and gets applied to B.

**Scope check before touching anything**: `apply_index_ops` (non-tx path,
`write_ops.rs` ~:95-104) takes a single `backend: &Arc<dyn IndexBackend>` and
applies ops to it alone — that path is already correct. Do not touch it.

## Task

1. Give `BumpFtsStats` the same `Provenance` that `SetPosting`/`RemovePosting`
   carry (or a stable backend identity — `index_id` + `instance_epoch`). Name
   alone is insufficient — DROP+CREATE with the same name needs the epoch to
   distinguish instances.
2. At commit, group in-memory ops by owning backend instead of broadcasting
   to all backends in `apply_index_ops_at_commit`.
3. In `retract_stale_provenance_ops`, retract stale `BumpFtsStats` the same
   way stale posting ops are retracted (currently it's an unconditional
   `return true`).

## Tests (minimum, must fail on current HEAD before your fix)

1. Two FTS indexes on **different fields**, one insert: assert `doc_count == 1`
   **and** `sum_doc_len`/`avgdl` correct on each backend — not just the
   counter, the actual BM25 aggregate.
2. Two FTS indexes, update one field: stats change only on the owning
   backend.
3. Stage tx → DROP FTS → CREATE FTS with the same name → commit: new backend
   does NOT receive the stale bump.
4. Same scenarios for delete and abort.
5. End-to-end: BM25 `$score` against an independent reference calculation,
   not just the stats counters.

Each test must be discriminating — verify it actually fails against the
current (broken) code before your fix, not just that it passes after.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh @storage
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine -- fts
```

Report the exact diff and exact test output. Do not summarize as "done" —
show what changed and what the tests actually assert.
