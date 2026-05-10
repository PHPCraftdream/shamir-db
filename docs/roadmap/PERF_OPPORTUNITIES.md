# Performance Opportunities — Beyond Asymptotic Wins

Status: **review pass after the A → D + sorted-index sprints.** Companion
to `docs/ops/PERF_BASELINE.md` (measured numbers).

Where A/B/C/D cut O(n) → O(log n) on the write path (set/update/delete
via index → 800–1100× wins; count(*) → 3000× via RecordCounter), this
document is about **next-class wins**. Two flavours mixed in:

- **Per-record constant-factor reduction** — chip away at the
  "ceremony" each op does. Cumulatively 3–10× across the profile.
- **Architectural feature additions** that unlock new asymptotic
  paths — most notably covering indexes for disk-backend range
  queries, which the latest bench (`796cdbf` / `c1f0520`) showed as
  the gap between in-memory wins (4×) and disk wins (1.2–1.8×) at
  the same selectivity.

> **2025-05-10 update.** This pass re-graded every item against
> measured bench data and verified each symptom in code. Win
> estimates from the original draft were trimmed where they
> overshot. Items that landed during the sprint are marked **DONE**
> and kept for history. New items surfaced by the sled-bench cost
> model are marked **NEW** at the bottom.

---

## Quick map — what's worth doing first

Ranked by cost / value after the verification pass:

| Tier | Items | Why |
|------|-------|-----|
| 🥇 **Easy wins** | **F** (interner Vec reverse, 1-2 h), **L** (batch RecordId, 1-2 h), **Q1** (MIN/MAX fast-path, 1 h), **Q2** (Filter::Gt/Lt in sorted-index planner, 1 h) | Each cheap; each verified; small but free 3–10 % each. |
| 🥈 **Real wins on real workloads** | **G** (LRU posting cache, 1 day), **O** (covering index, ~1 week), **P** (vectored multi-get, 2–3 days) | These move the dial on UI dashboards and disk-backend production. |
| 🥈 **Architectural cleanup** | **H₂** (Persistable trait), 1 day | Not a perf spike — prevents the next recurrence of write-amplification. |
| 🥉 **Modest, wide refactors** | **K** (projection lazy, 5-7 days), **R** (reverse iter on Store, 1 day) | Real but expensive. K only really helps when SELECT is narrow. |
| 👎 **Overestimated in original draft — defer** | **I** (sync Store core), **J** (SmallMap), **M** (specialised filter shapes) | Original draft predicted 20–50 %; closer look says 5–15 %, with effort cost not justified yet. |
| 🚫 **Later, conditional** | **N** (prepared plan cache), **rkyv** | Only after profiling against real workload. |

---

## Слепок по-русски

### Что вижу как структурную трату

#### 1. Async ceremony там где работа синхронная — **REVISED**

Каждая операция в `Store` trait объявлена `async fn`. Для in-memory
backend — `DashMap.insert` чистый синхронный. Для redb — синхронный с
маленькой блокировкой. Но snapshot всех вызовов проходит через tokio
state machine: future, waker, poll. На `set` нет ни одного `await`
внутри backend'а — только async-обёртка вокруг sync работы.

**Original draft:** 20–40 % win на in-memory.
**Actual:** ~5–15 %. Async overhead per call ~50–100 ns. Для
in-memory ops в микросекундах это ~5 % per call; накапливается, но
не до 40 %.

Эффорт: 2–3 дня в исходном плане → **реалистично 1 неделя**. Все
backend impls + все engine call sites — wide mechanical refactor.

Cost / benefit не оправдан в текущей фазе.

#### 2. Allocation pressure повсюду — **REVISED**

`InnerValue::Map` каждый — heap-allocated HashMap. 1000 records на
read = 1000 HashMap allocations.

**Original draft:** 10–30 % via SmallMap (inline для ≤8 полей).
**Actual:** ~3–8 %. Rust allocator очень быстрый; HashMap allocation
~50 ns. Per-record decode + interner lookups доминируют.

Эффорт реально 2-3 дня — Map тип фундаментальный, правки рябят по
codec + tests. Win не оправдывает риск регрессии в текущей фазе.

#### 3. Interner reverse-lookup на каждом field-resolve — **REVISED**

Read возвращает 1000 records × 5 полей = 5000 `interner.get_str`
calls. Реализация: `map_interned_to_user.get(id).map(|k|
k.clone())` — DashMap shard lookup + Arc clone.

Это реально (верифицировано в коде).

**Original draft:** 5–50 % win.
**Actual:** ~2–8 %. DashMap lookup ~80–150 ns + clone ~40 ns =
~200 ns × 5000 = 1 ms из 80 ms read = 1.2 %. Уберём — небольшой
кусочек, но дёшево.

Эффорт 1-2 ч, верно. Просто чистый win — делаем.

#### 4. Index posting list — монолитный bincoded blob — **REAL, KEEP**

`lookup_by_index(idx_name, value)` лезет в info_store, читает ВЕСЬ
BTreeSet, десериализует. Для city-index ~1250 ids = ~20 KB parse per
lookup.

LRU cache `(table, idx, value) → BTreeSet<RecordId>` — read из
info_store происходит один раз, дальше — память. Invalidate на
write.

Win **5–30× на repeat lookups** (UI dashboards, admin tables) —
оценка реальная. Cold cache: as today.

Эффорт 1 день — invalidation correctness требует careful tests.

#### 5. Filter evaluation через vtable — **REVISED**

`compile_filter` → `Box<dyn FilterCallback>`. `cb.matches(record,
ctx)` per record — virtual call.

**Original draft:** 30–50 % win на simple-filter scans.
**Actual:** ~5–15 %. vtable dispatch ~5–10 ns/call. Для 10K records
× ~10 ns = 100 µs из 90 ms scan = 0.1 %. На scan-heavy путях фильтр
не bottleneck; data load доминирует.

Эффорт 1 день, верно — но win скромный.

#### 6. Persist amplification — **DONE** ✓

> Закрыто в `a3013c7` (counter cache + bulk_insert -29 %). Counter
> теперь in-memory `AtomicU64`; `persist()` no-op'ится при unchanged.
> Тот же паттерн что Opt A для interner.

H₂ (generic `Persistable` trait) — рекомендуется отдельным
архитектурным sprint'ом чтобы рецидив не повторился для следующего
metadata-state'а. ~1 день.

#### 7. Lazy materialization отсутствует — **REAL, NARROWED**

`SELECT email FROM users LIMIT 100` сейчас декодирует все поля каждой
matched записи, потом `apply_select` отбрасывает 95 %.

Win: **3–7× для projection-heavy queries** (узкая выборка из широких
записей). Эффорт 5–7 дней — codec module + read executor.

Полезно когда workload — SELECT с явной проекцией. Для SELECT * win
нулевой.

#### 8. Bincode vs zero-copy — **CONDITIONAL**

rkyv даёт zero-copy чтение через `unsafe archived_root`. Для
schema-less `InnerValue` — sloppy fit (рябит по типу). Реально
оправдано для **system records** (interner state, counter, index
meta) где shape известен. Подождать.

#### 9. RecordId::new — три syscall'а на insert — **REAL, KEEP**

```rust
let now_micros = Utc::now().timestamp_micros();   // vDSO на Linux
rand::rngs::OsRng.fill_bytes(&mut bytes[8..16]);   // getrandom syscall
```

Bulk insert 1000 = 1000 syscalls (главное — OsRng). На нашем хосте
syscall ~1 µs → 1 ms из 25 ms bulk_insert = 4 %.

Batch allocator (1024 ids worth of randomness one call) сводит к
**1 syscall на 1000 inserts**. Win 3–8 %, эффорт 1-2 ч.

#### 10. JSON parsing на каждом execute() — **DEFER (Opt N)**

Re-parse одного и того же query shape. Win 10–30 % только на
**стационарном** workload (UI dashboards с фиксированным набором
запросов). Эффорт 1 неделя (parameter detection в AST).

Оставить на потом — не делать пока не появится profile-evidence что
parse cost доминирует.

### Один взгляд под другим углом

Эти 10 пунктов = «БД выполняет много церемониальной работы для
каждого record на каждом проходе». Каждый по отдельности — 3–15 %.
Сумма дешёвых (F + L + Q1 + Q2): ~10–25 % cumulative за полдня
работы.

Дорогие (G, O, P): реальные win'ы для production workloads на
disk-backend и UI-style read-heavy — но это уже sprint'ы, не «между
делом».

И — критическое наблюдение из последнего бенч-прогона на sled:
**самый большой неисследованный win — covering index (Opt O ниже).**
Disk range queries сейчас платят K random reads после index lookup;
covering index убирает эту цену. См. break-even анализ в
`PERF_BASELINE.md` (раздел «Sorted index on disk backend»).

---

## Per-item details (English)

Letters continue from A/B/C/D used in `PERF_BASELINE.md`. New items
(post-verification) start at **O**.

### Opt F — interner reverse-lookup as `Vec<String>` ✓ KEEP

**File:** `crates/shamir-types/src/core/interner/`

**Symptom (verified in code).** Reverse mapping is a `TDashMap<u64,
UserKey>`; `get_str(id)` does hash + shard + clone. The forward
direction stays write-only, so swapping reverse to a `Vec<String>`
indexed by `u64` is safe — interner is monotonic, no removal.

**Effort.** 1-2 hours.

**Realistic win.** 2–8 % of full-read query latency (corrected from
original "5–50 %" — verified via measured cost of `~200 ns × 5000
lookups` per 10K-record read).

Still net-positive: cheap, no surface change, narrows a verifiable
hot loop.

### Opt G — in-memory LRU cache for index posting lists ✓ KEEP

**File:** `crates/shamir-engine/src/index/index_manager.rs`

**Symptom.** Every `lookup_by_index` re-deserialises ~20 KB blob from
`info_store`. Hot for indexed read/update/delete repeat workloads.

**Fix.** `lru::LruCache<(idx_name, values), BTreeSet<RecordId>>`,
~1-2 MB per repo. Invalidate on `on_record_*` hooks.

**Effort.** 1 day, including invalidation correctness tests.

**Realistic win.** 5–30× on repeated indexed lookups (admin
dashboards, polling). Cold cache: as today.

### Opt H — counter persist debouncing **DONE in `a3013c7`**

In-memory `AtomicU64` cache + `persist()` short-circuit on unchanged.
Measured `bulk_insert/1000` win: 27.2 ms → 19.4 ms (-29 %).

### Opt H₂ — generic `Persistable` mechanism ✓ KEEP

**File:** new module in `shamir-engine`.

**Symptom.** Persist amplification is a **pattern**. Fixed once for
interner (Opt A), again for counter (Opt H); will recur for every
metadata blob.

**Fix.** `Persistable` trait + `PersistRegistry`. End-of-batch hook
calls `flush_dirty()` once. No more per-op `.persist().await`
sprinkled across write_exec.rs.

**Effort.** ~1 day including migrating interner + counter onto it.

**Win.** Not a perf spike — recurrence prevention. 0–5 % directly;
infinite value in not having to fix it again next time.

### Opt I — sync `Store` API + async wrapper ❌ DEFER

**Win revised down.** Original "20–40 % on in-memory" was optimistic.
Async per-call overhead is ~50–100 ns; for in-memory ops in
microseconds that's ~5 % per call, accumulating to maybe 10–15 %
across a full operation chain — **not 40 %**.

**Effort revised up.** "2-3 days" → **~1 week**. Every backend impl
+ every engine call site. Mechanical but wide.

Cost / benefit not justified yet. Re-consider when other dirt is
out.

### Opt J — inline `SmallMap` for small records ❌ DEFER

**Win revised down.** Original "10–30 %". Reality: Rust allocator
~50 ns per HashMap alloc; codec decode cost dominates. ~3–8 % real
win.

**Effort.** 2-3 days, foundational type, ripples through codec +
tests.

Defer.

### Opt K — projection-aware lazy materialisation ✓ KEEP, NARROWED

**Symptom.** `SELECT email LIMIT 100` decodes all fields then drops
95 %.

**Realistic win.** 3–7× for projection-heavy queries (narrow SELECT
out of wide records). Zero win for SELECT *.

**Effort.** Revised 3-5 days → **5-7 days**. Touches codec module +
read executor. Done right, the projection mask can also flow into
future `Store` partial-read APIs.

Real, but expensive. After F / L / Q1 / Q2 / G.

### Opt L — batch `RecordId` allocator ✓ KEEP

**Symptom (verified).** `RecordId::new()` calls `Utc::now()` (vDSO on
Linux, ~20 ns) + `OsRng.fill_bytes(&mut bytes[8..16])` (real
`getrandom` syscall, ~1 µs). Bulk insert N=1000 → ~1 ms wasted
purely on syscalls before any storage write.

**Fix.** Thread-local `RecordIdAllocator` with 16 KB random pool;
refill every 1024 ids. Single syscall per pool.

**Effort.** 1-2 hours.

**Win.** 3–8 % on bulk insert (from `~1 ms / 25 ms`).

### Opt M — specialised hot filter shapes ❌ DEFER

**Win revised down.** Original "30–50 % on simple-filter scans".
Reality: vtable dispatch ~5–10 ns; for 10K records × 10 ns = 100 µs
out of 90 ms scan = 0.1 %. Scan-heavy paths bottleneck on data load,
not filter eval.

**Realistic win.** 5–15 %. Effort 1 day — reasonable, but win is
too small relative to F / G / Q1 / Q2.

Defer.

### Opt N — prepared-query plan cache ❌ DEFER

**Conditional.** Only meaningful on stationary workloads (UI
dashboards reusing same query shape). Effort ~1 week (AST
parameterisation is non-trivial). Wait for profile evidence.

---

## NEW post-bench items

These surfaced during the sled-bench cost analysis (commit `c1f0520`
in `PERF_BASELINE.md`) — they're not in the original draft.

### Opt O — covering index ★ HIGH-IMPACT

**File:** `crates/shamir-engine/src/index/sorted_index_manager.rs`
(extend with optional `included_fields` per index).

**Symptom (measured).** On sled at 10 % selectivity, sorted-index
range queries are only **1.2–1.8×** faster than full scan. Break-down:

- B-tree index range scan: cheap, scales with K matches.
- **N×random `data_store.get(id)`: ~125 µs/record** on sled.
- Sequential scan (full scan path): ~8 µs/record.
- Break-even: K/N < ~6 %.

So when selectivity is ≥ 6 %, the per-record random-read penalty
eats the records-not-loaded savings. This is real DB physics.

**Fix.** Store projected fields directly in the index entry:

```text
physical_key  = SORTED_TAG || name_interned || encoded_value || record_id
physical_value = bincode(Map of included_fields)  ← NEW (was empty)
```

On range query that touches only `included_fields`, the data store
is **never opened**. Index scan returns the answer directly.
True O(log N + K) on disk.

DDL:

```json
{ "create_index": "by_age_with_email",
  "table": "users",
  "fields": [["age"]],
  "sorted": true,
  "include": ["email", "name"] }
```

**Effort.** ~1 week. Storage layout extension, write-path
maintenance (covered fields update on every record change), planner
extension (recognise when query is covered).

**Realistic win.** **100–1000× on disk for range queries where the
SELECT is satisfied by `included_fields`** — eliminates the random-
read penalty entirely. This is the path to Postgres-class
range-scan performance.

The single most impactful item left.

### Opt P — vectored / batched data-store get

**File:** `crates/shamir-storage/src/types.rs` + backend impls.

**Symptom.** Where Opt O isn't applicable (query needs fields not in
the index), we still do N independent `data_store.get(id)` calls.
On sled each is a B-tree walk from root = ~125 µs. For K=1000
matches, 125 ms purely in random reads.

**Fix.** Add `Store::get_many(keys: Vec<Bytes>) -> Vec<DbResult<Bytes>>`
to the trait, with native impls on backends that can fold multiple
gets into a single B-tree pass:

- **sled** has `Tree::iter` from any starting point — sort keys,
  iter once, pick up each.
- **redb** allows multiple `get`s inside one read transaction —
  amortises txn setup cost.
- **fjall / nebari / persy** — similar patterns.
- Default impl: loop over `get` — same as today.

**Effort.** 2-3 days. Trait extension + 5 backend impls + engine
hook into `lookup_records_via_index`.

**Realistic win.** 3–10× on disk for index-based lookups when K is
in the hundreds-thousands. Combines well with Opt O (covering only
the filter columns, falling back to vectored get for projected
fields not in the index).

### Opt Q1 — MIN(field) / MAX(field) fast-path via sorted index

**File:** `crates/shamir-engine/src/table/read_exec.rs` (new fast
path) + `sorted_index_manager.rs::lookup_min` (already implemented,
not wired).

**Symptom.** `SELECT min(score) FROM users` currently does a full
scan + reduce. We already have `SortedIndexManager::lookup_min`
returning the first record under the prefix (O(log n + 1)) — it's
just not wired into the query planner.

**Fix.** In `read()`, before the regular index plan, recognise:

```rust
- exactly one aggregate item: Aggregate { func: Min/Max, field }
- no WHERE / GROUP BY / ORDER BY / DISTINCT
- sorted index exists for `field`
```

→ Call `lookup_min` (or `lookup_max` once reverse iter lands —
see Opt R). Return a single-record result.

**Effort.** ~1 hour for MIN (lookup_min already exists).
~1 day for MAX (needs Opt R first).

**Realistic win.** O(N) → O(log n) — **300–1000× at N=10K** for the
MIN case. Cheap and obvious.

### Opt Q2 — `Filter::Gt` / `Filter::Lt` in sorted-index planner

**File:** `crates/shamir-engine/src/table/read_exec.rs::try_plan_sorted_index_scan`

**Symptom.** Current planner handles `Between`, `Gte`, `Lte`
through the sorted index. `Gt` and `Lt` (strict) fall through to
full scan — not because we can't, just because the boundary
"exclude exact match" wasn't wired yet.

**Fix.** `Gt`: lower = `prefix || encoded(value) || [0xFF; 16]`
(skips all record_id tiebreakers at exact match value). `Lt`:
symmetric on upper bound. The codec is already there; just construct
the right bounds.

**Effort.** ~1 hour.

**Realistic win.** Same speedup as `Gte/Lte` on the affected
queries — 5–30× depending on selectivity. Pure capability fill-in.

### Opt R — reverse iteration on `Store` trait

**File:** `crates/shamir-storage/src/types.rs` + backend impls.

**Symptom.** `MAX(field)` and `ORDER BY field DESC + LIMIT K` need
to read the index from the end. Today no `Store` method supports
reverse iteration; both queries fall back to full scan + in-memory
sort.

**Fix.** Add `iter_range_stream_reverse(start_inclusive,
end_inclusive, batch_size)` to the trait. Default impl: collect to
Vec, reverse, yield. Native impls: sled `tree.range(...).rev()`,
redb `range(...).rev()`, etc. — they all support it cheaply.

**Effort.** ~1 day (mirror of the forward range work; 28 tests
ported with `_reverse` suffix).

**Realistic win.** Unlocks Q1 (MAX), and `ORDER BY DESC + LIMIT K`
on indexed columns — same magnitude as the existing ascending fast
path (Opt #1 from the earlier sprint plan).

---

## Recommended sprint order — revised

### Sprint α — half-day easy wins (3-5 hours total)

Pure cleanup, verified low-risk:

1. **Q1** — wire `MIN(field)` fast-path (1 hr).
2. **Q2** — Filter::Gt / Filter::Lt in sorted-index planner (1 hr).
3. **F** — interner `Vec<String>` reverse-lookup (1-2 hr).
4. **L** — batch `RecordId` allocator (1-2 hr).

Re-run `engine_perf.rs`, document deltas in `PERF_BASELINE.md`.

### Sprint β — read-heavy disk wins (1 day)

5. **G** — LRU posting-list cache with invalidation tests.

### Sprint γ — the big disk story (~1 week)

6. **Opt R** — reverse iter on Store. Unblocks MAX / DESC LIMIT
   asymptotics.
7. **Opt O** — covering index. **THE** path to Postgres-class
   range-query latency on disk.
8. **Opt P** — vectored multi-get for non-covered queries.

This is the single most impactful chunk left in the whole
performance picture.

### Sprint δ — architectural cleanup (1 day)

9. **H₂** — `Persistable` trait + registry. Migrate interner +
   counter onto it. Stops the next write-amplification recurrence
   before it starts.

### Sprint ε — projection (~1 week, conditional)

10. **K** — projection-aware lazy materialisation. Only if a real
    SELECT-narrow workload appears.

### Deferred / no plans yet

- **I, J, M** — overestimated in the original draft. Revisit after
  γ is shipped and we have new profile data.
- **N** — prepared-query plan cache. Conditional on observed
  re-parse cost.
- **rkyv** for system records only. Wait for shape stability.

Total Sprint α + β + γ: ~1.5 weeks of focused work. Cumulative gain
is hard to predict without re-bench, but α alone is +10–25 % across
the profile, and γ unlocks the disk-side ceiling that the latest
benchmark explicitly identified.

---

## What we deliberately skip — and why

- **Static dispatch through generics over backend.** Refactor of
  2-3 weeks for 10–15 % win. Not worth it while simpler wins are on
  the table.
- **rkyv for `InnerValue` itself.** Format lock-in, big test
  surface, modest win. Wait.
- **SIMD for filter evaluation.** Niche, not our typical workload.
  Reconsider for column-oriented analytics.
- **Custom binary protocol on the wire** instead of msgpack.
  Replacement complexity unjustified.

---

## Ground truth before each item

A reminder: every item above is *expected* — based on reading code
and structural reasoning. Before committing time to any of them,
**run `engine_perf.rs` first**, profile (`flamegraph` / `samply`)
the actual hot scenario, and confirm the symptom matches the
prediction.

The A → D results validated this loop: predictions in
`TRANSACTIONS_IMPL.md` matched what the bench surfaced + what the
fix removed. Keep that discipline.

The 2025-05-10 review pass on this doc itself is the same discipline
applied **to the predictions**: where measured numbers (sled bench
in `PERF_BASELINE.md`) showed the original win estimates were off,
the estimates here are corrected. Where new items emerged from the
measurement (Opt O, P, Q1, Q2, R), they're added.
