# Brief 46 — #1063 round 2: add the discriminating multi-FTS-index tests

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Context

The production fix for #1063 (BumpFtsStats provenance) is ALREADY DONE and
already in the working tree — do not touch `crates/shamir-tx/src/index_write_op.rs`,
`crates/shamir-index/src/fts_ranked_backend.rs`'s `plan_insert`/`plan_update`/
`plan_delete`/`apply_in_memory`, `crates/shamir-index/src/write_ops.rs`'s
`apply_index_ops_at_commit`, or `crates/shamir-engine/src/tx/pre_commit.rs`'s
`retract_stale_provenance_ops` — those are correct and verified.

**What's missing**: the round-1 attempt tried to write an integration test,
hit compilation errors (wrong `InnerValue` variant, incorrect API usage), and
DELETED the test file instead of fixing it — reporting "existing unit tests
cover this" as a residual-risk justification. They do not: the existing 606
unit tests in `shamir-index` only exercise the production code's *shape*
(single-backend paths, or a placeholder `provenance` value bolted onto an
existing single-index test to make it compile). None of them create TWO live
FTS backends and check cross-contamination — which is the entire point of
this bug.

This round's ONLY job: add that missing test coverage. Do not touch
production code unless a test reveals a genuine remaining defect.

## Exact template to copy from — do not improvise the API

`crates/shamir-engine/src/table/tests/f50_step2_index_lifecycle_tests.rs`,
**Part C** (`:203-300`, function `fts_quiescent_tx_exactly_one_bumpftsstats_no_double_count`)
is the exact pattern to extend — same file, same table, same tx-staging
style, already proven to compile and pass. Read it in full before writing
anything. Key pieces it already gives you:

- `fts_index_op(name, table, field)` (`:207-226`) — the `CreateIndexOp`
  shape for an FTS index (`index_type: Some("fts".into())`).
- `tbl.create_index_v2(&fts_index_op(...)).await.unwrap()` — how to create
  an FTS index on a table.
- `tbl.index2_registry().get_by_name(key_id(&tbl, "title_fts").await).await`
  → downcast via `.as_any().downcast_ref::<crate::index2::fts_ranked_backend::FtsRankedBackend>()`
  to read backend-internal state.
- `fts.doc_count()` — existing public accessor.
- The insert pattern: `let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();`
  → `write::insert("docs").rows([mpack!({ "field": "value" })]).build()`
  → `tbl.execute_insert_tx(&op, &mut tx, true, None, &shamir_types::access::Actor::System).await.unwrap()`
  → `repo.commit_tx(tx).await.expect(...)`.
- `key_id(&tbl, "title").await` and `record_with_str` / `mpack!` helpers already
  defined at the top of the file — reuse them, do not redefine.

## Step 0 — add a `sum_doc_len()` accessor (small, required first)

`FtsRankedBackend::stats` is `pub(crate)` (`crates/shamir-index/src/fts_ranked_backend.rs:31`)
and `FtsStats::sum_doc_len` is a public `AtomicU64` field
(`crates/shamir-index/src/bm25.rs:55`) — but `pub(crate)` on `stats` means an
engine-crate test cannot read it directly (only `shamir-index`-internal code
can). Add, right next to the existing `pub fn doc_count(&self) -> u64`
(`fts_ranked_backend.rs:55`):

```rust
pub fn sum_doc_len(&self) -> u64 {
    self.stats.sum_doc_len.load(std::sync::atomic::Ordering::Relaxed)
}
```

This mirrors `doc_count()` exactly — same visibility, same access pattern.
No other production change.

## Step 1 — write the tests, in a NEW file

Create `crates/shamir-engine/src/table/tests/p1063_multi_fts_index_stats_tests.rs`
(new file — do not extend `f50_step2_index_lifecycle_tests.rs`, this is a
distinct topic per this repo's "one file per topic" test convention). Wire it
into `crates/shamir-engine/src/table/tests/mod.rs` (`pub mod p1063_multi_fts_index_stats_tests;`).

Required tests (each MUST fail if you temporarily revert the production fix —
verify this yourself before reporting done, by checking out the pre-fix
versions of the 4 production files listed above in a scratch copy or by
mentally tracing the code path, and confirming the assertion would not hold):

1. **`two_fts_indexes_different_fields_one_insert_doc_count_and_sum_doc_len_correct`**
   — create TWO FTS indexes on the same table on DIFFERENT fields (e.g.
   `title_fts` on `title`, `body_fts` on `body`), both created BEFORE any tx
   stages (quiescent case, like Part C). Insert one row with both fields
   populated with DIFFERENT-length text (so `doc_len` differs per field —
   this is what makes the bug's `sum_doc_len` pollution scenario real, not
   just a `doc_count` scale bug). Commit. Assert on EACH backend
   independently: `doc_count() == 1` (not 2), AND `sum_doc_len()` equals
   THAT backend's own field's token count (not the other field's, not the
   sum of both). Compute the expected token count using the same tokenizer
   the backend uses (or assert the two backends' `sum_doc_len()` values are
   DIFFERENT from each other when the two fields have different lengths —
   that alone proves no cross-contamination without needing to hardcode a
   tokenizer's exact output).

2. **`two_fts_indexes_update_one_field_only_owner_stats_change`** — same two
   backends, insert then update ONLY the `title` field. Assert `title_fts`'s
   `doc_count`/`sum_doc_len` changed appropriately (net zero for doc_count on
   an update, `sum_doc_len` reflects new length) while `body_fts`'s stats are
   COMPLETELY UNCHANGED (no bump touched it at all).

3. **`stale_bump_from_dropped_and_recreated_fts_index_not_applied_to_new_instance`**
   (the ABA case) — a tx STAGES an insert while `title_fts` (instance A) is
   live. Before that tx commits, DROP `title_fts` and CREATE a NEW FTS index
   with the SAME name `title_fts` (instance B — this mints a fresh
   `instance_epoch`, see `IndexRegistry`'s `BackendEntry.gen`). Commit the
   original tx. Assert instance B's `doc_count()`/`sum_doc_len()` are BOTH
   still 0 (or whatever they were from B's own backfill, NOT incremented by
   the stale tx's bump) — the stale `BumpFtsStats` for instance A must have
   been retracted, not applied to B.
   - Use the pause-hook-free sequential pattern (no concurrency needed —
     this is a straight-line stage → DDL → commit sequence, same determinism
     style as the rest of this file per its module doc `:10-11`).
   - If you cannot find a way to DROP an index2/FTS index in this test
     harness within reasonable effort (check `tbl.drop_index2(name, op_id)` —
     see other tests in `crates/shamir-engine/src/table/tests/` for the
     call shape, e.g. `p1048_index2_drop_durability_tests.rs`), it is
     acceptable to skip ONLY this specific test and say so explicitly in
     your report — do not delete or silently omit it without flagging it.

4. **Delete/abort variants of test 1** — same two-backend setup, delete the
   row instead of inserting (assert `sign: -1` bump lands on the right
   backend only), and a staged-then-ABORTED tx (assert NEITHER backend's
   stats changed at all — nothing should apply on abort).

Do NOT attempt an end-to-end `$score` BM25-ranking test against an
independent reference calculation (the original brief's stretch goal) — the
`doc_count`/`sum_doc_len` assertions above already discriminate the bug
directly and are cheaper to get right. Skip it.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- p1063
./scripts/test.sh -p shamir-index
```

Report the exact diff, the exact test names that exist in the new file, and
paste the actual `nextest` output for the `p1063` filter (not a paraphrase).
If any of the 4 required tests could not be written, say exactly which one,
why, and what you tried — do not silently drop a test and call the task
complete.
