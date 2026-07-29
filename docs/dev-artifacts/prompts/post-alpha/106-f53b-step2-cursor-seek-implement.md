# Brief for F-53b Step 2 (#878, P1, implement) — land the AsOf-aware cursor index-seek

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

**Read `docs/dev-artifacts/research/f53b-cursor-seek-spike.md` in full
first** — Step 1's decision memo (commit `bf3f37a2`). It proved the
mechanism with a test-local prototype; this step wires it into the
production `read_as_of` path. Do not re-derive the design — implement
what the memo settled.

**The four settled decisions this step implements (do not re-litigate):**

1. **The mechanism**: `MvccStore::version_of(record_id)` classifies each
   sorted-index seek candidate against the cursor's pinned version.
   `version_of(id) != 0 && <= pinned` ⇒ the record's current value equals
   its pinned-snapshot value (MVCC immutability), so the index posting's
   position is trustworthy — include it via `get_at(id, pinned)`. A write
   landed after the pin + `get_at == None` ⇒ INSERT-after-pin, exclude. A
   write landed after the pin + `get_at == Some` ⇒ the pinned-version
   posting was MOVED or REMOVED by a concurrent UPDATE/DELETE — the index
   cannot place it correctly.
2. **The mandatory gate**: because case 3 above is a genuine, irreducible
   miss (proven by the spike's two negative tests), the fast path is ONLY
   safe when NO concurrent write could have moved/removed a pinned
   posting. This requires a NEW per-index "last mutation version"
   high-water — this step's first and most load-bearing addition.
3. **Where it lives**: inside/dispatched from `read_as_of`
   (`read_temporal.rs:45`) — confirmed to have exactly ONE caller
   (`read_exec.rs:297`, the `Temporal::AsOf` arm), so this benefits every
   AsOf caller (cursor `FetchNext` and any direct point-in-time query)
   with zero duplication.
4. **Scope**: single-column indexed ORDER BY, ASC/DESC, keyset-eligible
   (matching `try_plan_keyset_seek`'s existing restriction exactly).
   Everything else (no ORDER BY / offset mode, multi-column, unindexed,
   computed expressions) keeps the existing full-rescan `read_as_of` path
   unchanged.

## What to implement (memo §5)

### 5.1 The per-index mutation high-water gate (FIRST — load-bearing for safety)

Add to `SortedIndexManager` (`crates/shamir-index/src/legacy/sorted_index_manager.rs`):
- New field `last_mutation_version: AtomicU64` (per-manager granularity is
  sufficient — a false-negative gate check only costs a fallback to the
  already-correct full scan, never a correctness bug).
- `fetch_max(AcqRel, version)` on every `on_record_created` /
  `on_record_updated` / `on_record_deleted` — bumped at APPLY (commit)
  time with the commit's MVCC version, NOT at tx-stage time. **This
  ordering is load-bearing**: sorted-index ops are staged into
  `tx.index_write_set` at STAGE time (`table_manager_tx_ops.rs:239`) and
  applied at commit; bumping the high-water at stage time would let an
  uncommitted (possibly-aborting) tx disable the fast path for unrelated
  cursors. Wire the `fetch_max` at `commit.rs`'s Phase 5c
  (`IndexPut`/`IndexDel` replay), alongside each posting apply.
- New accessor `pub fn last_mutation_version(&self) -> u64` (Acquire
  load).

### 5.2 The AsOf seek arm in `read_as_of`

- New `pub(super) async fn read_as_of_keyset_seek`, in a sibling file
  `read_asof_seek.rs` (mirroring `read_index_scan.rs::read_keyset_seek`'s
  shape): resolve eligibility (single-column indexed ORDER BY + keyset
  pagination — reuse/adapt `try_plan_keyset_seek`'s eligibility logic,
  `read_planner.rs:472-522`, so it also accepts the AsOf temporal), check
  the §5.1 gate (`sorted_indexes().last_mutation_version() <= pinned`),
  then run the spike-proven loop (`lookup_range_first_k_page` +
  the `version_of`/`get_at` classifier).
- Wire it as the FIRST branch in `read_as_of`, before the inline-top-K
  short-circuit (F-53a, already landed — leave that code alone) and the
  full-scan tail. Gated on eligibility + the high-water check. If the
  classifier signals `concurrent_modified > 0` (should be impossible
  under a correctly-implemented gate, but keep as defence-in-depth), fall
  back to the full scan for that page rather than returning a wrong
  result.
- **Cursor shape conversion**: `crates/shamir-server/src/db_handler/cursor_handlers.rs`
  currently pages via a WHERE boundary-filter (`field >= seek_key`) +
  OFFSET `tie_skip` (`:363-403`), NOT `Pagination::After` — the shape
  `try_plan_keyset_seek` requires. Convert the cursor's keyset-mode pages
  to emit `Pagination::After` when keyset-eligible (it already tracks
  `seek_key` + `tie_skip` + the last `RecordId`, which map cleanly to
  `after { key, limit }` + `after_id`), so the SAME new seek arm serves
  both one-shot AsOf queries and cursor `FetchNext` pages. This is the
  memo's recommended option (a) over duplicating the boundary-filter
  shape into a second seek arm.

### 5.3 Tests

- Port the spike's 4 test-local tests
  (`f53b_cursor_seek_spike_tests.rs`) to exercise the PRODUCTION `read()`
  path (AsOf + `Pagination::After`) instead of the test-local
  `asof_index_seek_spike` helper — assert the `QueryStats::index_used`
  label switches to the seek path and `records_scanned` drops to
  `O(page_size)`, not just that the results match.
- A gate-fallback test: a concurrent UPDATE to the indexed field after
  the cursor's pin → assert the seek arm DECLINES (high-water advanced
  past pinned) and the result equals the existing full-scan baseline
  (no miss — this is the test that proves the gate actually prevents the
  spike's negative-test failure modes from ever reaching a real cursor).
- A cursor-level integration test in
  `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`:
  `CreateCursor` → `FetchNext` page 1 → concurrent insert (and separately,
  in another test, a concurrent UPDATE to the indexed field) → `FetchNext`
  page 2 asserts correctness (insert absent; update-case falls back
  correctly) AND that per-page server work dropped (via the `QueryStats`
  the cursor already surfaces in its response).
- DESC direction parity (mirror the spike's ASC-only prototype).

### 5.4 Documentation

Update `docs/guide-docs/KNOWN_LIMITATIONS.md:402-405` once this lands:
narrow the "full pinned-version scan per page" claim to apply only to
non-keyset-eligible cursors OR cursors where a concurrent write advanced
the gate past the pin — do not claim the limitation is fully closed for
every cursor shape, since it explicitly is NOT (multi-column, unindexed,
computed-expression ORDER BY, and gate-tripped cursors all still pay the
full-rescan cost, correctly).

## What NOT to do (explicitly out of scope per the memo §5.4)

- Multi-column / computed-expression ORDER BY (no composite seek
  primitive exists; this would need a much larger separate slice).
- A versioned/temporal index (the alternative mechanism that could avoid
  needing the high-water gate entirely — explicitly a "later performance
  slice" per `read_temporal.rs`'s own doc comment; the gate makes it
  unnecessary for the cursor use case).
- Any interaction with the covering-index `INCLUDES` version stamp — the
  memo proved (§1.1) this doesn't apply to the target non-covering index
  shape; the mechanism is `version_of`, not the stamp.
- Touching F-53a's landed streaming top-K code
  (`crates/shamir-engine/src/query/read/order.rs`'s `TopKHeap`, the
  inline-heap branches in `read_exec.rs`/`read_temporal.rs`) — unrelated,
  already closed, and this step's new seek arm should be a SIBLING
  branch to that one inside `read_as_of`, not a replacement.
- Touching F-53c's landed FK index-fast-path work — unrelated crate area.
- Weakening the cursor's snapshot-isolation guarantee in any way — a
  resumed page must NEVER observe a write committed after `CreateCursor`.
  The gate is what makes this safe; do not relax it for convenience.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-index -p shamir-server -- --check`
  and `cargo clippy --workspace --all-targets -- -D warnings` must be
  clean.
- This is a genuinely large task (a new durable-ish high-water primitive,
  a new read-path arm, a cursor-shape conversion, and a real concurrency
  test suite). Timebox it: if any single piece (especially the cursor
  `Pagination::After` conversion, which touches `cursor_handlers.rs`'s
  existing 1832-line, well-tested logic) proves substantially harder or
  riskier than expected, STOP, land what's solid (the gate + the seek arm
  for ONE-SHOT AsOf queries, even if the cursor conversion itself needs a
  further follow-up task), and document precisely what's deferred and
  why in your final summary. A partial, honestly-scoped landing is better
  than a risky rush through the cursor's own well-tested pagination logic.
- Clean up any scratch/debug log files you create in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-index -p shamir-server -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-index -p shamir-server --full
```

When done, give your final summary as plain text: exactly what you
landed vs. deferred (and why, if anything), the high-water gate design,
the seek arm's integration point in `read_as_of`, whether the cursor
shape conversion happened, test results (including the gate-fallback and
cursor-level concurrency tests' actual output), and confirmation
fmt/clippy are clean.
