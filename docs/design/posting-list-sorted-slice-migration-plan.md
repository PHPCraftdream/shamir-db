# Posting-list representation: `BTreeSet<RecordId>` → sorted-slice `Arc<[RecordId]>`

Task #499 (PERF-RADICAL-3.2), deferred from #488. Audit source:
`docs/audits/2026-07-06-perf-radical-o-notation.md` finding 3.2.

## 1. Current state (with file:line citations)

### 1.1 The canonical posting-fetch entry point

`crates/shamir-index/src/legacy/index_manager.rs::lookup_by_index`
(lines 705-763) returns `Arc<BTreeSet<RecordId>>`. It:

1. Builds the 25-byte physical `index_key`
   (`build_index_key(false, name_interned, values)`).
2. On a `posting_cache` HIT returns `Arc::clone` of the cached
   `Arc<BTreeSet<RecordId>>` (O(1) — the #488 fix).
3. On MISS it prefix-scans `info_store.scan_prefix_stream(index_key, 512)`,
   and for every matched posting key extracts the trailing 16 bytes into a
   `RecordId` and `BTreeSet::insert`s it (line 734), then wraps the set in an
   `Arc`, populates the cache, and returns it.

### 1.2 Posting-key layout — the scan is ALREADY sorted and unique

`build_posting_key` (`index_keys.rs:306-311`):

```
posting_key = index_key (25 bytes) || record_id.as_bytes() (16 bytes)
```

Every posting under one `index_key` shares the identical 25-byte prefix, so
the storage prefix-scan visits them in ascending order of the trailing
`record_id` bytes. `RecordId` is `#[derive(..., PartialOrd, Ord, ...)]` over
`[u8; 16]` (`record_id.rs:20-21`), i.e. its `Ord` is byte-lexicographic —
**identical** to the physical scan order. There is at most ONE posting per
`(index_key, record_id)` (the value is an empty `Bytes`), so the scan is also
inherently **duplicate-free**.

Conclusion: the `BTreeSet` on the MISS path is pure overhead. The scan
already yields sorted, unique `RecordId`s; collecting them into a `Vec`
(push in scan order) produces the same logical set with:

- zero per-node heap allocations (one growable buffer instead of N tree
  nodes),
- no tree rebalancing,
- a contiguous, cache-friendly layout for the consumer's iteration.

### 1.3 Every consumer of the returned posting set

Grepped `\.lookup_by_index\(` across `crates/**/src/**/*.rs`. The internal
(`IndexManager`) return value is consumed by:

| Site | Usage | Semantics needed |
|---|---|---|
| `read_index_scan.rs:518-524` | `record_ids.extend(ids.iter().copied())` — UNION into a result set | ordered iteration |
| `read_exec.rs:445-449` | `total += ids.len()` (count) | `len()` |
| `read_exec.rs:1349-1355` | `ids.iter()` collect into `Vec` | ordered iteration |
| `write_helpers.rs:381-386` | `record_ids.extend(ids.iter().copied())` — UNION | ordered iteration |
| `write_helpers.rs:440-445` | `ids.iter().next()` — first element | first / iteration |
| `validator_db.rs:225-231` | `!ids.is_empty()` | `is_empty()` |
| `validator_db.rs:304-310` | `ids.iter()` + `Some(id) != exclude_rid` | ordered iteration |
| `table_manager_index_mgmt.rs:362` | `(*arc).clone()` → owned `BTreeSet` (public wrapper) | boundary conversion |

The `TableManager::lookup_by_index` public wrapper (line 331) returns an
owned `BTreeSet<RecordId>`; its only callers are `RepoInstance::lookup_by_index`
(`repo_instance.rs:1123`) and `DbInstance::lookup_by_index`
(`db_instance.rs:270`), both pass-through public API with **no callers** in
`shamir-server`/`shamir-client`/`shamir-sdk`/`shamir-db`/`shamir-connect`
(verified at #488 and re-verified here). Tests assert `.contains(&rid)`,
`.len()`, `.is_empty()` on the wrapper result.

**Every consumer needs only ORDERED-ITERATION / `len` / `is_empty` / first /
membership semantics.** NONE needs mutable set operations (insert/remove into
the returned set) or set algebra directly on the returned value. `contains`
on a sorted slice is `binary_search(..).is_ok()` (O(log n)).

### 1.4 There is NO posting-set-vs-posting-set intersection

The audit's premise of an "O(n log m) `BTreeSet` intersection with cache
misses at every comparison" does **not** correspond to any current call site.
The multi-value loops in `read_index_scan`/`read_exec`/`write_helpers` are
UNIONs (`extend`), one `lookup_by_index` per value in an `IN (...)` / composite
list, merged into a downstream `new_set`. The AND across different filter
predicates is applied as a *residual filter over fetched records*, not as a
merge over two posting slices. So the galloping/merge-intersection algorithm
the audit's finding 3.2 imagined has no consumer to serve today; designing one
now would be speculative. The real, measurable cost of `BTreeSet` here is
(a) N node allocations on the MISS scan and (b) pointer-chasing iteration on
every consumer read (cache HIT included).

### 1.5 The write path never materialises a posting SET

`plan_record_created` / `plan_record_updated` / `plan_record_deleted`
(`index_manager.rs:441-639`) emit individual `IndexWriteOp::SetPosting`/
`RemovePosting` for one posting KEY each. `create_index` / `create_index_from_records`
(lines 214-350) likewise push individual `(posting_key, empty)` pairs into a
`set_many`. **No `BTreeSet<RecordId>` exists anywhere on the write / backfill
path** — the sorted-slice representation is a pure READ-side concern. There is
therefore no coupling with the crash-safety guarantees established by the #488
/ #490 write-hook and incremental-backfill work: those paths are untouched.

## 2. Feasibility verdict — SAFE, implement now

This is the clean, low-risk read-side change the brief's Step-2 hopes for:

- The scan already produces sorted, unique `RecordId`s → collecting into a
  `Vec<RecordId>` → `Arc<[RecordId]>` is correctness-preserving by
  construction (same elements, same order, no dedup needed).
- All internal consumers use iteration / `len` / `is_empty` / first, which
  `Arc<[RecordId]>` supports natively; the one membership check pattern maps
  to `binary_search(..).is_ok()`.
- The public `TableManager`/`Repo`/`Db` wrappers keep their `BTreeSet` return
  type via a boundary `iter().copied().collect()` (they have no hot-path
  caller — accepted trade-off, unchanged from #488).
- The write / backfill / crash-safety paths are not touched.

No deferral is warranted. The full read-side migration fits one surgical pass.

**STATUS: IMPLEMENTED (task #499).** The single-step migration in §4 landed:
`lookup_by_index` now returns `Arc<[RecordId]>`; the cache value type,
consumers, and public-wrapper boundary were updated; regression tests and a
before/after bench were added. See §4 / §6 below for the verified results.

## 3. Type design

Use the bare `Arc<[RecordId]>` (no newtype). Rationale: a newtype would need
`Deref<Target=[RecordId]>` + `FromIterator` and buys nothing over the slice's
own inherent methods, which already cover every consumer. The cache value type
becomes `Arc<[RecordId]>`; the return type of
`IndexManager::lookup_by_index` becomes `DbResult<Arc<[RecordId]>>`.

Membership (`contains`) on the returned value, where needed, is
`slice.binary_search(&id).is_ok()` (sorted-slice invariant documented at the
call site and on the method).

## 4. Migration (single committable step)

1. `IndexManager::posting_cache` value type
   `Arc<BTreeSet<RecordId>>` → `Arc<[RecordId]>`.
2. `lookup_by_index` MISS path: collect the scan into
   `Vec<RecordId>` (already sorted+unique), `into()` → `Arc<[RecordId]>`.
3. Internal consumers: `.iter().copied()` / `.len()` / `.is_empty()` /
   `.first()` unchanged (slice supports them). No `.contains(&x)` on the
   internal `Arc` return exists; the public-wrapper tests use `BTreeSet`.
4. Public wrappers (`table_manager_index_mgmt`, `repo_instance`,
   `db_instance`) unchanged in signature; the internal
   `table_manager_index_mgmt` wrapper converts via
   `arc.iter().copied().collect()` instead of `(*arc).clone()`.
5. Regression test: assert byte-for-byte identical membership between the old
   `BTreeSet` path and the sorted-slice path across edge cases (empty, single,
   disjoint, fully-overlapping, 10k). The slice must be sorted and
   duplicate-free.
6. Bench: add an equality-lookup + full-iteration bench at |postings| ≥ 10k
   (the audit's stated gap), measuring the consumer's real cost
   (`lookup + iterate/collect`), baseline `BTreeSet` vs sorted-slice.

## 5. Landmines

- **Sortedness is an invariant, not an assertion.** It holds because the
  physical scan order equals `RecordId::Ord`. If a future change makes the
  posting-key suffix NOT the raw `record_id` bytes (e.g. a covering-index
  envelope inserted before the id), the "already sorted" property breaks. The
  regression test pins this by asserting the returned slice is sorted.
- **Duplicate ids.** Impossible today (one posting per `(index_key,
  record_id)`), but if the scan could ever yield the same id twice, `Vec`
  push would keep both whereas `BTreeSet` deduped. The test asserts
  duplicate-freedom; if that ever changes, add a `dedup()` after collect.
- **`contains` cost.** On the old `BTreeSet` it was O(log n) tree walk; on the
  slice it's O(log n) `binary_search` — same asymptotics, better constant.
  Only matters for the public wrapper's `BTreeSet` (unchanged) — internal
  callers don't do membership on the returned value.
