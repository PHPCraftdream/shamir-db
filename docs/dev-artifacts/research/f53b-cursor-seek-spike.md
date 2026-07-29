# F-53b Step 1 — cursor index-seek design spike (#875, P1)

**Status:** spike complete. **The brief's primary lead is DISPROVEN for the
target case; a narrower, real mechanism is PROVEN.** Recommendation below.

The brief's single most important question — *can the sorted index's
per-posting version stamp make an index-seek AsOf-aware?* — is answered with a
real test, not an assumption:

- **The covering-projection version stamp does NOT apply to the target case.**
  The envelope `[8 bytes: version] ++ [msgpack]` is written **only** when the
  index `is_covering()` (i.e. it has `included_fields`/INCLUDES). A plain
  single-column `ORDER BY field` index — exactly the shape both the cursor and
  the existing `try_plan_keyset_seek` fast path target — is **non-covering** and
  stores `Bytes::new()` for every posting (`sorted_index_manager.rs:355-358`,
  verified in §1.1). There is no version stamp to read.
- **`MvccStore::version_of(record_id)` IS a real, cheap (in-memory `RecordCell`)
  per-candidate version probe that makes the seek AsOf-aware for the
  INSERT-after-pin case** — the concurrency case the brief's prototype targets.
  It is **PROVEN** (green test): a row inserted after the cursor's pin never
  appears in any later page, and pages are byte-identical to the production
  full-scan baseline across 3+ pages, at `O(page_size)` seek cost per page
  instead of `O(N)`.
- **It is NOT a complete snapshot-isolation solution by itself.** Two cases a
  current-state index fundamentally cannot serve are **DISPROVEN** (negative
  tests, §1.3): an UPDATE to the indexed field after the pin MOVES the posting
  (the record's pinned-version position no longer exists in the index), and a
  DELETE after the pin REMOVES the posting (the record id is never yielded at
  all). Both cause a silent MISS of a row the baseline correctly returns. A safe
  production path therefore **must gate on "no concurrent write touched this
  index since pin" and fall back to the existing full scan otherwise** — it
  cannot rely on per-candidate filtering alone.

So: AsOf-aware index-seek **is possible and worth doing for Step 2**, but only
behind a per-index mutation-version high-water gate (the existing full-rescan
`read_as_of` remains the fallback). The mechanism is `version_of` + `get_at`,
not the covering-projection stamp. Prototype code + tests are committed
alongside this memo as a clearly-scoped spike artifact (test-local; no
production read-path wiring — that is Step 2).

---

## 1. What this spike settled

### 1.1 Open question 1 — does the per-posting version stamp apply? (DISPROVEN for the target case)

**Decision: NO. The covering-projection version envelope is not present on the
index shape the cursor fast path targets. The mechanism must use
`MvccStore::version_of(record_id)` instead.**

**Investigation and reasoning (read before deciding, per the brief):**

`build_covering_projection` (`sorted_index_manager.rs:1249-1288`) writes the
`[8 bytes: version LE] ++ [msgpack]` envelope **only when `def.is_covering()`**
(`sorted_index_definition.rs:94` — true iff `included_fields_interned` is
non-empty). The three sorted-index plan entry points confirm it:

- `plan_record_created` (`:355-358`): `let value = if def.is_covering() {
  build_covering_projection(..) } else { Bytes::new() };`
- `plan_record_updated` (`:422-431`): same guard on both old/new projections.
- `plan_record_deleted` (`:460-477`): emits only a `RemovePosting` — no value at
  all.

A single-column `ORDER BY score` index created via
`create_sorted_index("score_idx", &["score"])` has NO `included_fields`, so
`is_covering()` is false and **every posting's value is `Bytes::new()`** — there
is no version stamp on the postings the seek would walk. Worse, even if the
index WERE covering, `lookup_range_first_k_page` (`:791-907`) returns only
`Vec<RecordId>` — it iterates `for (key, _) in b?` and **discards the value**
(the envelope) entirely. Using the stamp would require a new index primitive
that yields `(RecordId, version)` pairs. That is strictly more work than the
alternative below, which needs no index-layer change.

**The alternative the brief named — `MvccStore::version_of(record_id)` — works.**
`version_of` (`mvcc_store/mod.rs:1404`) is a public, in-memory `RecordCell`
probe (a single `cells.read_sync` — no I/O), returning the version at which the
record's CURRENT value was committed. It composes cleanly with the seek: for
each `RecordId` the index yields, one `version_of` call classifies the candidate
against the pinned snapshot (§1.2). This is the mechanism the prototype proves.

### 1.2 Open question 1 (cont.) — can `version_of` make the seek AsOf-aware? (PROVEN for inserts)

**Decision: YES, for the INSERT-after-pin concurrency case — proven by the green
test. The classifier is:**

| `version_of(id)` vs `pinned` | `get_at(id, pinned)` | Meaning | Action |
|---|---|---|---|
| `v != 0 && v <= pinned` | (== current value) | No write touched this key since the pin. By MVCC immutability the AS-OF value EQUALS the current value, so the posting's position is correct for the snapshot. | **include** (read via `get_at`, which == current) |
| `v > pinned` *or* `v == 0` | `None` | A write landed on this key after the pin AND the record did not exist at pin. | **exclude** — INSERT-after-pin correctly dropped |
| `v > pinned` *or* `v == 0` | `Some(_)` | The record existed at pin but was modified after. | **flag `concurrent_modified`** — see §1.3; a production path must fall back |

The `v != 0 && v <= pinned` branch is the common case (the cursor's table is
quiescent): every candidate is included in `O(1)` classification + one vectored
`get_at`. The prototype's `asof_index_seek_spike` implements exactly this loop
and is the green test's subject.

**Why `v <= pinned` ⇒ AS-OF == current is sound:** MVCC versions are immutable.
A record whose `RecordCell.version` is `<= pinned` has had no committed write
since the snapshot was taken, so the value the index posted (the value at
`version`) is identical to the value visible at `pinned`. The index's
current-state position therefore IS the pinned-snapshot position. (This is the
same invariant `get_at_many`'s "direct path" relies on at
`mvcc_store/mod.rs:1063-1072`.)

### 1.3 Open question 1 (cont.) — the two cases `version_of` CANNOT handle (DISPROVEN — mandatory gate)

**Decision: a current-state sorted index CANNOT correctly serve an AsOf ORDER BY
query in the presence of a concurrent UPDATE to the indexed field or a
concurrent DELETE. Both are proven by negative tests. A production fast path
must gate on a per-index "last mutation version" high-water and fall back to the
full scan when it advances past the pin.**

**The UPDATE-indexed-field case** (`negative_update_indexed_field_after_pin_seek_cannot_place_row`):
`plan_record_updated` (`:437-446`) REMOVES the old posting and SETs a new one at
the new value's position when the indexed key changes. So after a post-pin
update of the ORDER BY field, the record's pinned-version posting **no longer
exists** in the index — only the new-value posting does, stamped with the
post-pin version. `version_of > pinned` flags it `concurrent_modified`, but
neither including it (wrong position: it'd appear at its NEW value, not its
pinned value) nor excluding it (the row genuinely existed at pin and must appear
at its OLD position) is correct. The prototype conservatively drops it; the
baseline (full scan + `get_at`) returns it at its old position. **The seek
misses a row the snapshot must show.**

**The DELETE-after-pin case** (`negative_delete_after_pin_seek_misses_row`):
`plan_record_deleted` (`:460-477`) emits `RemovePosting`. After a post-pin
delete, the posting is GONE, so the seek never yields the record id at all —
`version_of` is never even consulted. The baseline returns the row via
`list_stream_with_tombstones` + `get_at` (the CR-B1 / #767 machinery). **The
seek misses a row the snapshot must show, with no `concurrent_modified` signal
possible** (the index simply does not contain it).

These are **not** fixable by refining the per-candidate classifier — the
pinned-version postings do not exist in a current-state index. They require
either (a) a versioned/temporal index (explicitly "a later performance slice"
per `read_as_of`'s own doc comment, `read_temporal.rs:30-34`), or (b) a
conservative gate that disables the seek when any concurrent write could have
moved/removed a pinned posting. The spike recommends (b) for Step 2: a cheap,
per-index monotonic "last mutation version" atomic, bumped on every
`on_record_created/updated/deleted`, compared against the cursor's
`pinned_version`. When `index_last_mutation <= pinned`, the index mirrors the
pinned state exactly and the seek is fully correct (every candidate is the
`v <= pinned` common case). When it has advanced, fall back to the existing
full scan. This is a one-way ratchet per cursor (the first concurrent write
disables the fast path for that cursor's lifetime), which honestly matches the
dominant cursor workload (paging a large, stable result set).

> **Note on the repo-level gate.** `RepoTxGate::last_committed()`
> (`repo_tx_gate.rs:299`) is a global high-water, but it is REPO-scoped (shared
> across tables). Using it as the gate would disable the fast path whenever ANY
> table in the repo is written — far too coarse. The gate must be per-index (or
> per-table); it does not exist yet and is Step 2's first touch point (§5.1).

### 1.4 What about the `live_version` / covering-projection freshness path?

`MvccStore::live_version` (`mvcc_store/mod.rs:1413`) and
`decode_covering_projection` exist for the **index-only / covering read path**
(slice A3): they validate that a covering posting's embedded version matches the
live cell version, to decide whether the posting's projected columns are fresh
enough to answer a query without fetching the record body. That is a
CURRENT-state freshness check, not an AsOf visibility check — it answers "is
this posting's projection up to date RIGHT NOW", not "was this record's value at
`pinned` what the posting claims". It does not help here (and, per §1.1, the
target index is non-covering anyway).

---

## 2. Where the fix lives — `read_as_of`, not a cursor-specific mechanism

**Decision: the AsOf-aware seek belongs inside (or dispatched from)
`read_as_of` (`read_temporal.rs:45`), NOT in `cursor_handlers.rs`.**

**Investigation — `read_as_of` has exactly ONE caller.** A workspace grep for
`read_as_of` finds a single call site: `read_exec.rs:297`, inside
`TableManager::read_impl`'s `Temporal::AsOf { at } =>` arm (`:295-299`). Every
AsOf read — cursor `FetchNext` AND any direct point-in-time query — funnels
through this one method. (`cursor_handlers.rs:1258-1260` builds the
`Temporal::AsOf { at: Version(pinned_version) }` query and hands it to the
engine's `read`; it does not call `read_as_of` directly.)

**Reasoning:**

1. **Placing it in `read_as_of` benefits every AsOf caller with zero
   duplication.** The seek logic already lives in `read_index_scan.rs`'s
   `read_keyset_seek` (the `Temporal::Latest` fast path). The AsOf variant is a
   sibling that adds the §1.2 classifier + the §1.3 gate; both share
   `lookup_range_first_k_page`. Duplicating this inside `cursor_handlers.rs`
   (the brief's alternative) would copy the ordered-walk + stale-posting-resume
   loop (`read_index_scan.rs:534-583`) into the server crate, where it does not
   belong and would drift.
2. **`read_as_of` already carries everything the seek needs** — the full
   `ReadQuery` (ORDER BY, WHERE, pagination), the `FilterContext`, and (after
   resolving `At`) the concrete `pinned_version`. The F-53a inline-top-K branch
   (`read_temporal.rs:119-243`) is the precedent for an order/limit-shaped
   short-circuit inside this method; the seek is the same class of fix.
3. **The gate check is naturally per-call.** `read_as_of` resolves `pinned`
   up front (`:60-72`); comparing it against the per-index high-water and
   branching to the seek vs the existing scan is a local decision.

**Caveat — the cursor's query shape needs a small adjustment.** The cursor
pages via a WHERE **boundary filter** (`field >= seek_key`, `cursor_handlers.rs:363-403`)
+ LIMIT + OFFSET `tie_skip`, NOT via `Pagination::After`. The existing
`try_plan_keyset_seek` fast path requires `Pagination::After`
(`read_planner.rs:492-497`), so the cursor cannot reach it today even on the
Latest path. Step 2 has two options (§5.2): (a) have the cursor emit
`Pagination::After` when keyset-eligible (it already tracks `seek_key` +
`tie_skip`, which map cleanly to `after { key, limit }` + `after_id`), so a
single new AsOf seek arm in `read_as_of` serves both; or (b) add a seek arm that
accepts the boundary-filter shape directly. The spike recommends (a) — it
unifies the cursor and one-shot AsOf paths and lets `try_plan_keyset_seek`'s
eligibility logic be reused.

---

## 3. Scope — single-column indexed ORDER BY only (CONFIRMED, matching `try_plan_keyset_seek`)

**Decision: Step 2's boundary is exactly a single-column ORDER BY covered by an
existing sorted index, ASC or DESC, keyset-eligible. Every other shape keeps the
existing full-rescan `read_as_of`. No reason found to narrow or widen it.**

**Reasoning:**

- The seek primitive `lookup_range_first_k_page` is single-column by
  construction (one `seek_encoded`, one `field_path`). Multi-column ORDER BY,
  computed expressions, and unindexed ORDER BY are already rejected by
  `try_plan_keyset_seek` (`read_planner.rs:477-489`) and inherit the same
  fallback here.
- The `version_of` classifier is field-agnostic (it's a per-record-id MVCC
  probe), so the §1.2 mechanism does not itself restrict columns — but the
  ordered-walk correctness (posting position == AS-OF position) does, because
  the index must order by the SAME column the query orders by. Single-column is
  the natural fit.
- Widening to multi-column would require a composite sorted index + a
  multi-key `seek_encoded` + composite `after_id` tie-breaking — a separate,
  larger slice that does not exist on the Latest path either. Out of scope.
- The gate (§1.3, §5.1) is what makes even the narrow case SAFE; without it the
  seek would silently miss rows under concurrent update/delete. Step 2 must land
  the gate alongside the seek.

---

## 4. What was prototyped

### 4.1 Prototype code (committed alongside this memo)

- **`crates/shamir-engine/src/table/tests/f53b_cursor_seek_spike_tests.rs`**
  (new) — a TEST-LOCAL `asof_index_seek_spike` helper that reimplements the
  seek + the §1.2 classifier using only public / `pub(crate)` engine APIs
  (`sorted_indexes().lookup_range_first_k_page`, `mvcc_store_ref().version_of` /
  `get_at`). It is deliberately NOT wired into the production `read()` path —
  that is Step 2. It returns an `AsofSeekPage` carrying `rows`,
  `resume_key`, `candidates_examined`, `excluded_inserts`, and
  `concurrent_modified` so the tests can measure work and prove the
  snapshot-isolation invariant. Four tests:
  1. `red_current_asof_path_scans_full_table_per_page`
  2. `green_asof_seek_matches_baseline_with_insert_after_pin`
  3. `negative_update_indexed_field_after_pin_seek_cannot_place_row`
  4. `negative_delete_after_pin_seek_misses_row`
- **`crates/shamir-engine/src/table/tests/mod.rs`** — one new `pub mod` line.

No production read-path code was touched (the brief: "prototype ONE case only,
do NOT implement Step 2").

### 4.2 The red→green proof (run 2026-07-30)

Harness: MVCC table (`MvccStore` + `RepoTxGate`, full history retained), a
non-covering sorted index on `score` (Int), 30 rows scores `0,10,…,290`,
`page_size = 10` (3 full pages). The cursor's pin = `gate.last_committed()`
after the initial inserts (mirrors `gate.open_snapshot().version()`).

```
./scripts/test.sh -p shamir-engine -- f53b_cursor_seek_spike
```
```
     Summary … 4 tests run: 4 passed, … skipped      exit=0
```

**Red (`red_current_asof_path_scans_full_table_per_page`)** — the CURRENT
production `read_as_of` (the cursor's per-`FetchNext` path), measured via the
`QueryStats::records_scanned` it already reports, over 3 cursor-style pages
(WHERE `score >=` prior-last + OFFSET 1 + LIMIT 10, `Temporal::AsOf`):

| page | `records_scanned` |
|---|---|
| 1 | 30 (== N) |
| 2 | 30 (== N) |
| 3 | 30 (== N) |
| **cumulative** | **90 (== 3N)** |

Each page enumerates the WHOLE table; paging K pages costs `O(K × N)`, not
`O(K × page_size)`. (F-53a's inline top-K bounds *memory*, not *scan count* —
the enumeration is still full-table.) Correctness sanity: the 3 pages are
exactly `[0…90]`, `[100…190]`, `[200…290]`.

**Green (`green_asof_seek_matches_baseline_with_insert_after_pin`)** — the
prototype seek vs the production baseline, 3 pages + an empty page 4, with a
row INSERTed after the pin before page 2 (`score=125`) and again before page 3
(`score=225`) — both sorting into an upcoming page's range:

| page | baseline scores | seek scores | match? | `candidates_examined` | `excluded_inserts` | `concurrent_modified` |
|---|---|---|---|---|---|---|
| 1 | `[0…90]` | `[0…90]` | ✅ | 10 | 0 | 0 |
| 2 (after +125) | `[100…190]` | `[100…190]` | ✅ | 11 | 1 (the `125` insert) | 0 |
| 3 (after +225) | `[200…290]` | `[200…290]` | ✅ | 11 | 1 (the `225` insert) | 0 |
| 4 | `[]` | `[]` | ✅ | 0 | 0 | 0 |
| **cumulative** | scan 90 | **seek 32** | — | — | — | — |

- **Snapshot isolation holds on the seek path**: `125` and `225` NEVER appear
  in any page (`excluded_inserts >= 1` on pages 2 and 3 — the classifier saw
  each late insert, `version_of > pinned` + `get_at == None`, and dropped it).
- **Identical contents**: every page's score sequence equals the production
  baseline (byte-identical row set, in order).
- **Meaningfully less work**: cumulative seek `candidates_examined = 32` vs
  baseline `records_scanned = 90` at `N=30`; the ratio widens with `N` (seek is
  `O(pages × page_size)`, baseline is `O(pages × N)`). At `N=10_000,
  page_size=10`, 3 pages would be ~30 seek vs ~30_000 scan.

**Negative — UPDATE indexed field after pin**
(`negative_update_indexed_field_after_pin_seek_cannot_place_row`): the `score=30`
row is `set` to `35` after the pin. Baseline (full scan at pinned) returns
`[10,20,30,40,50]` (the row at its OLD position). The seek flags
`concurrent_modified >= 1` and CANNOT place the row at 30 — `30` is absent from
the seek output. **The seek misses a row the snapshot must show.**

**Negative — DELETE after pin** (`negative_delete_after_pin_seek_misses_row`):
the `score=30` row is deleted after the pin. Baseline returns `[10,20,30,40,50]`
(via tombstone-inclusive enumeration). The seek returns `[10,20,40,50]` with
`concurrent_modified == 0` — the posting is gone, so the index never yields the
id and no signal is possible. **Same miss; same conclusion: the gate is
mandatory.**

### 4.3 What the prototype deliberately does NOT do

- No production read-path wiring (Step 2).
- No per-index mutation high-water (Step 2 §5.1) — the prototype's insert-only
  scenario never triggers §1.3, so the gate is not exercised; the negative
  tests prove why it is required.
- No DESC direction (symmetric; ASC proves the mechanism).
- No `Pagination::After` cursor-shape conversion (Step 2 §5.2).

---

## 5. Implementation plan (Step 2)

### 5.1 The per-index mutation high-water gate (FIRST — load-bearing for safety)

Add a monotonic "last mutation version" to `SortedIndexManager`
(`crates/shamir-index/src/legacy/sorted_index_manager.rs`):

- New field `last_mutation_version: AtomicU64` (or per-`SortedIndexDefinition`
  if finer grain is wanted; per-manager is sufficient since a cursor fast-path
  miss only falls back to the existing correct scan).
- `fetch_max(AcqRel)` on every `on_record_created` / `on_record_updated` /
  `on_record_deleted` (and the `plan_*` tx-stage variants, advanced at APPLY
  time — the same place `version` is known). The value is the write's MVCC
  version (the `version` arg already passed to `on_record_*`).
- New accessor `pub fn last_mutation_version(&self) -> u64` (Acquire load).

The gate in `read_as_of`: after resolving `pinned`, if the seek is otherwise
eligible, compare `sorted_indexes().last_mutation_version() <= pinned`. If true
→ seek is fully correct (every candidate is the §1.2 common case; `excluded_*`
and `concurrent_modified` stay 0). If false → fall back to the existing full
scan. This makes the §1.3 misses **impossible**: the fast path only runs when
the index provably mirrors the pinned state.

> **Subtlety — tx-path writes.** Sorted-index ops go into `tx.index_write_set`
> at STAGE time (`table_manager_tx_ops.rs:239`) and are APPLIED at commit. The
> high-water must be bumped at APPLY (commit) time, with the commit version, not
> at stage time — otherwise an uncommitted tx would disable the fast path for
> unrelated cursors. The apply path is `commit.rs`'s Phase 5c
> (`IndexPut`/`IndexDel` replay); wire the `fetch_max` there alongside each
> posting apply.

### 5.2 The AsOf seek arm in `read_as_of`

- New `pub(super) async fn read_as_of_keyset_seek` in a sibling file
  (`read_asof_seek.rs`, mirroring `read_index_scan.rs::read_keyset_seek`):
  resolves eligibility (single-column indexed ORDER BY + keyset pagination, via
  a shared `try_plan_keyset_seek`-style helper that ALSO accepts the AsOf
  temporal), checks the §5.1 gate, then runs the prototype's loop
  (`lookup_range_first_k_page` + the §1.2 classifier) against `pinned`.
- Wire it as the FIRST branch in `read_as_of` (before the inline-top-K and
  full-scan tails), gated on eligibility + the high-water. On any miss signal
  (`concurrent_modified > 0` — should be impossible under the gate, but kept as
  a defence-in-depth), fall back to the full scan.
- **Cursor shape**: convert the cursor's keyset pages to `Pagination::After`
  when keyset-eligible in `cursor_handlers.rs` (it already tracks `seek_key` +
  `tie_skip` + the last `RecordId`), so the same seek arm serves one-shot and
  cursor AsOf reads. This also lets the cursor drop the per-page
  boundary-filter re-scan entirely for eligible cursors.

### 5.3 Tests to add in Step 2

- Port the prototype's four tests to call the production `read()` path (AsOf +
  `Pagination::After`) instead of the test-local helper, asserting the
  `index_used` stat label switches to the seek path and `records_scanned`
  drops to `O(page_size)`.
- A gate-fallback test: concurrent UPDATE to the indexed field after pin →
  assert the seek arm declines (high-water > pinned) and the result equals the
  full-scan baseline (no miss).
- A cursor-level test (`cursor_handler_tests.rs`): `CreateCursor` → `FetchNext`
  page 1 → concurrent insert → `FetchNext` page 2 asserts the insert is absent
  AND the page is correct, AND the server's per-page work dropped (via the
  `QueryStats` the cursor already surfaces).
- DESC direction parity.
- Update `KNOWN_LIMITATIONS.md:402-405` once the fast path lands for eligible
  cursors (narrow the "full pinned-version scan per page" claim to "only for
  non-keyset-eligible / concurrently-written cursors").

### 5.4 Explicitly OUT of scope for Step 2

- Multi-column / computed-expression ORDER BY (no composite seek primitive).
- A versioned/temporal index (the only mechanism that could make the seek
  correct WITHOUT the high-water gate — a much larger, separate slice; the gate
  makes it unnecessary for the cursor use case).
- Covering-index `INCLUDES` interaction (the §1.1 stamp stays unused; the
  mechanism is `version_of` regardless of covering-ness).

---

## 6. Decision summary

| Question | Decision | Rationale |
|---|---|---|
| Q1a: does the covering-projection version stamp apply to the target index? | **NO.** Non-covering single-column indexes store `Bytes::new()`; the envelope exists only for `INCLUDES` indexes, and `lookup_range_first_k_page` discards values anyway. | Verified at all three plan entry points (`:355-358`, `:422-431`, `:460-477`) + the seek primitive's key-only iteration. |
| Q1b: can `version_of` make the seek AsOf-aware? | **YES for INSERT-after-pin** (proven, green test); **NO for UPDATE-indexed-field / DELETE-after-pin** (disproven, negative tests). | `version_of` is a cheap in-memory per-record MVCC probe; it correctly excludes post-pin inserts but cannot recover records whose pinned-version posting was moved/removed by a concurrent write. |
| Q2: where does the fix live? | Inside / dispatched from `read_as_of` (single caller `read_exec.rs:297` serves every AsOf read). NOT cursor-specific. | One call site → benefits all AsOf callers; reuses `read_keyset_seek`'s ordered walk; avoids server-crate duplication. |
| Q3: Step 2 scope | Single-column indexed ORDER BY, ASC/DESC, keyset-eligible, behind a per-index mutation high-water gate; everything else keeps the full scan. | Matches `try_plan_keyset_seek`'s existing restriction; the gate is what makes even this narrow case safe (§1.3). |

---

## 7. Verification run (pre-handoff gate)

```
cargo fmt  -p shamir-engine -p shamir-server -- --check     # exit 0
cargo clippy --workspace --all-targets -- -D warnings        # exit 0
./scripts/test.sh -p shamir-engine -- f53b_cursor_seek_spike # 4/4 passed, exit 0
```

No production read-path code changed; the only committed code is the test-local
prototype + the memo. The orchestrator should run the full
`./scripts/test.sh -p shamir-engine -p shamir-server --full` scope to confirm
no broader regression (the spike touches only a new test module + the test
manifest line).
