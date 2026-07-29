# Brief for F-53c (#876, P2) — wire index-assisted lookup into FK CASCADE/SET NULL actions

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. Investigated this session
(Explore-agent, read-only). Confirmed current state — **the scope is
narrower than the original review suspected**:

- **RESTRICT already uses an index fast-path.** `fk_restrict.rs`'s
  `child_has_reference` (`:292-387`) calls `table.find_single_field_index`
  (`read_planner.rs:154-161`) and, if a supporting index exists on the FK
  column, uses `index_manager_ref().lookup_by_index()` — only falling back
  to a full `list_stream_tx` scan when no index exists or the field is
  overlay-only. **Do not touch this — it's already correct.**
- **CASCADE / SET NULL do NOT use this same fast-path — this is the real
  gap.** `fk_actions.rs`'s `plan_cascade_recursive` (`:354-452`,
  child-level scan `:404`; grandchild variant `:700`) and
  `fk_on_update.rs`'s CASCADE/SET NULL handling (`:417-468`, scans at
  `:445`/`:877`) do an unconditional full `list_stream_tx` scan with
  manual per-row field matching — even when
  `find_single_field_index` would find a usable index on the exact same FK
  column `child_has_reference` already checks for RESTRICT. **This is the
  concrete, scoped fix.**
- **Reverse-FK cache cold-miss cost is real but already cache-aside** —
  `fk_reverse_cache.rs`'s `get_or_build_by_parent` → `build_reverse_fk_entries`
  (`:486-516`) does a genuine O(all tables in repo) scan (`list_table_names`
  + per-table schema read) ONLY on a cold miss (F-28 Step 4, #831 already
  made this cache-aside with whole-repo invalidation on schema mutation) —
  every subsequent FK op against a warm cache is O(1). **This is lower
  priority than the CASCADE/SET NULL scan gap** (which pays its cost on
  EVERY action execution, not just once after a schema change) — treat as
  optional/secondary, see "Optional" section below.
- **Multi-level cascade discovery already benefits from the warm cache** —
  `plan_cascade_for_ids`'s recursion (`:500-512`, `:777-789`) calls
  `discover_action_refs` at each level, but that hits the (by then warm)
  reverse-FK cache, not a fresh table scan. No separate fix needed here
  beyond whatever the CASCADE/SET NULL child-scan fix below provides at
  each level (the recursion naturally inherits it).
- **Cross-reference confirmed**: F-40b's RI barrier research
  (`docs/dev-artifacts/research/f40b-ri-barrier-spike.md:26-62`) already
  documents that `child_has_reference` does a raw `list_stream_tx` (not
  `filter_stream_tx`) for its predicate recording, accepting a coarse
  `TableScan` SSI dependency, and that the RI barrier's own token
  recording deliberately happens at function ENTRY (before the index
  fast-path branches) specifically so it fires regardless of which
  sub-path executes. **This fix must not disturb that recording point** —
  wiring an index fast-path into the CASCADE/SET NULL scan must still
  record the SAME predicate dependency (`PredicateDep::TableScan` or
  whatever RESTRICT's existing pattern uses) at the same relative point,
  or the RI barrier's mutual-exclusion guarantee (F-46, already closed
  this session) could regress. Read `fk_restrict.rs`'s exact ordering
  (predicate recording vs. index-fast-path branch) before touching
  `fk_actions.rs`/`fk_on_update.rs` — mirror it exactly.

## What to implement

1. **`fk_actions.rs`'s `plan_cascade_recursive`** (child-level scan `:404`,
   grandchild variant `:700`): before the unconditional `list_stream_tx`
   scan, check `find_single_field_index` for the FK column exactly as
   `child_has_reference` does. If found, use
   `index_manager_ref().lookup_by_index()` to get the candidate RecordIds
   directly (mirroring RESTRICT's pattern), instead of streaming and
   matching every row. Preserve the EXACT SSI predicate recording ordering
   `child_has_reference` uses (see the cross-reference note above) so the
   RI barrier / SSI dependency tracking is unaffected.
2. **`fk_on_update.rs`'s CASCADE/SET NULL scans** (`:417-468`, scan sites
   `:445`/`:877`): same fix, mirroring #1.
3. **Batch old-parent-value lookups where the action needs the pre-update
   value** — investigate whether the current per-row scan already reads
   what it needs in one pass, or whether there's a redundant second lookup
   per affected row that an index-assisted path could collapse into a
   single batched read. Only fix this if investigation shows a genuine
   redundant-lookup pattern — do not invent batching that isn't needed.
4. **Cache action plans by schema generation** — investigate whether a
   repeated cascade against the SAME (parent, action-kind) pair within a
   short window re-derives an identical plan shape each time (not the
   scan results — the STRUCTURE of what index/scan to use). If schema
   changes are rare relative to FK actions (they are — DDL is infrequent),
   a small per-(table, FK-column) cache of "which index (if any) to use"
   keyed by schema generation could avoid repeating the
   `find_single_field_index` lookup on every action. Only implement this
   if it's a small, low-risk addition — do not build new invalidation
   infrastructure if the existing reverse-FK cache's generation-tracking
   can be reused/extended cheaply. If it looks disproportionate, defer and
   say so in your summary.

## Optional (only if time permits within your timebox)

**Persisted reverse-FK catalog** (replacing/supplementing the runtime
`build_reverse_fk_entries` schema-scan on a cold cache miss): this is
real but lower-priority (only pays its cost once per schema-mutation
cache invalidation, not per-action). If you have time after items 1-3
above, investigate whether a small persisted catalog (built once at
schema-bind time, updated incrementally on FK-relevant schema changes)
is a clean, low-risk addition — but do NOT let this crowd out items 1-2
above, which are the actual scoped fix for this task. If you don't reach
this, say so explicitly and it will be tracked as a follow-up.

## What NOT to do

- Do NOT touch `fk_restrict.rs`'s `child_has_reference` — it already does
  the right thing (index-aware with scan fallback).
- Do NOT touch F-46/F-47's RI barrier / reverse-FK-cache correctness fixes
  from earlier this session — this task is about SCAN COST, not the
  concurrency correctness those fixed. Read the RI barrier's predicate-
  recording ordering carefully so this task's changes don't regress it,
  but do not change that mechanism itself.
- Do NOT touch F-53a/F-53b's landed streaming top-K / cursor-seek spike
  work — unrelated.

## Benchmark

Add (or extend an existing) `bench_scale_tool::Harness`-based bench in
`crates/shamir-engine/benches/` proving the fix: a parent table with many
children (e.g. 10k+ child rows, a small fraction matching the FK
constraint's referencing value) and a supporting index on the FK column,
before vs. after, for a CASCADE or SET NULL action — showing materially
fewer rows scanned / lower latency. Run via
`CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p
shamir-engine --bench <name>`, isolated from `test`/`clippy`'s target
dir per the project's bench-cache convention.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- Write/extend tests confirming: CASCADE/SET NULL still produce
  IDENTICAL results (same rows affected, same final state) whether or not
  an index exists on the FK column (i.e. both the index-fast-path and the
  scan-fallback paths must be exercised and produce the same outcome);
  the RI barrier's mutual-serialization test suite
  (`fk_ri_barrier_tests.rs`) still passes unchanged (regression guard —
  confirms the predicate-recording ordering wasn't disturbed).
- Clean up any scratch/debug log files you create in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-engine --bench <your-bench-name>
```

When done, give your final summary as plain text: exactly which of items
1-4 you completed (and whether the optional persisted-catalog item was
reached or deferred), the index-fast-path mechanism you wired in and how
you preserved the RI barrier's predicate-recording ordering, before/after
benchmark numbers, test results (including confirming
`fk_ri_barrier_tests.rs` still passes), and confirmation fmt/clippy are
clean.
