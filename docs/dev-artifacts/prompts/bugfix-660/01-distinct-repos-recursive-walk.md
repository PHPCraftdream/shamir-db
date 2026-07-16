# #660 — `distinct_repos()` doesn't walk into `Batch`/`ForEach` bodies

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The bug (already root-caused)

`distinct_repos()` (`crates/shamir-query-types/src/batch/query_entry.rs:90`)
collects the set of repos a batch touches, via `qe.op.table_ref()` per
top-level entry. `BatchOp::table_ref()`
(`crates/shamir-query-types/src/batch/batch_op.rs:561-575`) returns `None`
for both `BatchOp::Batch(_)` and `BatchOp::ForEach(_)` — it does NOT walk
into the nested body's `queries` map.

Consequence: a TRANSACTIONAL batch whose ONLY top-level data-bearing entry
is a bare `ForEach` (or `Batch` sub-batch) fails with
`"transactional batch has no data ops to target a repo"`
(`crates/shamir-engine/src/query/batch/batch_execute.rs:443-451`:
`distinct_repos` returns an empty set → `repo_name.is_empty()` → error),
even though the loop body clearly does write to a table in a repo. This was
found during Epic04/E e2e testing and worked around there by adding a
harmless top-level `Read` alongside the `ForEach` (see
`crates/shamir-client/tests/batch_for_each_e2e.rs`'s
`for_each_over_literal_array_...` test, which has a `#660` doc-comment and
an `orders_probe` read).

The Epic04 ADR (`docs/dev-artifacts/design/oql-04-loops-foreach-adr.md`)
explicitly mandates this walk: a `ForEach` node's body's `table_ref()`s
"must be visible to `distinct_repos()` the same way `Batch(sub)`'s are" —
but the walk was never implemented for either variant.

## Why the nested bodies genuinely belong in the outer repo set

A `ForEach`/`Batch` body executes WITHIN the outer transaction (Epic04 ADR
Decision 4: a loop-iteration failure aborts the WHOLE tx batch — so the
body's writes are part of the outer tx). Therefore the nested bodies' repos
genuinely participate in the outer transaction's cross-repo scope and MUST
be collected by `distinct_repos`. This is a correctness fix, not just an
ergonomic one: today the cross-repo single-repo guard
(`batch_execute.rs:79`, `135`) also can't see a nested body that writes to a
DIFFERENT repo than the top-level ops, so a genuinely cross-repo
transactional batch built via a sub-batch/loop could slip past the guard.

## Fix

1. **Make `distinct_repos` recurse into `BatchOp::Batch(sub).batch.queries`
   and `BatchOp::ForEach(fe).batch.queries`.** The cleanest shape is a
   recursive helper that walks a `&TMap<String, QueryEntry>` and, for each
   entry: adds `op.table_ref()`'s repo if present, AND if the op is
   `Batch`/`ForEach`, recurses into its body's `queries`. Implement this in
   `query_entry.rs` alongside `distinct_repos` (keep `distinct_repos`'s
   public signature unchanged — it stays `pub fn distinct_repos(queries:
   &TMap<String, QueryEntry>) -> TFxSet<String>`; just make its body call
   the recursive collector). Watch for: the recursion needs access to the
   `BatchOp` variants' inner `batch` field — check the exact field names
   (`SubBatchOp.batch`, `ForEachOp.batch`) and that they're reachable from
   `query_entry.rs`'s imports.
2. **Update `distinct_repos`'s doc comment** — it currently states
   explicitly that bodies are NOT walked (lines ~79-86); that becomes stale
   and MUST be corrected to describe the new recursive behavior and WHY
   (nested bodies execute within the outer tx, so their repos participate in
   the cross-repo guard).
3. **Consider `BatchOp::table_ref()`** — it returns `Option<&TableRef>`
   (single), which structurally cannot express a nested body's multiple
   tables, so do NOT try to make `table_ref()` itself recurse. Leave
   `table_ref()` as-is (it's used elsewhere for single-table purposes); do
   the recursion in `distinct_repos`'s collector only. If you find other
   callers that would ALSO benefit from the recursive view (grep for
   `table_ref()` usage), note them in your summary but do NOT change their
   behavior in this task unless they're demonstrably part of the same bug.
4. **Remove the #660 workaround** in
   `crates/shamir-client/tests/batch_for_each_e2e.rs`: the
   `for_each_over_literal_array_...` test's extra top-level `orders_probe`
   `Read` (added solely to give `distinct_repos` a `table_ref`) should be
   removed so the test proves a BARE top-level `ForEach` in a transactional
   batch now works end-to-end. Update that test's `#660` doc-comment to note
   the bug is fixed (or remove the now-obsolete workaround explanation).
5. **Add unit tests** in `query_entry.rs`'s test module (or wherever
   `distinct_repos` is currently tested — grep for `distinct_repos` in test
   files):
   - a batch whose only entry is a `ForEach` with a body that inserts into
     `repo="main"` → `distinct_repos` returns `{"main"}` (not empty).
   - same for a bare `Batch(sub)` sub-batch.
   - a nested body whose table is in a DIFFERENT repo than a top-level op →
     `distinct_repos` returns BOTH repos (proving the cross-repo guard can
     now see the nested repo).
   - deep nesting (ForEach inside a Batch inside a ForEach) collects all
     levels' repos.
6. **Update the docs**: `docs/guide-docs/guide/01-queries.md`'s `for_each`
   section currently documents #660 as a known limitation (the
   "bare for_each can't determine its repo, add another top-level op"
   note) — remove/update that note since it's now fixed. Also update
   `docs/dev-artifacts/roadmap/oql/FINAL-SUMMARY.md`'s #660 entry to mark it
   resolved (move it from "known limitations" to a fixed note, or annotate
   it "FIXED in <this commit>").

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-query-types -p shamir-engine --full` green.
- `./scripts/test.sh -p shamir-client --full -- for_each` green (the e2e
  test with the workaround removed must now pass with a bare top-level
  `ForEach`).
- `cargo fmt --check` clean for every touched crate.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.

## Out of scope

- Do NOT change `BatchOp::table_ref()`'s single-`Option` behavior.
- Do NOT touch #641, #643, #634, #659 — separate tasks.
- No TS changes needed (this is a server-side repo-scoping fix; the wire
  shape is unchanged).
