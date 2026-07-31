# F-78 (#905) — stream legacy regular/unique index build instead of materializing the whole table

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## The problem

`TableManager::collect_all_current_records` (`crates/shamir-engine/src/
table/table_manager_streaming.rs`, currently ~line 452) builds a
`Vec<(RecordId, InnerValue)>` of the ENTIRE table in memory, called from
BOTH CREATE INDEX call sites in `table_manager_index_mgmt.rs` (regular,
~line 544; unique, ~line 622). Each callee then builds FURTHER full-table-
sized vectors/maps on top of that:

- **Regular** (`IndexManager::create_index_from_records`,
  `crates/shamir-index/src/legacy/index_manager.rs`, currently ~line 444,
  its Phase 2): iterates `&records` building `posting_writes: Vec<(Bytes,
  Bytes)>` and `cache_index_keys: Vec<Bytes>` — a SECOND full-table-sized
  allocation — then a SINGLE `info_store.set_many(posting_writes)` call
  with the entire batch at once (not chunked).
- **Unique** (`IndexManager::create_unique_index_from_records`,
  `crates/shamir-index/src/legacy/index_manager_unique.rs`, currently
  ~line 371): builds a `TMap<Bytes, usize>` duplicate-count map PLUS an
  `entries: Vec<(RecordId, Bytes)>` — two more full-table-sized
  structures — before any posting is written, then (if no duplicates) a
  THIRD full-table Vec (`writes`) for the final `set_many` call.

So a CREATE INDEX on a medium-to-large table holds multiple O(table)
decoded/derived structures on the heap simultaneously, under a
long-held write lock (F-70's `begin_write_barrier`). Under memory
pressure the process can abort; every writer's latency includes the
FULL build time. This is a highly visible violation of this repo's
`O(x → 0)` pillar ("avoid hidden O(N)/O(N²) in helpers... prefer batched +
amortized over per-row").

**Note this is running on TOP of F-72's (#899) already-shipped
Building→Ready state machine** — `create_index_from_records`'s Phase 1
(register at `Building`) and Phase 3 (flip to `Ready` + persist) are
correct and must NOT be disturbed. Only Phase 2 (the middle, currently
"materialize a Vec, then iterate it") is in scope for this task's
streaming rewrite. The unique path currently has NO Building/Ready
treatment at all (by design — F-72 scoped that gate to planner-visible
reads, and unique-index enforcement is a write-path concern) — do not
retrofit a Building/Ready state onto unique as part of this task; that
would be scope creep.

## The reference pattern already in this codebase

`TableManager::create_sorted_index_with_include`
(`crates/shamir-engine/src/table/table_manager_sorted_index.rs`) ALREADY
streams: `let stream = self.list_stream(1000); ... while let Some(batch) =
stream.next().await { for (id, cow) in batch? { ... self.sorted_indexes.
on_record_created(&id, &record, 0).await?; } }` — O(batch) memory, no
whole-table materialization. Use this as the shape to mirror for the
regular-hash path (which has no duplicate-detection problem, so it can
stream directly, batch-writing postings via `set_many` per batch instead
of one giant call at the end).

## Fix, per family

### Regular (simpler — no cross-record dependency)
Rewrite the CREATE path so records are never materialized into one Vec:
stream via `list_stream` (or whatever seam `collect_all_current_records`
itself uses — check whether it needs to stay for OTHER callers like
`doctor.rs`, in which case only these two CREATE call sites stop using
it), and write postings in bounded batches (e.g. every N records or every
M bytes) instead of one `set_many` at the very end. Preserve F-72's
Phase 1 (register at `Building`) BEFORE the streaming loop and Phase 3
(flip to `Ready`) AFTER it completes — the streaming loop replaces only
the body of Phase 2.

### Unique (harder — duplicate detection needs global knowledge)
Duplicate detection cannot simply stream naively — you cannot know a key
is duplicate-free until you've seen every row with that key, and a naive
two-pass (count first, then write) still needs O(distinct keys) memory
for the count map, which may itself be close to O(table) for a
low-cardinality index. Pick ONE bounded-memory approach and justify it:
- **External/partitioned duplicate detection**: e.g. sort/partition
  streamed index keys into bounded buckets (spill-to-disk or a bounded
  external structure) and detect duplicates within each bucket — bounds
  peak memory to a configurable working set instead of O(table).
- **A temporary unique backend with bounded memory**: stream postings
  into a temporary store, relying on the store's own uniqueness
  primitive to reject/flag a duplicate at write time (e.g. `set` that
  fails/reports on an existing key) rather than an in-memory
  duplicate-count map, then promote/rename that temporary store into the
  live posting space on success (or discard on duplicate-detected
  failure).
If genuinely neither fits in the time available for this task, it is
ACCEPTABLE to defer the unique-family streaming fix explicitly (see the
escape hatch below) — do not ship a half-working, non-deterministic, or
silently-incorrect duplicate-detection rewrite under schedule pressure.

## Progress / checkpoint / cancel + write-delta catch-up

- Add progress/checkpoint support so a long streaming build can report
  how far it has gotten (a simple counter/logged milestone is sufficient
  for this task — a full resumable-checkpoint mechanism is out of scope
  unless it falls out naturally from the chosen approach).
- Prefer a write-delta catch-up model over holding writers blocked for
  the WHOLE scan: since F-70's `begin_write_barrier` already
  raises-bit+drains+locks BEFORE the backfill starts, and the write-hook
  captured at Phase 1 (`Building`) already maintains postings for
  concurrent/new writes during the streaming loop (per the existing
  register-first ordering), reconfirm that this existing mechanism
  already gives write-delta catch-up for free, or identify precisely
  what's missing if it doesn't — do not invent a new mechanism without
  first checking whether the existing register-first + live-hook
  ordering already covers this requirement.

## Measurement (mandatory, per review #1 §9)

Before/after: CREATE INDEX wall time, peak RSS, and writer p95/p99
latency during a build, on a fixture table sized large enough to make
the O(table) allocations actually show up in the numbers (hundreds of
thousands of rows, not a handful).

- Use `bench_scale_tool::Harness` — **NOT Criterion** (see this repo's
  CLAUDE.md: the workspace migrated off Criterion; copy an existing bench
  file, e.g. `crates/shamir-engine/benches/tx_pipeline.rs`, as the
  template).
- Run with `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench` so the
  test/clippy incremental cache is not invalidated (per CLAUDE.md's bench
  cache isolation rule).
- Report the before/after numbers explicitly in the commit message —
  this task's whole justification is a measurable resource-usage
  reduction, so ship the evidence, not just the claim.

## Definition of done

- A correctness-equivalence test: build the SAME index both the OLD
  (materializing) way and the NEW (streaming) way against an identical
  fixture table, and assert the resulting posting SETS are identical
  (same keys, same values) — the streaming rewrite must not change WHAT
  gets indexed, only HOW.
- For unique: a test proving duplicate detection still correctly rejects
  a table with a genuine duplicate under the new bounded-memory approach,
  with the same (or a clearly-documented, still-useful) error shape as
  today's `DbError::UniqueIndexCreationFailed`.
- The benchmark results described above, committed alongside the code
  (as a doc/comment or a checked-in results snippet — your call on
  format, but it must be reviewable, not just asserted in a chat
  message).
- `cargo fmt -p shamir-index -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-index -p shamir-engine --full` green.
- **Escape hatch** (this is a substantial P1 task): if the unique-family
  bounded-memory rewrite cannot be completed soundly in this pass, it is
  acceptable to land ONLY the regular-hash streaming fix (fully
  correctness-tested and benchmarked) and explicitly document the
  unique-family gap as deferred follow-up work in the commit message,
  rather than ship an unsound or untested unique-duplicate-detection
  rewrite.
- Do not touch F-72's Building/Ready state-machine mechanics beyond
  reshaping Phase 2's body from "materialize then iterate" to "stream
  and batch-write."
- Do not run this task concurrently with any other task touching
  `index_manager.rs`, `index_manager_unique.rs`,
  `table_manager_index_mgmt.rs`, or `table_manager_streaming.rs`.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
