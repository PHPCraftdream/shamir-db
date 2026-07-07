Task: CRIT-7 — index2 empty result set falls through to full scan instead
of returning zero rows.

## Where

`crates/shamir-engine/src/table/read_exec.rs:332-403` (the plain index2
path, inside the query-execution method that starts around line ~280).

```rust
// ── index2: FTS / Functional / Vector accelerated path ─────
if let Some(ref filter) = query.r#where {
    if let Some(result) = self.try_plan_index2(filter, interner).await {
        let (rids_vec, index_tag) = match result {
            crate::index2::backend::IndexResult::Set(rids) => {
                (rids.into_iter().collect::<Vec<_>>(), "index2")
            }
            crate::index2::backend::IndexResult::Ranked(ranked) => (
                ranked.into_iter().map(|(r, _)| r).collect::<Vec<_>>(),
                "index2_ranked",
            ),
        };
        if !rids_vec.is_empty() {
            // ... builds the QueryResult from rids_vec and RETURNS Ok(...)
        }
        // <-- BUG: if rids_vec IS empty, execution falls out of this
        //     `if let Some(result) = ...` block entirely (no early
        //     return), then falls through the legacy btree index-scan
        //     check below (which won't match — the filter used index2,
        //     not a btree index), and eventually reaches the full-scan
        //     path further down in this same function.
    }
}

// Try index scan first (legacy btree)
if let Some(ref filter) = query.r#where {
    if let Some((idx_name, lookup_sets, residual)) = self.try_plan_index_scan(filter, interner) {
        // ... does not match here, falls through further
    }
}
// ... eventually reaches a full-scan evaluation of the compiled FilterNode
// against every row in the table.
```

## Why this is CRITICAL (not just a perf nit)

`try_plan_index2` returning `IndexResult::Set(rids)` or
`IndexResult::Ranked(ranked)` is index2's **authoritative, complete**
answer for that filter — an empty set means "zero rows match", full
stop. The current code only handles the non-empty case explicitly; an
empty result is silently treated as "index2 didn't apply" and the query
falls through to a full table scan.

Two distinct severities:

1. **FTS / functional index2 (`IndexResult::Set`/`Ranked` from a
   text-match or functional-index backend):** an empty match set on a
   full scan re-evaluates the *original* filter node (e.g.
   `FilterNode::FtsMatch`) against every row — tokenizing + lowercasing
   every row's text field again, redundantly, since index2 already
   proved none of them match. Correctness is preserved (the full scan
   re-derives the same "zero matches" answer) but at O(N) needless cost
   — see `docs/audits/2026-07-06-perf-hot-paths.md:14` for the ~400×
   figure on a 10k-row miss.

2. **`VectorSimilarity` via index2 (the plain, non-filtered-vector path
   — NOT the `read_filtered_vector_scan`/V3.1/V3.2 code around line
   1250+, which is a separate, already-correct path with its own
   fallback):** `Filter::VectorSimilarity { .. }` compiles to
   `FilterNode::True` (`crates/shamir-engine/src/query/filter/compile.rs:123`)
   — because a vector-similarity predicate has no meaningful
   post-filter-node representation; it's meant to be answered ENTIRELY
   by the vector index, never re-evaluated row-by-row. When index2
   returns an empty set here and the code falls through to the full
   scan, the full-scan filter evaluates to `true` for EVERY row (since
   the compiled node is `FilterNode::True`) — the query returns **every
   row in the table** instead of zero. This is a correctness bug, not
   just a perf one: a "find nearest neighbours, none within threshold"
   query returns the entire table.

## Fix

When `try_plan_index2` returns `IndexResult::Set`/`IndexResult::Ranked`
(regardless of whether `rids_vec` is empty), that result is
authoritative — return the (possibly empty) `QueryResult` immediately,
the same way the current `if !rids_vec.is_empty()` block does, just
without gating on non-emptiness. Concretely: hoist the existing result-
building logic (records/pagination/stats construction currently inside
`if !rids_vec.is_empty() { ... }`) so it runs unconditionally once
`try_plan_index2` has returned `Some(result)`, and returns
`Ok(QueryResult { records: vec![], ... })` (with `records_scanned: 0`,
`records_returned: 0`) when `rids_vec` is empty. Do NOT special-case
`VectorSimilarity` separately — the fix must be general to any index2
result (Set or Ranked), since the same fall-through bug affects FTS too
(the perf side of it).

Double-check: `get_many_bytes(&[])` on an empty slice — confirm it
handles a zero-length input cheaply (should short-circuit without any
storage round-trip; if it doesn't, that's a secondary tightening but
not blocking for this fix).

## TDD requirement (mandatory — this is a correctness bug, not a
refactor)

1. **Red**: write a failing `#[tokio::test]` (or extend an existing
   index2/vector-search integration test) that:
   - Creates a table with a vector index (or FTS index2 — cover both
     shapes if feasible in one pass, otherwise prioritize
     `VectorSimilarity` since it's the correctness-breaking case).
   - Seeds rows that do NOT match / are outside any similarity
     threshold that would return anything, OR queries an empty table.
   - Issues a `VectorSimilarity` query (or FTS query) expected to match
     ZERO rows.
   - Asserts the returned `QueryResult.records` is empty AND
     `records_returned == 0`. Before the fix, this should currently
     fail by returning all rows (or all rows matching residual, for
     FTS) instead of zero.
2. **Green**: apply the minimal fix described above.
3. Confirm the fix doesn't break any EXISTING index2/vector-search
   tests (there are existing suites under
   `crates/shamir-engine/src/table/tests/` and vector-related
   integration tests from the VR/vector campaign — run the relevant
   scope, not the whole workspace, first).

## Test scope command (existing test wrapper — do NOT use raw `cargo test`)

```
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-engine --full   # if the new test is an integration test, not unit
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- The failing test you wrote (file + test name) and what it asserted
  before/after the fix.
- The exact diff shape of the fix (which lines moved/changed in
  `read_exec.rs`).
- Whether you covered both the FTS/functional case and the
  VectorSimilarity case, or just one (and why, if only one).
- Any pre-existing clippy/fmt issues found but not touched.
