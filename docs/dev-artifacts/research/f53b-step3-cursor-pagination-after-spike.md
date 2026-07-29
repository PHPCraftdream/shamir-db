# F-53b Step 3 — can `cursor_handlers.rs` reach the AsOf keyset-seek arm, and how? (#879, P1)

**Status:** spike complete. **The brief's blocking finding is CONFIRMED (a
caller `WHERE` clause permanently excludes a cursor from this path). A
narrower, real mechanism IS proven reachable for the no-`WHERE` case, and its
mandatory safety net (silent-fallback detection) is proven with a real
negative test.** Recommendation below is scoped for a follow-up implement
task (Step 4), not landed here.

This spike does not re-litigate F-53b Step 1/2's findings (the AsOf-aware
seek arm itself, `read_as_of_keyset_seek`, and its per-index mutation
high-water gate) — see `f53b-cursor-seek-spike.md` for those. It answers the
ONE question Step 2 deferred: can `cursor_handlers.rs`'s `FetchNext` actually
be converted to send `Pagination::After` and reach that arm, and if so, what
does the cursor-side bookkeeping (`PaginationMode`, `CursorState`, fallback)
need to look like to do it safely?

---

## 1. What this spike settled

### 1.1 The blocking finding — CONFIRMED, not fixed (out of scope, per the brief)

`TableManager::try_plan_keyset_seek` (`read_planner.rs:472-525`) — shared by
both the pre-existing `Temporal::Latest` fast path and the new AsOf arm —
hard-requires:

```rust
if query.r#where.is_some()
    || query.group_by.is_some()
    || query.select.distinct
    || query.count_total
    || exec::has_aggregates(&query.select)
{
    return None;
}
```

`cursor_handlers.rs`'s CURRENT (`Keyset`-mode) bookmark scheme ALWAYS
AND-combines an inclusive `Gte`/`Lte` boundary into `query.r#where`
(`boundary_filter`, `cursor_handlers.rs:377-403`). So even if
`cursor_handlers.rs` started emitting `Pagination::After` for every keyset
page, `try_plan_keyset_seek` would see `where.is_some()` (the boundary
filter) and return `None` on literally every call past page 1 — falling
through to the OLD full-scan/inline-top-K tails in `read_as_of`, which (per
the brief) resolve `Pagination::After` via `Pagination::resolve()`'s
unconditional `(skip=0, take=limit)` mapping — i.e. PAGE ONE FOREVER. This
was verified again in this spike (not merely re-read from the brief): the
new test's `probe_1_eligibility_probe_uses_only_public_api` explicitly
proves ANY caller `WHERE` disqualifies the eligibility probe, including a
trivially-true one — the guard has no notion of "harmless" vs "restrictive"
`WHERE`, it excludes all of them uniformly.

**This spike does NOT attempt to fix this.** Extending
`try_plan_keyset_seek` (and the underlying `lookup_range_first_k_page`
seek primitive, which has no WHERE-residual-filter concept at all) to accept
an arbitrary `WHERE` alongside the seek is a materially larger change — it
would need a residual-filter evaluation step interleaved with the ordered
index walk (skip candidates that fail the residual, keep asking the index
for more until `limit` residual-passing rows accumulate), a new code path
neither `read_keyset_seek` nor `read_as_of_keyset_seek` has today. Per the
brief, this is explicitly out of scope for Step 3.

**Consequence for scope, restated precisely:** a cursor can use the new
`IndexSeek` bookmark ONLY when the CALLER's ORIGINAL query — the one passed
to `CreateCursor`, before `cursor_handlers.rs` adds anything — has:

1. `query.r#where.is_none()` (not "no residual filter after removing the
   boundary" — literally none at all, checked at `create_cursor` time before
   any bookmark exists),
2. a single, simple (top-level-field) ORDER BY column,
3. a sorted index covering that column,
4. (checked per-`FetchNext`, not at `create_cursor` time) the F-53b Step 2
   gate — `sorted_indexes().last_mutation_version() <= pinned` — still
   holding.

Every cursor with a caller `WHERE` clause keeps TODAY's exact `Keyset`/
`Offset` scheme, completely unchanged. This is a NEW THIRD mode
(`IndexSeek`), not a replacement for `Keyset`.

### 1.2 The eligibility probe — PROVEN buildable from already-`pub` API, no new `TableManager` method needed

**Decision: `create_cursor` (or a small new free function beside
`pagination_mode_for_query` in `cursor_handlers.rs`) can decide `IndexSeek`
eligibility using ONLY methods that are ALREADY `pub` today. No new `pub`
method needs to be added to `TableManager` or `SortedIndexManager` for the
eligibility check itself.**

Verified building blocks, all already reachable from `shamir-server` (which
only depends on `shamir_db`, which does `pub use shamir_engine as engine;
pub use shamir_engine::query;` — a blanket re-export, confirmed by reading
`crates/shamir-db/src/lib.rs:21-22`):

- `TableManager::sorted_indexes(&self) -> &SortedIndexManager`
  (`table_manager.rs:715`, `pub fn`).
- `SortedIndexManager::find_by_field(&self, field_path: &[u64]) ->
  Option<SortedIndexDefinition>` (`sorted_index_manager.rs:240`, `pub fn`).
- `shamir_engine::query::filter::resolve::intern_field_path(field: &[String],
  interner: &Interner) -> Option<Vec<u64>>` (`resolve.rs:50`, `pub fn`, in a
  `pub mod resolve` under a `pub mod filter` under `pub mod query` —
  confirmed via `crates/shamir-engine/src/query/mod.rs` and
  `crates/shamir-engine/src/query/filter/mod.rs`, both declare their
  submodules `pub`).
- `table.interner().get()` — already called by `cursor_handlers.rs`'s
  existing `build_filter_context`/`order_by_column_is_schema_typed_scalar`.

This spike's `probe_index_seek_eligible` (new test file
`crates/shamir-engine/src/table/tests/f53b_step3_cursor_after_spike.rs`)
implements exactly this — a ~15-line function using only the above,
deliberately NOT delegating to `try_plan_keyset_seek` itself (that method
ALSO checks `query.pagination.keyset()`, which does not exist yet at
`create_cursor` time — the cursor has no bookmark yet to check).
`probe_1_eligibility_probe_uses_only_public_api` proves it correctly:

- accepts a plain `ORDER BY <indexed field>` with no `WHERE`,
- rejects ANY `WHERE` (even a trivially-satisfiable one on the same field —
  proving the "no exceptions" restriction from §1.1 concretely, not just by
  reading the planner's guard),
- rejects an ORDER BY on an unindexed column,
- rejects a multi-column ORDER BY.

**Why this matters for the production wiring (Step 4):** `create_cursor`
already runs `pagination_mode_for_query` (shape-only) followed by
`order_by_column_is_schema_typed_scalar` (schema-typed gate) followed by
`order_by_column_contains_null` (data-safety probe) before committing to
`PaginationMode::Keyset`. The settled design (§2 below) adds
`probe_index_seek_eligible` as a **new, EARLIER branch** ahead of that
existing chain: `IndexSeek` is attempted first (cheaper — no null probe
needed, see §2.3), and only queries that fail `IndexSeek` eligibility fall
through to the existing `Keyset`/`Offset` decision chain, which is
completely unchanged.

### 1.3 The `CursorState` bookmark — a real `RecordId`, confirmed on this task's own harness

**Decision: `IndexSeek`-mode `CursorState` needs exactly ONE new bookmark
field — `after_id: Option<RecordId>` (plus the already-existing `seek_key:
Option<QueryValue>` field, reused) — no `tie_skip` counter analogue needed.**

F-53b Step 2's `read_as_of_keyset_seek` (`read_asof_seek.rs:187-202`)
attaches a real `RecordId` to every output row:

```rust
let records: Vec<QueryRecord> = matched
    .iter()
    .map(|(id, _)| *id)
    .zip(result_qv)
    .map(|(id, fields)| {
        QueryRecord::Inserted(shamir_query_types::write::InsertedRecord {
            id: Some(id),
            fields,
        })
    })
    .collect();
```

This spike's `probe_2_seek_arm_attaches_real_record_id` test re-confirms
this concretely (not just by re-reading Step 2's source): every row of a
production `read()` call that takes the `_asof_keyset` path carries
`QueryRecord::Inserted { id: Some(id), .. }` with `id` being one of the
actually-inserted `RecordId`s. This is precisely what
`Pagination::after_with_id(key, limit, after_id)` (`limit.rs:245-255`,
already used by `try_plan_keyset_seek`'s own Latest-temporal callers) wants.

Contrast with why `Keyset` mode needs `tie_skip` at all: the GENERIC AsOf
full-scan/inline-top-K tails (`read_as_of`'s `apply_select_value_bytes` /
`try_project_page_only_bytes` paths, and F-53a's `TopKHeap`) emit
`QueryRecord::Direct` — no id, ever (confirmed again by this spike's own
harness: `probe_2`'s baseline-style full-scan call, if run without
`Pagination::After`, produces `QueryRecord::Direct` rows with no `_id`,
matching `cursor_handlers.rs`'s existing CR-A4 module-doc claim). `tie_skip`
exists ONLY as a substitute for a missing real id. `IndexSeek` mode doesn't
need that substitute because its OWN read path (the seek arm) always
attaches the real id — so its bookmark is simpler than `Keyset`'s, not just
different.

### 1.4 The silent-fallback hazard — PROVEN with a real test, and its fix identified

**Decision: `stats.index_used` IS a reliable, cheap, per-call signal for
"did the fast arm actually fire this call", and the CR-D1 `StuckAtCeiling
-> permanent Offset` pattern (already precedented in `cursor_handlers.rs`)
is the correct, safe recovery — reused as-is, not reinvented.**

This is the load-bearing finding of this spike. The new test
`negative_gate_failure_mid_lifetime_must_not_page_one_forever` runs exactly
the scenario the brief demands:

1. 30 rows (scores `0..290`), pin the snapshot, fetch page 1 via
   `Pagination::After` — the seek arm fires
   (`index_used == "sorted_idx_<n>_asof_keyset"`), returns `[0..90]` plus a
   real `after_id`.
2. A concurrent `UPDATE` to the indexed field on an unrelated row (`200 ->
   205`) lands strictly between page 1 and page 2's `FetchNext` — "an
   intervening write" per the brief, exercising the SAME
   `last_mutation_version` high-water gate F-53b Step 2 already built (this
   spike does not touch that gate; it only proves what happens on the
   CURSOR side once it trips).
3. Page 2 is fetched with the SAME kind of call a naive "always send
   `Pagination::After`" `fetch_next` would make (same seek key + same
   `after_id`, same pinned version).
4. **Result, confirmed by the test:**
   - `index_used` no longer ends in `_asof_keyset` — the gate declined, the
     read fell through to `read_as_of`'s full-scan tail. This confirms
     `index_used` is observable and correct as a per-call detector.
   - **The hazard, confirmed concretely (not assumed):** the full-scan
     fallback ALSO has no seek-key boundary in `query.r#where` (there never
     was one — `IndexSeek` mode, unlike `Keyset` mode, carries NO boundary
     filter at all; the seek key lives only in `Pagination::After`, which
     the full-scan tail ignores via `Pagination::resolve()`'s `(skip=0,
     take=limit)` mapping). So the "naive" page 2 call reproduces **page 1
     exactly** (`[0..90]` again) — not an error, not a crash, not obviously
     wrong-looking data (it IS a valid page of the table, just the WRONG
     one) — a genuinely silent hazard a client has no local way to detect.
   - **The fix, prototyped and proven correct:** the SAME
     `LimitOffset { offset: <rows returned so far>, limit }` bookmark
     CR-D1's `StuckAtCeiling` fallback already uses. The test builds this
     fallback query manually (offset = 10, the row count from page 1) and
     confirms it returns the TRUE page 2 (`[100..190]`), with the moved
     row's NEW value (205) absent (the pinned snapshot still shows it as
     `200`, correctly deferred to its own page).

**Why this reuses CR-D1 exactly, per the brief's instruction not to invent a
new pattern:** `CursorState.offset` is ALREADY maintained in parallel
today, independent of `mode` (`cursor_handlers.rs`'s existing
`new_offset = state.offset + outcome.result.records.len() as u64` on the
`Keyset` branch, mirrored on `Offset`). Extending this to also track on a
THIRD (`IndexSeek`) mode branch costs nothing new — it is already updated by
every `FetchNext` regardless of which mode produced the page, exactly
the property CR-D1's doc comment cites as what makes its own fallback safe
("`CursorState::offset` is maintained in parallel on the Keyset branch the
whole time ... it already reflects the true count of rows returned so far,
independent of which mode produced them").

### 1.5 Value-proposition sanity check — honest read, not a blocking gate

**The permanent no-`WHERE` restriction (§1.1) means `IndexSeek` covers a
narrow, specific cursor shape: "give me everything in this table (or an
index2/legacy-unfiltered view of it), ordered by one indexed column, with no
filter at all." Realistic assessment, not over-claimed:**

- **This is a MINORITY of real cursor usage.** Any cursor over "recent
  orders for customer X", "active users in region Y", "records where
  status = pending" — i.e. essentially any cursor a real application opens
  to page through a FILTERED view — has a `WHERE` clause and can NEVER use
  `IndexSeek`, no matter how the pagination bookmark is shaped. Those
  cursors keep paying `O(page_size × pages)` work on the ALREADY-fixed-cost
  side (CR-A4's `Keyset` mode is itself `O(page_size)`-ish per page relative
  to `tie_skip`, not `O(N)`) but never see the seek arm's `O(page_size)`
  index-walk win over the STILL-`O(N)`-per-page full-scan baseline
  `read_as_of` pays for `Keyset`/`Offset` mode's own reads (see the F-53b
  Step 1 spike's own red/green numbers: `records_scanned ≈ N` on EVERY
  `Keyset`-mode page today, regardless of `WHERE`).
- **What DOES benefit, concretely:** "browse/export the whole table sorted
  by one column" cursors — admin/reporting UIs paging an unfiltered listing
  ("all products by price", "all users by signup date"), bulk-export
  tooling, and any cursor whose caller happens to apply post-hoc filtering
  client-side rather than server-side `WHERE`. This is a real, legitimate
  pattern, but it is the MINORITY case for a typical multi-tenant /
  filtered-query application; the majority "page through MY subset of the
  data" shape is exactly what a `WHERE` clause exists to express, and that
  shape is permanently excluded.
- **Framing for Step 4's prioritization:** `IndexSeek` is a real, provable
  `O(N) -> O(page_size)` win for the shape it covers, and the mechanism
  (probe, bookmark, fallback) is now fully proven safe to build. But Step 4
  should NOT be scoped or marketed as "fixes cursor pagination performance"
  broadly — it fixes ONE narrow (if real) shape. The bulk of the `O(N)`
  full-rescan cost documented in F-53b Step 1's red-path measurement
  (`records_scanned ≈ N` per `Keyset`/`Offset` page) remains UNCHANGED for
  every filtered cursor after Step 4 lands. Any future work to shrink that
  cost for the WHERE-bearing majority would need the residual-filter
  extension to the seek machinery explicitly ruled out in §1.1 — a
  materially larger, separate task, not a corollary of Step 4.

---

## 2. Settled design for Step 4 (NOT implemented in this spike)

### 2.1 `PaginationMode::IndexSeek` — new third variant

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaginationMode {
    /// (unchanged) Boundary-filter seek — CR-A4's inclusive-Gte/Lte +
    /// tie_skip scheme. Used whenever the query has a simple ORDER BY but
    /// is NOT IndexSeek-eligible (has a WHERE, no covering sorted index,
    /// etc).
    Keyset,
    /// (unchanged) Plain row-count offset — no ORDER BY at all.
    Offset,
    /// NEW (Step 4): sorted-index keyset seek via Pagination::After,
    /// reaching TableManager::read_as_of_keyset_seek directly. Decided
    /// ONCE at create_cursor time (same CR-A4 discipline) by
    /// probe_index_seek_eligible: query.where.is_none() AND a single
    /// simple-field ORDER BY AND a sorted index covers it. Falls back
    /// PERMANENTLY to Offset (see IndexSeekOutcome below) the first time a
    /// FetchNext's stats.index_used does not end in "_asof_keyset" —
    /// mirrors CR-D1's StuckAtCeiling -> Offset transition exactly.
    IndexSeek,
}
```

Eligibility order in `create_cursor` (probe order matters — cheapest/most
restrictive first):

1. `probe_index_seek_eligible` (§1.2) — no I/O beyond the interner lookup
   already needed elsewhere. If `Some(index_name)`, mode = `IndexSeek`,
   SKIP `order_by_column_is_schema_typed_scalar` +
   `order_by_column_contains_null` entirely (§2.3 — the seek arm's own
   per-candidate MVCC classifier already handles data-shape edge cases the
   `Keyset` boundary-filter scheme cannot, so those two `Keyset`-specific
   safety probes are unnecessary work for `IndexSeek`).
2. Else, fall through to TODAY's existing chain unchanged:
   `pagination_mode_for_query` -> schema-typed gate -> null probe ->
   `Keyset` or `Offset`.

### 2.2 `CursorState` field additions

```rust
pub struct CursorState {
    // ...existing fields unchanged (query, mode, seek_key, tie_skip, offset, exhausted)...

    /// NEW (Step 4): the last row's RecordId, for IndexSeek mode's
    /// Pagination::after_with_id tie-breaker. `None` before the first
    /// FetchNext (nothing to seek past yet) and whenever `mode !=
    /// IndexSeek`. Reuses the EXISTING `seek_key: Option<QueryValue>`
    /// field for the ORDER BY value half of the bookmark (no new field
    /// needed there) — only the id half is new, because IndexSeek is the
    /// ONLY mode whose read path attaches a real RecordId (§1.3).
    pub after_id: Option<RecordId>,
}
```

`tie_skip` stays `0`/unused for `IndexSeek`-mode cursors (it is
`Keyset`-mode-specific machinery, not repurposed).

### 2.3 `FetchNext` dispatch — new `IndexSeek` arm + the fallback

```rust
match (state.mode, state.seek_key.clone()) {
    (PaginationMode::IndexSeek, seek_key_opt) => {
        let mut q = base_query.clone();
        q.pagination = Pagination::after_with_id(
            seek_key_opt.map(|v| vec![v]).unwrap_or_default(),
            Some(effective_page_size as u64),
            state.after_id,
        );
        q.temporal = Temporal::AsOf { at: At::Version(cursor.pinned_version()) };
        let result = table.read_with_encoding(&q, &ctx, Default::default()).await?;

        let took_seek_arm = result.stats.as_ref()
            .and_then(|s| s.index_used.as_deref())
            .is_some_and(|l| l.ends_with("_asof_keyset"));

        if took_seek_arm {
            // normal path: extract next (seek_key, after_id) from the
            // last row, has_more from records.len() vs page_size (peek
            // row convention, same as today), advance state.offset in
            // parallel (CR-D1 precondition for the fallback below).
        } else {
            // CR-D1-style ONE-TIME transition: force_offset_mode = true,
            // re-run THIS call via fetch_offset_page(state.offset, ...)
            // so the CALLER still gets a correct page back (not an error,
            // not the stale seek-mode page) — mirrors StuckAtCeiling's own
            // "re-run this same call via the offset bookmark" recovery
            // exactly (cursor_handlers.rs:1684-1712 today).
        }
    }
    (PaginationMode::Keyset, Some(seek_key)) => { /* unchanged */ }
    _ => { /* unchanged Offset path */ }
}
```

The `force_offset_mode` commit-after-budget-gate ordering (today's existing
`if force_offset_mode { state.mode = PaginationMode::Offset; }`, gated
behind `enforce_page_budget` clearing first) applies identically — no new
hazard introduced by adding a second trigger for the same flag.

### 2.4 Tests Step 4 must add (production-path, not test-local)

- Port `probe_1`/`probe_2`/the negative test from this spike's test-local
  harness to actually drive `cursor_handlers::create_cursor` /
  `fetch_next` (mirrors how F-53b Step 2 ported Step 1's test-local spike
  into `f53b_asof_seek_tests.rs` against the real `read()` path).
- A `cursor_handler_tests.rs` case: `CreateCursor` (no WHERE, indexed
  ORDER BY) -> `FetchNext` page 1 (assert `IndexSeek` mode pinned, seek arm
  fired) -> concurrent write -> `FetchNext` page 2 (assert transparent
  fallback to `Offset`, correct data, `mode` now permanently `Offset`) ->
  `FetchNext` page 3 (assert it stays on `Offset`, no attempt to re-probe
  `IndexSeek`).
- DESC direction parity (the seek arm already supports it; the cursor
  wiring should too).

### 2.5 Explicitly OUT of scope for Step 4 (same restrictions this spike found)

- Any `WHERE`-clause support for `IndexSeek` — permanently blocked by
  `try_plan_keyset_seek`'s shared guard (§1.1); not a Step 4 task.
- Multi-column ORDER BY — no composite seek primitive exists.
- Changing `Keyset`/`Offset` mode behavior in any way — both stay byte-for-
  byte as they are today; `IndexSeek` is purely additive.

---

## 3. Prototype artifacts (committed alongside this memo)

- **`crates/shamir-engine/src/table/tests/f53b_step3_cursor_after_spike.rs`**
  (new) — three tests, all against the PRODUCTION `read()` path (no
  test-local reimplementation of the seek itself — Step 2 already landed
  and proved that; this spike only prototypes the CURSOR-side mechanism
  around it):
  1. `probe_1_eligibility_probe_uses_only_public_api` — the `create_cursor`-
     time eligibility probe, built from already-`pub` API.
  2. `probe_2_seek_arm_attaches_real_record_id` — confirms the seek arm's
     `RecordId` attachment (the `CursorState` bookmark design basis).
  3. `negative_gate_failure_mid_lifetime_must_not_page_one_forever` — THE
     load-bearing negative test: proves the silent page-one-forever hazard
     concretely, proves `stats.index_used` detects it reliably, and proves
     the CR-D1-precedented offset-bookmark recovery is correct (right data,
     no duplication, no loss).
- **`crates/shamir-engine/src/table/tests/mod.rs`** — one new `pub mod`
  line.

No production code (`cursor_handlers.rs`, `cursor_registry.rs`,
`read_planner.rs`, `read_asof_seek.rs`, `read_temporal.rs`) was touched.
`try_plan_keyset_seek`'s `where.is_some()` guard was read and exercised
(via the probe test), never modified.

---

## 4. Verification run

```
cargo fmt -p shamir-engine -p shamir-server -- --check     # exit 0
cargo clippy --workspace --all-targets -- -D warnings        # exit 0
./scripts/test.sh -p shamir-engine -- f53b_step3_cursor_after_spike
# 3 tests run: 3 passed, 1754 skipped, exit=0
```

The orchestrator should additionally run the full
`./scripts/test.sh -p shamir-engine -p shamir-server --full` scope to
confirm no broader regression — this spike touches only a new test module +
the test manifest line, so none is expected.

---

## 5. Decision summary

| Question | Decision |
|---|---|
| Can `cursor_handlers.rs` reach the AsOf seek arm for EVERY cursor? | **NO, permanently.** Any caller `WHERE` clause excludes it — confirmed by both re-reading and re-exercising `try_plan_keyset_seek`'s shared guard. Out of scope to fix. |
| Can it reach the arm for a NARROWER shape (no WHERE, single indexed ORDER BY)? | **YES.** A new `PaginationMode::IndexSeek`, decided once at `create_cursor` via a probe built entirely from already-`pub` API — no new `TableManager`/`SortedIndexManager` method required. |
| What bookmark does `IndexSeek` need? | `seek_key: Option<QueryValue>` (reused from `Keyset`) + a NEW `after_id: Option<RecordId>` field — simpler than `Keyset`'s `tie_skip` counting hack, because the seek arm attaches a real `RecordId` (confirmed on this task's own harness, not just re-cited from Step 2). |
| Is a silent per-call fallback failure possible, and can it be detected? | **YES it's possible (proven with a real test — a naive always-`After` scheme silently re-returns page 1 when the gate declines mid-lifetime), and YES it's detectable** (`stats.index_used` not ending in `_asof_keyset`). The fix reuses CR-D1's exact `StuckAtCeiling -> permanent Offset` pattern — no new mechanism invented, per the brief's instruction. |
| How much real cursor usage does this actually help? | **A narrow, real slice** — unfiltered "browse the whole table sorted by one column" cursors. Any cursor with a `WHERE` (the majority of realistic multi-tenant/filtered application usage) is permanently excluded and keeps paying today's `O(N)`-per-page `Keyset`/`Offset` cost. Step 4 should be scoped and communicated as closing ONE specific gap, not the general cursor-performance problem. |
