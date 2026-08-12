# #1100 round 2 — `read_pre_tx_bytes` reads a stale/lagging physical `data_store`, not the current MVCC-visible value

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background — round 1's honest self-report, and what the orchestrator found investigating it

Round 1 (this same session, `p1100-stale-delete`) implemented
`rederive_stale_value_ops_post_stage` in `pre_commit.rs`, correctly chose
the commit-time re-derivation shape (option A), and honestly reported in
its final summary that 2 of 4 new tests still FAIL despite the logic
"looking correct" — `stale_snapshot_delete_leaves_dangling_posting` and
`stale_snapshot_update_leaves_dangling_posting`. Good instinct to report
that instead of claiming false success — the orchestrator has now found
the actual root cause by instrumenting the code with `eprintln!` probes
(temporary, already reverted — round 1's `pre_commit.rs` diff is
unchanged) and running the failing test directly.

**Root cause**: `read_pre_tx_bytes` (~line 986, the function round 1
correctly identified as the established "read current durable state"
primitive from `rederive_base_index_ops_post_stage`) reads via
`data_store.get(key)` — the RAW physical KV store. In this codebase's
lock-free commit path (`commit.rs::commit_tx`), the actual physical
`data_store`/`history` write is **NOT** part of the inline, awaited commit
sequence. Read `commit.rs` lines ~690-790 yourself: `apply_data_phase`
(inline, awaited) publishes the record to the **MVCC-visible overlay**
(`mvcc_store` — what `version_guard.commit()` makes visible to other
readers), but the actual `materialize_async_tail(...)` call is inside a
**`tokio::spawn`** at line ~771 — a detached background task `commit_tx`
does **NOT** await before returning to its caller. A comment right there
even says it plainly: "the value becomes durable in `history` only after
the background drainer replays the WAL entry."

So by the time tx2's `pre_commit_prelock` runs (immediately after tx1's
`commit_tx().await` returns), tx1's record is **already visible** via the
MVCC overlay (confirmed: the reproduction test's own assertions —
`lookup_by_unique_index` returning `Some(rid_r)` for "z" — pass BEFORE the
bug manifests) but **may not yet be reflected in the raw `data_store`**
the spawned tail task hasn't necessarily run yet when tx2's rederive
executes. Empirically confirmed via a debug probe: `read_pre_tx_bytes`
for tx2's DELETE of `rid_r` returned `None` (`NotFound`) even though
`rid_r` demonstrably, durably owns "z" per the test's own prior
assertions. That's why `rederive_stale_value_ops_post_stage` computes
`appended: Vec<...>` as empty — its `if let Some(current_bytes) = ...`
branch never executes, so `plan_record_deleted`/`plan_record_deleted_unique`
are never even called for the CURRENT value, and no removal op is ever
generated for "z".

This means `read_pre_tx_bytes`/`data_store.get()` is **not a reliable
"give me the record's current durable value" primitive** in this
lock-free commit path — it can lag behind an already-visible, already-MVCC
-published commit by an unbounded amount (until the spawned
`materialize_async_tail` task actually runs).

## The fix — use the SAME "current visible value" read primitive the rest of the codebase uses

`TableManager::read_one_tx_bytes` (`table_manager_streaming.rs` ~line
634) already has the correct semantics for exactly this need. Read it in
full. When called with `tx: None` (no tx context) it does:

```rust
// No tx, or no mvcc: read raw bytes from the data store.
if let Some(mvcc) = self.mvcc_store.as_ref() {
    return mvcc.get_current_bytes(id.as_bytes()).await;
}
match self.table.data_store().get(RecordKey::from_slice(id.as_bytes())).await {
    ...
}
```

— i.e. when an `mvcc_store` exists (true for any MVCC-routed table, which
is the case that matters here), it reads through
`mvcc.get_current_bytes(...)`, the MVCC-visible overlay — the SAME layer
`version_guard.commit()` publishes to inline, NOT the raw physical store.
This is the primitive `rederive_stale_value_ops_post_stage` should have
used instead of `read_pre_tx_bytes`/`data_store.get()`.

**What to change**: in `rederive_stale_value_ops_post_stage`
(`pre_commit.rs`), replace both calls to
`read_pre_tx_bytes(&data_store, table_token, rid, &k)` (one in the
`KvOp::Remove` / DELETE branch, one in the `KvOp::Set` / UPDATE branch)
with a call that goes through the MVCC-visible-current-value path instead
of the raw `data_store`. Two ways to get there — pick whichever fits the
existing code shape best, and explain your choice:

1. Call `tbl.read_one_tx_bytes(rid, None)` directly (it's already
   `pub(crate)`, and `rederive_stale_value_ops_post_stage` already holds
   `tbl: TableManager` via `repo.table_by_token_if_live(table_token)`).
   This reuses the EXACT same method `delete_tx`/`update_tx` themselves
   call for their own snapshot-bound reads (just with `None` instead of
   `Some(tx)`), so its semantics are already proven-correct and tested
   elsewhere in the crate.
2. Or, if `tbl.mvcc_store` is reachable from `pre_commit.rs` without
   going through `TableManager`'s method (check the actual visibility —
   it may be private to `table_manager_streaming.rs`), call
   `mvcc.get_current_bytes(key.as_ref())` directly, keeping the same
   overall function shape `read_pre_tx_bytes` has today (so
   `rederive_base_index_ops_post_stage`'s existing, working use of
   `read_pre_tx_bytes` is untouched — this fix is scoped to
   `rederive_stale_value_ops_post_stage` only, do NOT change
   `read_pre_tx_bytes` itself or its other 3 call sites, since altering
   its semantics could affect `rederive_base_index_ops_post_stage`'s
   established, working behavior for its own DEFINITION-change trigger).

Option 1 is very likely the cleaner fit given `rederive_stale_value_ops_post_stage`
already resolves `tbl` per table_token in its loop — but investigate both
before choosing, and state your reasoning in the final summary (this
decision matters more than the line-level diff, same standard the
original brief set).

**A separate, out-of-scope observation to flag but NOT fix in this
round**: `rederive_base_index_ops_post_stage`'s own use of
`read_pre_tx_bytes`/`data_store.get()` may have this SAME async-tail
staleness exposure for ITS trigger condition (a base_index DEFINITION
change racing a tx). Do not touch that function or its call to
`read_pre_tx_bytes` — just note the observation in your final summary so
it can be tracked as a follow-up task. This round is scoped strictly to
`rederive_stale_value_ops_post_stage`.

## Re-verify after the fix

1. Re-run the exact reproduction:
   `./scripts/test.sh -p shamir-engine -- p1100_stale_snapshot_delete_posting`
   — all 4 tests (2 repro + 2 regression) must pass.
2. Mutation-test: temporarily disable the fixed read (revert to
   `read_pre_tx_bytes`/`data_store.get()`, or otherwise neuter the
   MVCC-visible read), confirm the 2 repro tests fail again, restore,
   confirm all 4 pass.
3. Full gate, exactly as the original brief specified:
   ```
   cargo fmt -p shamir-engine -p shamir-index -p shamir-tx -- --check
   cargo clippy -p shamir-engine -p shamir-index -p shamir-tx --all-targets -- -D warnings
   ./scripts/test.sh -p shamir-engine -p shamir-index -p shamir-tx --full
   ```
   All three must pass clean.

Report the real pass/fail counts, which read primitive you chose (option
1 or 2 above) and why, confirmation of the mutation test, and the
async-tail staleness observation about `rederive_base_index_ops_post_stage`
for the record.
