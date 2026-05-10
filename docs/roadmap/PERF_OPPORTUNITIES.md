# Performance Opportunities — Beyond Asymptotic Wins

Status: **planned/ideated, no code yet.** Companion to
`docs/ops/PERF_BASELINE.md` (which captures measured numbers and the
A → B → C → D round of asymptotic optimisations).

Where A/B/C cut O(n) → O(log n) on the write path's hot scenarios
(set/update/delete via index — 800-1100× wins), this document is
about the **next class** — reducing the per-record constant factor.
No single item here is a 1000× spike; cumulatively they should give
3-10× across the whole read/write profile, on every workload.

These are observations from a fresh re-walk of the codebase after the
A-D sprint. The lens: «что лишнее тут делается каждую миллисекунду» —
because next-class wins come from removal, not addition.

---

## Слепок по-русски

### Что вижу как структурную трату

#### 1. Async ceremony там где работа синхронная

Каждая операция в `Store` trait объявлена `async fn`. Для
in-memory backend — `DashMap.insert` чистый синхронный. Для redb —
синхронный с маленькой блокировкой. Но snapshot всех вызовов
проходит через tokio state machine: future, waker, poll. На `set`
нет ни одного `await` внутри backend'а — есть только async-обёртка
вокруг sync работы.

Эта церемония стоит **сотни наносекунд на вызов**. На горячем пути
с 100K op/s — заметные проценты.

#### 2. Allocation pressure повсюду

Каждое чтение проходит через `Bytes` (heap-allocated ref-counted
slice), которое потом дешифруется в новый `Vec<u8>` и из него — в
`InnerValue` с `HashMap` для каждого Map'а. Запись 5-полевая =
~6 heap-allocations.

На 1000 records read = ~6000 allocations + GC pressure. Аллокаторы
в Rust быстрые, но не бесплатные.

Подсказка какие у нас типичные records: маленькие, плоские,
≤8 полей. Идеально для **inline storage** — `SmallVec<[(K, V); 8]>`
вместо HashMap, `[u8; 16]` inline вместо Bytes для коротких ключей.

#### 3. Interner reverse-lookup на каждом field-resolve

Read возвращает 1000 records. Каждый record — 5 полей. Каждое поле —
`inner_to_json_value(value, interner)` → `interner.get_string(key_id)`
на каждом ключе. **5000 DashMap lookups** на один read query, при
том что `key_id → String` — это просто массив (interner монотонный,
никогда не сжимается).

Решение очевидное: `Vec<String>` индексированный по u64 для reverse
lookup. **O(1) array index** вместо hashmap. Forward map
(`String → u64`) можно оставить DashMap — он только на write.

Копеечная правка, но 5000× ускорение конкретного hot inner loop.

#### 4. Index хранится как монолитный bincoded BTreeSet

`lookup_by_index(idx_name, value)` лезет в info_store, читает ВЕСЬ
BTreeSet matching docs, десериализует. Для индекса по `city`, при
равномерном распределении 10K records / 8 cities = 1250 ids per
city = ~20KB blob. **На каждый lookup — 20KB парсинг**.

Решение: **in-memory index cache.** LRU mapping
`(table, idx, value) → BTreeSet<RecordId>`. Read из info_store
происходит один раз, дальше — память. Invalidate на write по тому
же ключу. Один-два MB кеша покрывают 99% query workloads.

#### 5. Filter evaluation через vtable

`compile_filter` возвращает `Box<dyn FilterCallback>`. На каждый из
10 000 records — virtual call `cb.matches(&record, ctx)`. Inner loop
горячий — vtable lookup + indirect call вместо inlined comparison.

Для 90% запросов filter — это `Eq { field, value }` или `And` of
`Eq`'s. Для них vtable не нужен. **Specialized fast paths** для
top-3 filter shapes — direct call, inlinable.

Не 1000×, но 30-50% на простых запросах.

#### 6. Persist amplification — паттерн который никуда не делся

Уже исправили для interner (Opt A). Сейчас **тот же паттерн
возникает для counter** (вижу в working tree — `counter.persist()`
добавлено в insert/update/delete). И **тот же паттерн будет
возникать снова и снова** для каждого нового metadata-state'а
который БД захочет durable.

Это структурная проблема, не точечная. Решение — **общий
"dirty-flag flush" механизм**: trait/struct `Persistable` с
методами `mark_dirty()` + `persist_if_dirty()`. Один backend
timer / batch-end hook вызывает `persist_if_dirty()` на всех
зарегистрированных. Никто не должен помнить про persist по месту.

#### 7. Lazy materialization отсутствует

`SELECT email FROM users LIMIT 100` сейчас:

1. Full scan all records (или index)
2. Полная десериализация bytes → InnerValue для **всех** полей
   каждой записи
3. `inner_to_json_value` всей структуры
4. **Потом** `apply_select` отбрасывает все поля кроме `email`

Мы потратили работу на 95% полей которые потом выкинули.

Решение: `SelectProjection` определяется до материализации, и
`inner_to_json_value` принимает projection mask, читает только
нужные поля. Подобные partial-decode оптимизации — стандартные в
современных БД (Postgres, ClickHouse).

Win зависит от схемы — для записи на 10 полей с проекцией 1:
~10× меньше работы codec'а.

#### 8. Bincode vs zero-copy

Каждый `data_store.get(key)` → bytes → bincode::from_bytes →
InnerValue. Аллокации, копирование. Bincode — **decent but not fast**.

`rkyv` — zero-copy serialization. На write пишем archived bytes. На
read — `unsafe { rkyv::archived_root<...>(bytes) }` даёт **ссылку**
прямо в bytes без копирования. Чтение поля — pointer arithmetic.

Цена: rkyv требует специфичного derive, формат жёстче. Cost-benefit
для нашей schema-less InnerValue — sloppy. Может оправдаться для
**system records** (interner state, counter, index meta) где shape
известен.

#### 9. RecordId::new — три syscall'а на insert

```rust
let now_micros = Utc::now().timestamp_micros();    // syscall (Linux)
rand::rngs::OsRng.fill_bytes(&mut bytes[8..16]);    // syscall
```

На bulk insert 1000 = 2000 syscall'ов **до даже одного storage call**.

Решение: **batch RecordId allocator**. Один `now()` + один
`OsRng.fill_bytes(&mut [u8; 16384])` на 1024 ids. Lazy раздача из
pool. Bulk insert на 1000 = 2 syscall'а.

#### 10. JSON parsing на каждом execute()

Wire payload — msgpack. Сервер декодирует в DbRequest. Внутри —
`BatchRequest` уже DTO. Но затем для каждой query value мы
конвертируем `serde_json::Value` → `InnerValue` через recursive walk.

Если те же query JSON-shapes приходят повторно (typical для admin
UI), мы парсим re-парсим. **Prepared statements** — query template +
parameters. Парс плана раз, params подставляются.

Win зависит от стационарности workload. Для UI бэкенда c
фиксированным набором запросов — 10-30%.

### Один взгляд под другим углом

Если посмотреть на эти 10 пунктов одной фразой — **БД выполняет
много "церемониальной" работы для каждого record'а на каждом
проходе.** Async wrapping синхронной работы. Reverse lookup
интернера через хешмап вместо массива. Пере-десериализация bincode'а.
Проекция после материализации. Persist каждой metadata-копейки на
каждое write.

Каждый пункт по отдельности — 5-30%. Но сумма этих 30%-х =
**3-10× cumulative**. И это на всём, не только на specific scenario.

Это **другой класс оптимизаций чем A/B/C** (которые были про
асимптотику конкретного hot path). Это — **снижение per-record
constant factor**. Не 1000× spike, но 5× по всему профилю.

И, важное: эти win'ы **не требуют новых фич**. Это чистка того что
уже есть. Никакого нового API surface, никакого нового type.
Только аккуратная переработка hot path.

---

## Per-item details (English)

Letters continue from A/B/C/D used in `PERF_BASELINE.md`.

### Opt F — interner reverse-lookup as `Vec<String>`

**File:** `crates/shamir-types/src/core/interner/*`

**Symptom.** Every read materialisation calls
`interner.get_string(key_id)` per field, per record. The reverse
mapping is a `DashMap<u64, String>` — hash + lookup + Arc clone for
each call. For 1000 records × 5 fields → 5000 hashmap lookups per
read query.

**Fix.** Interner is monotonic (keys never removed). Use
`Vec<String>` indexed by `u64` for the reverse direction; forward
direction (`String → u64`) stays a `DashMap` (only touched on write).

```rust
struct Interner {
    forward: DashMap<String, u64>,            // write path
    reverse: parking_lot::RwLock<Vec<String>>, // read path, append-only
}
```

`get_string(id)` becomes `reverse.read()[id as usize].clone()`
(or return `&str` if we can plumb lifetimes — even better).

**Effort.** 1-2 hours. Small change in `core/interner/`, no API
surface change.

**Win estimate.** 5-50% of full-read query latency depending on
record-field count.

### Opt G — in-memory LRU cache for index posting lists

**File:** `crates/shamir-engine/src/index/index_manager.rs`

**Symptom.** `lookup_by_index(name, values)` reads the entire
posting blob from `info_store`, deserialises a `BTreeSet<RecordId>`,
returns. For a city-index with 1250 docs per value, that's a ~20 KB
parse per lookup — and a hot one (called from every indexed
read/update/delete).

**Fix.** Per-`IndexManager` instance, keep an LRU map:

```rust
type CacheKey = (u64 /* index_name_interned */, Vec<InnerValue> /* values */);
struct IndexCache {
    entries: lru::LruCache<CacheKey, BTreeSet<RecordId>>,
    capacity_bytes: usize,
}
```

Look up in cache first; on miss, fall back to `info_store.get`,
populate cache. Write path invalidates affected `(index, value)`
entries on `on_record_created/updated/deleted`.

Capacity tuning: ~1-2 MB per repo by default; configurable.

**Effort.** 1 day, including invalidation correctness tests.

**Win estimate.** 5-30× on repeated indexed lookups (which dominate
read-heavy workloads). Cold cache: same as today.

### Opt H — counter persist debouncing (recurrence of Opt A pattern)

**File:** `crates/shamir-engine/src/table/record_counter.rs` and
`write_exec.rs`

**Symptom.** Working tree currently shows `counter.persist().await`
calls added after every insert/update/delete — same write
amplification we just fixed for interner (Opt A). Counter blob is
small (8 bytes), but the `info_store.set` round-trip costs
async + serialisation regardless of payload size.

**Fix.** Same trick as Opt A — track `last_persisted_count: AtomicU64`,
`persist()` no-ops when current value equals it. Or better: integrate
with Opt H₂ (`Persistable` trait) below.

**Effort.** 30 minutes for the standalone fix.

**Win estimate.** 5-10% on write-heavy benchmarks (matches the gain
Opt A gave).

### Opt H₂ — generic `Persistable` mechanism (eliminate the recurrence)

**File:** new module in `shamir-engine`

**Symptom.** Persist amplification is a *pattern*, not a one-off. We
fixed it for interner; now it surfaced for counter; it'll surface
again for every metadata blob we add (index_meta, audit-tail,
fts-totals, vector-graph stats…).

**Fix.** Single mechanism every persistable metadata uses:

```rust
pub trait Persistable: Send + Sync {
    fn name(&self) -> &str;
    /// Return true if state changed since last persist.
    fn is_dirty(&self) -> bool;
    /// Write to durable storage and clear dirty.
    async fn persist(&self) -> DbResult<()>;
}

pub struct PersistRegistry { items: Vec<Arc<dyn Persistable>> }

impl PersistRegistry {
    /// Called at end of every batch (or by background timer).
    pub async fn flush_dirty(&self) -> DbResult<()>;
}
```

Every metadata holder registers itself. End-of-batch hook in
executor calls `flush_dirty()` once. No more per-op `.persist().await`
sprinkled across the code.

**Effort.** ~1 day including migrating interner + counter + index
meta to use it.

**Win estimate.** 5-15% on write workloads + protection against
future regressions.

### Opt I — sync `Store` API + async wrapper

**File:** `crates/shamir-storage/src/types.rs` + all backend impls

**Symptom.** `Store::get/set/insert/remove` all `async fn`, but
in-memory backend is pure-sync DashMap and redb is sync-with-mutex.
Each call costs a tokio state-machine + waker-plumbing yield (~100
ns) for nothing.

**Fix.** Reshape `Store` to have sync core + optional async wrapper:

```rust
pub trait Store: Send + Sync {
    // sync core
    fn get_sync(&self, key: &[u8]) -> DbResult<Option<Bytes>>;
    fn set_sync(&self, key: Bytes, value: Bytes) -> DbResult<bool>;
    // ...

    // async wrapper — default impl wraps sync; backends with real
    // I/O (sled mmap, fjall LSM compaction) override to spawn_blocking
    async fn get(&self, key: Bytes) -> DbResult<Bytes> { ... }
}
```

Engine on read path calls sync core when not crossing thread
boundaries. Backends that need true async (network-attached future
backends?) override.

**Effort.** 2-3 days — touches every backend impl + engine call sites.
Mechanical but wide.

**Win estimate.** 20-40% on in-memory backend; 5-15% on disk
backends (still saves the spawn_blocking hop for cached reads).

### Opt J — inline `SmallMap` for small records

**File:** `crates/shamir-types/src/types/value.rs` (`InnerValue::Map`)

**Symptom.** `InnerValue::Map` wraps `HashMap`-like structure. For
typical record (5-8 fields) every Map is a heap allocation +
HashMap overhead (capacity 16, fill factor wastage). On a read of
1000 records → 1000 HashMap allocations.

**Fix.** Use a stack-or-heap structure: inline `SmallVec<[(K, V); 8]>`
for ≤8 entries (no allocation, linear scan), spill to HashMap above.
For records of typical width — zero map allocations.

**Effort.** 2-3 days. The Map is a foundational type so changes ripple,
but the API can stay shape-compatible.

**Win estimate.** 10-30% allocation reduction on read-heavy
workloads, GC pressure drops correspondingly.

### Opt K — projection-aware lazy materialisation

**File:** `crates/shamir-engine/src/query/read/exec.rs::apply_select`
+ `shamir-types/src/codecs/interned/`

**Symptom.** `SELECT email FROM users LIMIT 100` materialises every
field of every matched record then drops 95 % of the result during
projection. We did the work and threw it out.

**Fix.** Two-step:

1. Plan the `SelectProjection` *before* materialisation — extract
   the set of needed field paths.
2. `inner_to_json_value_partial(value, projection, interner)` walks
   only the requested paths, leaves the rest as raw bytes / unread.

Bonus: the same projection mask can flow into the `Store::get`
layer for backends that support partial-record reads (none today,
but architecturally clean).

**Effort.** 3-5 days. Touches the codec module + read executor.

**Win estimate.** 5-10× for projections of 1-2 fields out of
8-10 wide records. Depends on workload.

### Opt L — batch `RecordId` allocator

**File:** `crates/shamir-types/src/types/record_id.rs`

**Symptom.** `RecordId::new()` does one `Utc::now()` syscall + one
`OsRng.fill_bytes(8)` call per id. Bulk insert 1000 → 2000 syscalls
before a single storage write.

**Fix.** Per-thread `RecordIdAllocator`:

```rust
struct RecordIdAllocator {
    timestamp_base: i64,           // captured once
    random_pool: [u8; 16384],      // 1024 ids worth
    cursor: usize,
}
```

`new()` returns `RecordId { timestamp_base + delta, &random_pool[cursor..]} `.
Refill when cursor exhausts.

For sub-microsecond ordering across allocations within the same
batch: `delta` increments per-call (still timestamped, monotonic
within the burst).

**Effort.** 1-2 hours.

**Win estimate.** 5-15% on bulk insert throughput; bigger relative
gain on Linux where syscalls cost more.

### Opt M — specialise hot filter shapes

**File:** `crates/shamir-engine/src/query/filter/eval.rs::compile_filter`

**Symptom.** Every filter compiled to `Box<dyn FilterCallback>` →
`vtable lookup + indirect call` per record. For 10K records the
pointer-chasing dominates simple Eq filters.

**Fix.** Recognise the 3-4 most common shapes and return a non-boxed
specialised closure inlined at call site. For complex shapes fall back
to Box. For example:

```rust
pub enum CompiledFilter {
    Eq { field_path: Vec<u64>, value: InnerValue },     // most common
    AndOfEq(Vec<(Vec<u64>, InnerValue)>),                // second most
    Custom(Box<dyn FilterCallback>),                     // fallback
}

impl CompiledFilter {
    #[inline]
    pub fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool {
        match self {
            Self::Eq { field_path, value } => /* inlined */,
            Self::AndOfEq(parts) => /* inlined */,
            Self::Custom(b) => b.matches(record, ctx),
        }
    }
}
```

**Effort.** 1 day.

**Win estimate.** 30-50 % on simple-filter scans.

### Opt N — prepared-query plan cache (mentioned, lowest priority)

**File:** would be new in `shamir-engine::query::batch`

**Symptom.** Same JSON shape repeatedly arriving: `{"from":"users","where":{"op":"eq",...}}`.
Each time: full JSON tree walk → `BatchRequest` → `BatchPlanner` →
filter compile → execute. Plan/parse cost is shared across executions
of "the same query with different parameters".

**Fix.** Hash the query AST minus parameters; cache compiled plan +
filter callback. Reuse on cache hit, substituting parameters.

**Effort.** 1 week (parameter detection requires careful AST
analysis).

**Win estimate.** 10-30 % on UI-style stationary workloads with
repeated query shapes; near zero on ad-hoc queries.

---

## Recommended sprint order

A pragmatic batching of the items above into shippable cycles:

### Sprint α — gardener pass (3-4 hours total)

Pure cleanup, 30%-class wins, recurring-pattern protection:

1. **Opt H** — counter persist debouncing (currently in-flight in
   working tree; finish the same way Opt A handled interner).
2. **Opt H₂** — extract `Persistable` trait so the recurrence stops
   here. Migrate interner + counter onto it.
3. **Opt F** — interner reverse-lookup as `Vec<String>`.
4. **Opt L** — batch RecordId allocator.

Re-run `engine_perf.rs`, document deltas in `PERF_BASELINE.md`.

### Sprint β — index cache (1-2 days)

5. **Opt G** — in-memory LRU cache for posting lists with proper
   invalidation hooks.

This is the single most impactful item left for read-heavy workloads
where the same indexed lookups repeat (admin UIs, dashboards).

### Sprint γ — async/sync layering (3-4 days)

6. **Opt I** — sync `Store` core + async wrapper; engine hot paths
   stop paying tokio state-machine cost for synchronous backends.

This is wide but mechanical — biggest CPU win without changing
algorithmic shape.

### Sprint δ — projection (4-5 days)

7. **Opt K** — projection-aware lazy materialisation. Largest impact
   on workloads with narrow `SELECT` clauses over wide records.

### Sprint ε — fast-path filters + small maps (3-4 days)

8. **Opt M** — specialised compiled-filter variants for top-3
   shapes.
9. **Opt J** — inline SmallMap for ≤8-field records.

### Later, conditional

10. **Opt N** — prepared-query plan cache. Only if profiling against
    a real UI workload shows query-parse cost dominating.
11. **rkyv for system records** — zero-copy reads of interner /
    index meta. Specialised, opt-in, after we know the shape is
    stable.

Total Sprint α-ε: roughly 3 weeks of focused work for a cumulative
3-10× across the entire profile (not concentrated on one scenario
like A/B/C/D were).

---

## What we deliberately skip — and why

- **Static dispatch through generics over backend.** Refactor of
  2-3 weeks for a 10-15 % win. Not worth it while simpler wins are
  on the table.
- **rkyv for `InnerValue` itself** (not just system records). Format
  lock-in, big test surface, ~2-3× win. Wait.
- **SIMD for filter evaluation.** Niche (large numeric scans), not
  our typical workload. Reconsider if column-oriented analytics
  becomes a use case.
- **Custom binary protocol on the wire** instead of msgpack. msgpack
  is good enough; replacement complexity not justified.

---

## Ground truth before each item

A reminder: every item above is *expected* — based on reading code
and structural reasoning. Before committing time to any of them,
**run `engine_perf.rs` first**, profile (`flamegraph` /
`samply`) the actual hot scenario, and confirm the symptom matches
the prediction.

The A/B/C results validated this loop: predictions in
`TRANSACTIONS_IMPL.md` matched what the bench surfaced + what the
fix removed. Keep that discipline.
