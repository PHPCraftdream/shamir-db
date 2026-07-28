# Brief for F-53a (#874, P1) — streaming top-K: bound memory during the scan, not after

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. `crates/shamir-engine/src/table/read_exec.rs`'s
`read_collecting` accumulates EVERY matched+projected row into a
`Vec<QueryRecord>` (`rec_acc`, first declared ~line 943, pushed ~lines
998/1031, returned ~line 1052) BEFORE `apply_order_by_topk`
(`crates/shamir-engine/src/table/order.rs:111-204`, or nearby — confirm
exact location) ever runs. The existing code comment at
`read_exec.rs:1067-1068` claims "O(K) memory instead of O(N)" — this is
**factually wrong**: that claim only describes the `BinaryHeap`'s own
internal workspace inside `apply_order_by_topk` (which IS correctly bounded
to `k = skip + take`, `order.rs:166`). The `rec_acc` Vec collecting into it
is still O(N) — every matched, fully-projected row, before the heap trim
ever sees them.

**Read the investigation findings below before touching anything — they
are already confirmed against the current code, do not re-derive them:**

### Confirmed current shape

- `read_collecting`'s scan loop (`read_exec.rs:~970-1042`, per stream batch):
  decode (`RecordView::new`) → filter (`cb.matches`) → project
  (`proj.project_value`) → unconditional push into `rec_acc`. Nothing about
  K or the sort spec influences this loop today, even though both are known
  from the `ReadQuery` at function entry.
- `count_total` (`read_exec.rs:~1075`, `order.rs`'s `apply_pagination:138-139`):
  computed as `records.len()` AFTER full materialization. Must become an
  independent running counter if the loop stops accumulating everything.
- `with_version` mode (`read_exec.rs:949-954,1001-1003,1034-1036`): tracks a
  companion `id_acc: Vec<RecordId>` in lockstep with `rec_acc`, sorted
  together (`read_exec.rs:1116`), then the per-record version array is
  rebuilt from the PAGINATED survivor IDs only (`read_exec.rs:1154`) — this
  mode only ever needs the FINAL top-K survivors' RecordIds, not the full
  scan's IDs, so it composes cleanly with a bounded heap as long as each
  heap item carries its RecordId.
- `apply_order_by_topk`'s actual comparison/tie-break logic (`order.rs:111-204`):
  multi-key ORDER BY via `SmallVec<[QvSortKey; 4]>` per record, per-key
  `OrderDirection::{Asc,Desc}` + `NullsOrder::{First,Last}`, insertion-index
  tie-breaking for stable-sort semantics, a max-heap (root = worst
  candidate) that evicts on a better incoming row. **This comparison logic
  is correct and must NOT change** — only WHERE the bounding happens moves
  (into the scan loop), not the comparison semantics.
- **No existing `BinaryHeap` usage anywhere else** in `shamir-engine` or
  `shamir-index` to reuse — `apply_order_by_topk`'s heap is the only one;
  treat its comparator/`HeapItem` shape as the thing to extract and reuse
  inline in the scan loop, not something to reinvent from scratch.
- **A second, independent site with the SAME bug**: `read_temporal.rs`'s
  `read_as_of` (the cursor path — `cursor_handlers.rs:1-96` routes through
  `Temporal::AsOf { at: At::Version(pinned) }` → `read_as_of`,
  `read_exec.rs:296-297`) also fully materializes `matched: Vec<(RecordId,
  Bytes)>` (`read_temporal.rs:~99`) before applying ORDER BY + pagination
  post-hoc (`read_temporal.rs:~197-202`) — **with NO bounded heap at all**,
  always a full O(N) sort even with a LIMIT. This is the SAME class of fix;
  apply it here too, not just in `read_collecting`.

## What to implement

Merge the WHERE-filter → projection → sort-key extraction directly into a
bounded max-heap DURING the scan, in BOTH `read_collecting` (`read_exec.rs`)
and `read_as_of` (`read_temporal.rs`):

1. When `query.order_by.is_some() && take_resolved.is_some()` (the existing
   `use_topk` gate condition, `read_exec.rs:~1070-1076` — check the EXACT
   condition, including how `count_total` currently disables it, and decide
   whether `count_total` can be supported alongside the bounded heap via an
   independent counter rather than disabling the optimization entirely),
   compute each row's sort key(s) INLINE in the scan loop (reusing
   `apply_order_by_topk`'s existing key-extraction/comparison logic —
   extract it into a shared helper both the old full-sort fallback path and
   the new inline-heap path can call, rather than duplicating the
   comparator) and push into a `k = skip + take`-capacity `BinaryHeap`
   immediately, WITHOUT ever accumulating the full `rec_acc`/`matched` Vec.
2. `count_total`, if requested, becomes an independent running counter
   incremented once per row that passes the WHERE filter (regardless of
   whether it makes it into the heap) — decouple it from `rec_acc.len()`.
3. `with_version` mode: each heap item must carry its `RecordId` (mirroring
   the existing `id_acc` pairing) so the post-heap version-array rebuild
   (`read_exec.rs:~1154`) still works unchanged — only the SOURCE of the
   survivor IDs moves (from a full paginated slice to the heap's final
   drained-and-sorted survivors).
4. Preserve the EXACT existing fallback path (no `ORDER BY`+`LIMIT`, or
   `GROUP BY`/aggregate/`DISTINCT` present, `read_exec.rs:~837`) — this
   task only changes the `use_topk` branch's internals, not when that
   branch is taken or what the non-top-K paths do.
5. `read_temporal.rs`'s `read_as_of`: apply the same inline-heap treatment
   when its caller has an ORDER BY + LIMIT (check whether it currently
   even has access to those at the point `matched` is built — if the
   temporal path's function signature doesn't carry order/limit down to
   the scan loop today, that's a real, separate finding — investigate and
   report rather than assuming; wiring it through may be a larger, separate
   change than `read_collecting`'s fix, in which case timebox and land
   `read_collecting`'s fix fully, document what's needed for
   `read_temporal.rs` in your final summary rather than forcing both into
   one pass if the second is genuinely more invasive).

## What NOT to do

- Do NOT change `apply_order_by_topk`'s comparison/tie-break semantics —
  multi-key direction, NULL ordering, and stable-sort tie-breaking must
  produce byte-identical results to today, just computed earlier in the
  pipeline.
- Do NOT touch the non-top-K fallback paths (no ORDER BY, GROUP BY,
  aggregates, DISTINCT) — those are out of scope.
- Do NOT touch F-53b's cursor/continuation redesign (page-to-page
  rescanning) — that is a DIFFERENT, separate problem (this task fixes
  per-page memory bounding; F-53b will fix cross-page rescan cost) tracked
  as its own task (#875). Do not attempt to solve both here.

## Benchmark

Add (or extend an existing) `bench_scale_tool::Harness`-based bench in
`crates/shamir-engine/benches/` (copy an existing file, e.g.
`tx_pipeline.rs`, as the template — see CLAUDE.md's bench-scale-tool
convention, do NOT reach for Criterion APIs) proving the fix: a
large-N/small-K scan + WHERE + `ORDER BY ... LIMIT K` workload, before vs.
after, showing materially lower peak allocation/latency. Run it via
`CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p
shamir-engine --bench <name>` per the project's bench-cache-isolation rule
— NOT mixed into the same target dir as `cargo test`/`clippy` runs.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- Write/extend tests confirming: identical ORDER BY output (multi-key,
  mixed asc/desc, NULL ordering) between the OLD full-materialize path
  (temporarily keep it reachable via a test-only comparison, or diff
  against a hand-computed expected order) and the NEW inline-heap path, for
  at least one representative case per ordering variant; `count_total`
  correctness when the heap discards non-survivor rows; `with_version`
  mode's version array still correctly maps to the paginated survivors.
- Clean up any scratch/debug log files you create in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-engine --bench <your-bench-name>
```

When done, give your final summary as plain text: the exact mechanism
(where the heap now lives, how `count_total`/`with_version` compose with
it), whether `read_temporal.rs`'s `read_as_of` got the same fix or was
found to need separate follow-up (and why), the before/after benchmark
numbers proving the memory/latency improvement, test results, and
confirmation fmt/clippy are clean.
