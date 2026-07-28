# Brief for F-53b Step 1 (#875, P1, spike) — cursor index-seek design: can it respect a pinned AsOf snapshot?

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is a **timeboxed design spike**, mirroring this session's established
precedent (F-40b's Step 1→Step 2 split, F-50's Step 1→Step 2→Step 3a→Step
3b arc). The goal is settling a design and proving the trickiest mechanism
works via a minimal prototype — NOT a full production implementation.

`docs/guide-docs/KNOWN_LIMITATIONS.md:402-405` already honestly documents:
cursor `FetchNext` "still executes a full pinned-version scan per page
internally (no true server-side streaming cursor at the engine level)" —
every page re-scans the whole matched set from scratch and re-applies the
boundary filter, so page N costs cumulative O(N × table_size), not
O(page_size).

## What was already investigated this session (do not re-derive)

**Cursor architecture** (`crates/shamir-server/src/db_handler/cursor_handlers.rs`,
1832 lines; `crates/shamir-server/src/cursor_registry.rs`):
- `CreateCursor` opens an MVCC snapshot ONCE (`gate.open_snapshot()`,
  `cursor_handlers.rs:1140`) and pins to that `pinned_version` for the
  cursor's ENTIRE lifetime (`cursor_handlers.rs:1141`). Every `FetchNext`
  reads via `Temporal::AsOf { at: Version(pinned_version) }`
  (`cursor_handlers.rs:1258-1260`), which routes to `read_as_of`
  (`read_temporal.rs:45`). **This snapshot-isolation guarantee (concurrent
  writes after `CreateCursor` invisible to every subsequent page) is a
  hard invariant any redesign MUST preserve exactly** — do not weaken it to
  read-latest-per-page.
- `CursorState` tracks `seek_key: Option<QueryValue>` (last-returned ORDER
  BY value), `tie_skip: u64` (rows already returned tied on `seek_key`,
  "CR-A4, #764"), `offset: u64`, and `mode: PaginationMode::{Keyset,Offset}`.
  `Keyset` mode applies for single-column ORDER BY; `Offset` for row-count
  pagination (no ORDER BY, or unsupported ORDER BY shapes).
- Every `FetchNext` clones the base query, builds a boundary filter
  (`field >= seek_key` / `<=`, inclusive per "CR-A4") via `boundary_filter()`
  (`cursor_handlers.rs:363-403`), AND-combines it with the original WHERE,
  and calls `read_as_of` — which does a **full scan of everything matching
  the boundary filter**, THEN applies the page's LIMIT — the scan itself
  never early-terminates (confirmed by `cursor_handlers.rs:533-545`'s own
  doc comment: "the `LIMIT 1` here does NOT make the underlying scan
  early-terminate").

**A DIFFERENT, already-working fast path exists — but is structurally
unreachable from the cursor path:**
- `read_planner.rs:472-522`'s `try_plan_keyset_seek` — for a ONE-SHOT
  `Temporal::Latest` query (never `AsOf`) with a SINGLE-column ORDER BY
  covered by an existing sorted index, it calls
  `crates/shamir-index/src/legacy/sorted_index_manager.rs:791-909`'s
  `lookup_range_first_k_page(seek_encoded, after_key, k, forward)` — a REAL,
  already-implemented range-seek-and-resume primitive: seeks to an encoded
  key, optionally resumes from a prior `after_key` (the full physical index
  key from the last row of the previous page), returns `(Vec<RecordId>,
  Option<Bytes>)` (ids + an opaque resume bookmark), supports both ASC/DESC.
- **The gap**: `Temporal::AsOf` never reaches this — `read_as_of` always
  does the full unindexed scan, regardless of whether a sorted index exists
  on the ORDER BY column. Multi-column ORDER BY / computed expressions are
  ALSO excluded even from the Latest-path fast route
  (`read_planner.rs:477-479` rejects `order_by.items.len() != 1`) — so ANY
  fix here inherits the same narrow single-column-indexed scope; broader
  ORDER BY shapes keep the existing full-rescan fallback, honestly.

**A promising, NOT-YET-CONFIRMED lead — investigate this FIRST, it gates
everything else:** the sorted index's posting entries for a "covering
projection" already carry an explicit version stamp — see
`sorted_index_manager.rs:1242-1247`'s doc comment: "the returned bytes are
a versioned projection envelope... `[8 bytes: version as u64 little-endian]
++ [msgpack: Vec<(String, QueryValue)>]`... `version` should be the MVCC
write version for the record" (also `:1308-1325`, `decode`). **This spike's
first job is determining whether this per-posting version stamp (or
something else in the index/MVCC layer) can be used to correctly exclude
postings for records committed AFTER the cursor's pinned snapshot version**
— i.e. whether `lookup_range_first_k_page` (or a variant of it) can be made
AsOf-aware by checking each candidate posting's version against
`pinned_version` before including it in the page, falling back to a
subsequent seek-continuation when a candidate is filtered out (post-pinned
writes must not silently shrink a page below its requested size without
resuming the seek). Read `crates/shamir-tx`'s MVCC store version model in
full before assuming this works or doesn't.

## What to settle

### 1. Can index-seek be made AsOf-aware at all?

Investigate the version-stamp lead above. Determine: does the sorted
index's covering-projection version field (or an alternative mechanism —
e.g. checking the underlying MVCC store's `version_of(record_id)` for each
candidate RecordId the index seek yields, filtering out anything committed
after `pinned_version`) let a seek-based page correctly respect pinned-
snapshot isolation? Prove or disprove this with a real test — this is the
single most important question, mirroring how F-50 Step 3a's spike proved
(not assumed) the bincode forward-compat mechanism.

### 2. Where does the fix live?

`read_as_of` (`read_temporal.rs`) may have callers beyond cursors —
investigate (grep `read_as_of` call sites) before deciding whether an
AsOf-aware seek path belongs there (benefiting every AsOf caller) or as a
cursor-specific mechanism inside `cursor_handlers.rs` (narrower blast
radius, but duplicates seek logic already living in the planner). State
your reasoning.

### 3. Scope: single-column indexed ORDER BY only, matching `try_plan_keyset_seek`'s existing restriction

Confirm this is the right Step 2 boundary — a cursor with a single-column
ORDER BY covered by an existing sorted index gets the seek-based fix;
every other shape (no ORDER BY / offset mode, multi-column ORDER BY,
unindexed ORDER BY, computed expressions) keeps the existing, honestly-
documented full-rescan behavior. State whether investigation reveals this
should be narrower or could reasonably be wider.

## What to prototype

Prove the mechanism for the narrowest real case: a single-column indexed
ORDER BY cursor, 3+ pages deep, with a WRITE committed to the table
AFTER `CreateCursor` but BEFORE a later `FetchNext` (the concurrency case
that tests the AsOf-awareness). Deterministic test (no need for a pause
seam — sequential steps: create cursor, fetch page 1, commit a new row,
fetch page 2, assert the new row does NOT appear and the page is still
correct/complete).

1. **Red proof**: demonstrate the CURRENT full-rescan-per-page behavior
   (correct results, but confirm — e.g. via a scan-count instrumentation
   or a benchmark — that page N really does cost O(N × table) work today).
2. **Green proof**: your settled index-seek + pinned-version-check
   mechanism produces IDENTICAL page contents to the full-rescan approach,
   for at least 3 pages deep, while doing meaningfully less work (measure
   it — record count scanned, or a benchmark, not just "looks faster").

## Deliverable

A decision memo at `docs/dev-artifacts/research/f53b-cursor-seek-spike.md`
(mirroring `f50-index-lifecycle-spike.md`'s structure): whether AsOf-aware
index-seek is possible and how (§1), where the fix lives (§2), the
Step 2 scope boundary (§3), the prototype's red→green proof with actual
numbers, and a precise Step 2 implementation plan with exact touch points.

## Constraints

- Timebox this — if the version-stamp lead turns out not to work (e.g. the
  covering-projection version field isn't actually populated/reliable for
  all index kinds, or MVCC's `version_of` doesn't compose cleanly with the
  index seek), STOP, document precisely what you found and why it's hard,
  and let the memo record a negative result — that is still a valuable,
  honest spike outcome (matching F-50 Step 3a's precedent of proving a
  mechanism does NOT work before landing the real one).
- Do NOT implement the full Step 2 (all pagination modes, full test
  coverage) — prototype ONE case only, per "What to prototype" above.
- Do NOT weaken the cursor's snapshot-isolation guarantee — a resumed page
  must never observe a write committed after `CreateCursor`.
- Do NOT touch F-53a's landed streaming top-K code
  (`crates/shamir-engine/src/query/read/order.rs`'s `TopKHeap`,
  `read_exec.rs`/`read_temporal.rs`'s inline-heap paths) — unrelated,
  already closed.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-server -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean if
  any prototype code is committed.
- Clean up any scratch/debug log files you create in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-server -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-server --full
```

When done, give your final summary as plain text: whether AsOf-aware
index-seek is possible (with the actual proof/disproof), where you'd wire
the fix and why, the Step 2 scope boundary, the prototype's red→green proof
with actual numbers, the memo's implementation plan, and confirmation
fmt/clippy are clean if you committed prototype code.
