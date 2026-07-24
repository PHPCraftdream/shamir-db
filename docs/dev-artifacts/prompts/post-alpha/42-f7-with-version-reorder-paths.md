# Brief for #797 (F-7) — with_version: reject for aggregates, actually work for plain ORDER BY

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context — read this file's own doc comment first

`crates/shamir-engine/src/table/read_exec.rs` lines 42-60 are a module-level
doc comment explaining the EXISTING, deliberate design: `with_version`
populates `QueryResult::versions` (a `Vec<u64>` index-aligned with
`records`) ONLY on read paths where records preserve scan order; ORDER BY /
DISTINCT / GROUP BY / aggregates reorder or collapse rows AFTER the
per-row version would have been read, so `versions` is left `None` even
when the client asked. This was an accepted, documented tradeoff from an
earlier task (FG-2).

A release-readiness review flagged this as too coarse: **aggregates and
GROUP BY should reject `with_version` outright** (a hard, clear error at
request time) rather than silently returning `None` — there is no
principled per-record version for a collapsed/aggregated row, so silence
here just hides a client's mistaken assumption. **Plain `ORDER BY` (no
`GROUP BY`, no aggregate, no `DISTINCT`) should actually WORK** — carry
each surviving row's `RecordId` through the reordering, and build the
`versions` array from those repositioned ids, since a plain sort doesn't
collapse row identity, it only reorders it.

**`DISTINCT` is treated the same as GROUP BY/aggregates for this task** —
even though the review's shorthand only explicitly named "aggregates" for
the reject bucket, DISTINCT is likewise a many-to-one collapse (multiple
distinct-duplicate rows become one output row), so which of several
possible source versions would `versions[i]` even mean is exactly as
ill-defined as it is for GROUP BY. Reject `with_version` for
`query.select.distinct == true` alongside `has_group_by`/`has_agg`.

## Investigation already done (verified by reading the code, not assumed)

- The dispatch in `read_exec.rs` (~line 772-810) computes
  `needs_full_collect = has_group_by || has_agg || has_order || has_distinct`
  and routes to `read_collecting` for all four. Inside `read_collecting`
  (~line 835 onward), the SAME flags are re-derived (`has_group_by`,
  `has_agg` ~line 848; `has_distinct` used later around line 969).
- `needs_raw` (~line 848: `let needs_raw = has_group_by || has_agg;`)
  decides which accumulator gets populated during the scan loop
  (~line 894-946): when `needs_raw`, rows go into
  `raw_acc: Vec<(RecordId, Bytes)>` (RecordId retained, needed by the
  aggregate/group-by lens); when NOT `needs_raw` (i.e. the plain-ORDER-BY-
  or-DISTINCT-or-neither case), rows go into
  `rec_acc: Vec<QueryRecord>` — **the `id` from each `(id, cow)` pair is
  discarded here today, never retained.** This is the gap that must be
  closed for the plain-ORDER-BY-with_version case: `rec_acc`'s
  accumulation loop needs a parallel `id_acc: Vec<RecordId>` (populated
  only when `query.with_version` is true, mirroring the existing
  `tracking_versions` conditional-tracking pattern already used elsewhere
  in this same file, e.g. ~line 481-482's `tracking_versions`/
  `survivor_ids` in a different function — copy that style).
- After accumulation, `qv_result: Vec<QueryValue>` is built from
  `rec_acc` (~line 956-963, the `else` arm of the `has_group_by`/`has_agg`
  dispatch at ~line 949-964) — this is exactly the branch where
  `id_acc` needs to be threaded alongside.
- Reordering happens at ~line 972-1003: `use_topk` (top-K heap,
  `exec::apply_order_by_topk`) or the `else` branch (full sort via
  `exec::apply_order_by_qv` + `exec::apply_pagination`). **F-6 (#796,
  already landed) added `&& !query.count_total` to the `use_topk` gate**
  for an analogous reason (the heap can't supply a total). This task
  adds `&& !query.with_version` to that SAME gate for the SAME reason
  (the heap-based top-K path, `apply_order_by_topk`, does not carry
  `RecordId` alongside its `BinaryHeap<QueryValue>` entries — threading
  ids through it is a separate, larger change explicitly out of scope
  here; forcing the full-sort path when `with_version` is requested is
  the same tradeoff F-6 made for `count_total`). Document this exclusion
  the same way F-6 documented its own.
- `crates/shamir-engine/src/query/read/order.rs`'s `apply_order_by_qv`
  (~line 26-49) ALREADY computes an index permutation internally
  (`idx: Vec<usize>`, sorted, then applied to `records` in Phase 3) — this
  is the exact mechanism needed to also permute a companion `RecordId`
  vector in lockstep. Add a sibling function, e.g.:
  ```rust
  pub fn apply_order_by_qv_with_ids(
      records: &mut Vec<QueryValue>,
      ids: &mut Vec<RecordId>,
      order_by: &OrderBy,
  )
  ```
  that does the SAME phase 1/2/3 as `apply_order_by_qv` but applies the
  identical `idx` permutation to `ids` as well as `records` (both vectors
  must be the same length going in — this is an invariant the caller
  guarantees, document it). Consider whether the existing
  `apply_order_by_qv` can be refactored to share the phase-1/phase-2 key
  computation with the new function (avoid duplicating
  `resolve_qv_order_keys`/`compare_qv_preresolved` calls) — but keep the
  diff surgical; a thin wrapper or shared private helper is fine, a full
  rewrite is not needed.
- `collect_versions`/`versions_from_matched` (~line 62-105 of
  `read_exec.rs`) are the EXISTING helpers that build the final
  `QueryResult::versions` from a list of `RecordId`s + `self.mvcc_store_ref()`
  — reuse one of these for the final `versions` field in the plain-ORDER-BY
  branch, don't reinvent the version lookup.

## The fix

1. **Upfront rejection.** As early as cleanly possible (either in the
   outer dispatch ~line 772-810, before `read_collecting` even starts its
   scan setup, or at the very top of `read_collecting` itself — pick
   whichever reads more naturally given the existing flag computations at
   each site), when `query.with_version && (has_group_by || has_agg ||
   query.select.distinct)`, return
   `Err(DbError::Validation("with_version is not supported with GROUP BY, \
   aggregates, or DISTINCT — no single version applies to a collapsed \
   row".to_string()))` (adjust the exact wording; check this file's
   existing `DbError::Validation` usage elsewhere in the crate for the
   established message style/convention first — match it rather than
   inventing new phrasing from scratch).
2. **Plain ORDER BY (no group_by, no agg, no distinct) + with_version:**
   - Add an `id_acc: Vec<RecordId>` (or reuse an existing naming
     convention if a very similar pattern already exists nearby) tracked
     alongside `rec_acc`'s accumulation, gated on `query.with_version`
     (skip the tracking entirely when not requested, to avoid the extra
     `Vec` allocation/pushes on the common path).
   - After `qv_result` is built from `rec_acc`, if `query.with_version` is
     true and this is the plain-ORDER-BY branch, keep `id_acc` in lockstep
     alongside `qv_result` up to the point of reordering.
   - Exclude `use_topk` when `query.with_version` is true (see above) —
     `&& !query.count_total && !query.with_version` on the same gate line.
   - In the full-sort `else` branch, call the new
     `apply_order_by_qv_with_ids(&mut qv_result, &mut id_acc, order_by)`
     instead of `apply_order_by_qv` when `query.with_version` is true (and
     this is the plain-ORDER-BY branch); otherwise keep the existing call
     unchanged.
   - After pagination slicing, the `versions` field for the final
     `QueryResult` must be built from the SAME slice of `id_acc` that
     survived pagination (skip/take) — `id_acc` needs the identical
     skip/take slicing `apply_pagination` applies to `qv_result`, or you
     restructure so both are sliced together (e.g. zip
     `(id_acc, qv_result)` into pairs before pagination, slice the pairs,
     then unzip — whichever is the smaller, clearer diff against the
     existing code shape). Use `collect_versions`/`versions_from_matched`
     with the FINAL, paginated id slice to build `versions` — not the
     pre-pagination full list.
   - When ORDER BY is absent (records preserve scan order, e.g. no ORDER
     BY / GROUP BY / DISTINCT / aggregates at all reaching this
     `read_collecting` path — check whether this combination is even
     reachable here, since `needs_full_collect` requires at least one of
     the four flags — if unreachable, no change needed for that case)
     — N/A if unreachable, just confirm.
   - `versions` stays `None` for non-MVCC tables (no `self.mvcc_store_ref()`)
     even when everything else lines up — mirror the EXISTING
     `tracking_versions` pattern's own `self.mvcc_store_ref().is_some()`
     check (see e.g. ~line 481, 1074, 1227 for how other paths in this
     file already gate on this) — don't invent a different convention.
3. **DISTINCT + with_version, and GROUP BY/aggregates + with_version** are
   both now hard errors per step 1 — no `versions: None` silent path
   remains reachable for them (double-check by reading the final
   `Ok(QueryResult { ... versions: None ... })` at ~line 1018-1031 — once
   the reorder-path fix lands, is `versions: None` still correct for the
   cases that DIDN'T take the with_version-ids path, e.g. `with_version ==
   false`? Yes — `None` is still correct there, just make sure the new
   `Some(...)` path is wired in for the one case it needs to be).

## Tests

Find or create the test file(s) covering `read_exec.rs`'s ORDER BY /
GROUP BY / with_version behavior (check
`crates/shamir-engine/src/table/tests/` for existing `with_version`
coverage first — the module doc comment mentions FG-2, search for that
marker too) and add:

1. **Plain `ORDER BY` + `with_version: true`** (no group_by/agg/distinct)
   → `QueryResult::versions` is `Some(...)`, index-aligned with the
   REORDERED `records` (not scan order) — i.e. `versions[i]` must be the
   correct version for `records[i]` AFTER sorting, not before. Include a
   pagination case (`ORDER BY` + `LIMIT`/`skip`) to prove the paginated
   slice of `versions` lines up with the paginated slice of `records`.
2. **`GROUP BY` + `with_version: true`** → hard error (check the exact
   error surfaces correctly through the wire/BatchError layer, not just
   at the `DbResult` level — trace how `DbError::Validation` propagates to
   a client-visible error code in this codebase's existing convention).
3. **Aggregate `SELECT` (e.g. `count`/`sum`) + `with_version: true`** →
   hard error, same shape as (2).
4. **`DISTINCT` + `with_version: true`** (no group_by/agg) → hard error.
5. **`ORDER BY` + `with_version: true` on a non-MVCC table** → `versions`
   stays `None` (not an error — this mirrors the EXISTING documented
   non-MVCC exception from the FG-2 module doc comment; with_version is
   "opt-in assistance, never a correctness contract").
6. Confirm the F-6 `count_total_true_excludes_topk_fast_path`-style test
   pattern (in `covering_read_tests.rs`) still passes unmodified — this
   task's `&& !query.with_version` addition to `use_topk` must not affect
   `count_total`-only queries that don't also request `with_version`.

## Constraints

- Do NOT attempt to thread `RecordId` through `apply_order_by_topk`'s
  heap-based path — excluding `with_version` from `use_topk` (forcing the
  full-sort fallback) is the accepted, in-scope mitigation, matching what
  F-6 already did for `count_total`.
- Do NOT change behavior for `with_version == false` — every existing
  code path for that case is unaffected by this task.
- Do NOT touch the query-builder (Rust `shamir-query-builder` /
  TS `shamir-client-ts`) to add client-side validation for this
  combination — the engine-level hard error is sufficient for
  correctness; builder-side fail-fast validation is explicitly OUT of
  scope for this task (a possible future nice-to-have, not required
  here).
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.
- Follow workspace conventions: `use` at file top, `mod.rs` re-exports
  only, one primary export per file, surgical diff.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine
```
