# Performance Journal

Хронологический журнал всех `/opti` циклов оптимизации ShamirDB.
Каждая запись — один (или несколько) atomic commit'ов с измеримым
до/после или явно отмеченным "investigated, no-win". Файл нужен
чтобы не ходить по второму кругу через те же гипотезы — если что-то
помечено "reverted" или "no-easy-win", смотрим что было раньше до
повторных попыток.

Замеры — на машине разработчика под Windows 10 + mimalloc. Цифры
могут смещаться на другой платформе/allocator'е, но **порядок** и
**характер** wins должны сохраняться.

---

## Методология

- `BENCH_QUICK=1 cargo bench` — quick criterion mode (`bench-quick`
  custom flag в проекте, фильтр sample_size + measurement_time).
- Все бенчи в `crates/<crate>/benches/*.rs`. После cycle — bench
  остаётся в репо для regression-detection.
- `/opti` skill (`.claude/skills/opti/SKILL.md`) — cycle: baseline
  bench → optim → tests → after bench → commit. Revert если не лучше.
- Workspace-wide test suite (`cargo test --workspace --lib`) — gate
  на каждый коммит.

---

## Сводная таблица wins

| # | Commit | Область | До | После | Δ |
|---|--------|---------|-----|-------|---|
| 1 | `bc28a00` | RecordId thread_rng | 226 ns | 144 ns | **1.57×** |
| 2 | `91883fd` | Interner ArcSwap (reverse) | — | — | **3.6×** на 4 потоках |
| 3 | `a395552` | WAL list_inflight batch 64→1024 | — | — | recovery faster |
| 4 | `c9f5de1` | MemBuffer batched byte-cap eviction | — | — | **1.81×** под pressure |
| 5 | `38e69ac` | MemBuffer bounded TTL sweep | — | — | **3.17×** под pressure |
| 6 | `57198d2` | MemBuffer → moka | 632 K/s | 1959 K/s | **3.23×** concurrent |
| 7 | `40b87a2` | MemBuffer skip inner.get probe | — | — | **-24%** byte-pressure |
| 8 | `cf56c2a` | MemBuffer dirty owns values | 6.6 ms | 3.23 ms | **-12%** (closes regression) |
| 9 | `8164582` | msgpack encode — drop rmpv tree | 13.09 ms | 4.38 ms | **2.99×** |
| 10 | `d1e49ca` | HAVING — json::Value direct walk | 13.57 ms | 8.18 ms | **1.66×** |
| 11 | `655ba4c` | Filter resolve_field borrow | 243 µs / 698 µs | 195 / 573 µs | **1.22–1.25×** |
| 12 | `5f1e793` | GROUP BY typed key + lazy json | 1.37–6.07 ms | 0.55–1.29 ms | **2.5–4.7×** |
| 13 | `caacf03` | Session/RateLimiter SipHash→FxHash | 102 ns / 62 ns | 53 ns / 32 ns | **~1.94×** |
| 14 | `4448909` | Filter pre-resolve literals | 160 µs / 404 µs | 137 µs / 330 µs | **~1.2×** |
| 15 | `49b41c1` | DISTINCT structural hash | 6.69 / 7.41 ms | 4.04 / 4.94 ms | **1.5–1.7×** |
| 16 | `30b7947` | json encode — drop json::Value tree | 11.65 ms | 3.57 ms | **3.27×** |
| 17 | `2f877af` | Index batch borrow + reuse key | — | — | refactor, expected O(N·I) alloc removed |
| 18 | `db89eb3` | posting_cache Mutex→DashMap | — | — | concurrent refactor |
| 19 | `84dac1d` | Session HMAC OnceLock cache | 134 ns | 12 ns | **11.08×** |
| 20 | `8ed07e7` | Session permissions RwLock dropped | — | — | clean-up (dead lock) |
| 21 | `42720a4` | SelectProjection borrow + pre-keys | 7.32 / 10.45 ms | 2.39 / 3.34 ms | **3.06–3.13×** |
| 22 | `aa0774b` | Sorted index borrow walk | — | — | устранён полный record.clone() per extract |
| 23 | `2df9650` | mimalloc global allocator | 578 ns | 294 ns | **1.96×** RPS |
| 24 | `ecffd54` | Interner Borrow<str> на UserKey | ~340 ns | ~142 ns | **2.2×** touch_ind |
| 25 | `749b47a` | FilterNode enum dispatch | ~parity | ~parity | refactor (win на complex trees) |
| 26 | `3c294a0` | `#[inline]` hints | — | — | hint cross-crate |

**Bench-only commits** (нет optim, фиксируют baseline или ловят
regression): `a86f924`, `dd86e85`, `12df89b`, `67db1ed`, `fae0b2e`,
`d9c632f`.

---

## Группы оптимизаций

### Storage / MemBuffer

- **moka migration** (`57198d2`, `40b87a2`, `cf56c2a`): шардированный
  W-TinyLFU + LRU cache. Concurrent reads — lock-free. Финальная
  архитектура: `cache: ArcSwap<moka::Cache>` (read accelerator) +
  `dirty: DashMap<RecordKey, Slot>` (write-back buffer, owns values).
  Listener убран — moka может evict freely.
- **WAL list_inflight batch 64→1024** (`a395552`): recovery scan
  amortise — fewer Vec/await per batch.
- **Eviction loops bounded** (`c9f5de1`, `38e69ac`): batched
  byte-cap + TTL sweep вместо per-iter scan.

### Codec (encode hot path)

- **msgpack** (`8164582`): `inner_to_msgpack` writes via direct
  serde stream over `InternedRef<'a>` wrapper. Никакой `rmpv::Value`
  tree (две прохода → один).
- **json** (`30b7947`): тот же приём для `inner_to_json`.
- **HAVING** (`d1e49ca`): `json_value_to_inner` walks `json::Value`
  прямо в `InnerValue`. Раньше делал `serde_json::to_vec` + parse +
  walk — лишний round-trip.

### Query pipeline

- **GROUP BY** (`5f1e793`): typed `GroupKeyItem` enum как ключ
  IndexMap (FxHash), lazy `inner_to_json_value` через `Entry::Vacant`,
  `sort_keys()` in-place. Композитный GROUP BY на 1000 records — **4.7×**.
- **DISTINCT** (`49b41c1`): structural Hash для `json::Value` через
  `HashableJson` wrapper + FxHash. Раньше `record.to_string()` per row.
- **SelectProjection** (`42720a4`): borrow leaves через
  `resolve_field_ref`, pre-built output keys (alias или last segment)
  в `SelectProjection::new`.
- **OrderBy DSU precompute** (`d9c632f`): tried, no win, reverted.
  Sort comparator dominated, not the BTreeMap lookups.

### Filter eval

- **resolve_field_ref** (`655ba4c`): borrow leaf — устранён clone
  per record для 12 callbacks.
- **pre-resolve literals** (`4448909`): `Option<InnerValue>` cached
  в Compare/Contains/Between. RHS String/Binary clone убран с hot loop.
- **FilterNode enum dispatch** (`749b47a`): single enum, `match`
  dispatch вместо `Box<dyn FilterCallback>` per node. Architectural.

### Index manager

- **batch insert borrow + key reuse** (`2f877af`):
  `extract_value_by_path_ref` (вариант к filter borrow), reuse
  computed `index_key: Bytes` для invalidation (раньше recomputed).
- **posting_cache** (`db89eb3`): `Mutex<HashMap>` → `DashMap` для
  concurrent lookups.
- **sorted index borrow walk** (`aa0774b`): `resolve_path` начинался
  с `record.clone()` — полный deep clone tree per extract per
  sorted-index. Заменён на iterative borrow walk.

### Session / connect

- **HMAC OnceLock cache** (`84dac1d`): `Session::hmac_key()` — был
  SHA256 per call. Pre-fill в `SessionStore::insert` после stamping
  `session_id`. **11.08×**.
- **SipHash → FxHasher** (`caacf03`): SessionStore (`[u8;32]` keys),
  RateLimiter (`Subnet [u8;3..8]` keys). DoS resistance уже upstream.
- **permissions RwLock dropped** (`8ed07e7`): repo-wide grep
  показал zero writes — dead lock, замена на bare `SessionPermissions`.

### Interner

- **ArcSwap reverse map** (`91883fd`): `id → UserKey` — lock-free
  reads. 3.6× под нагрузкой.
- **Borrow<str> на UserKey** (`ecffd54`): `touch_ind`/`get_ind`
  больше не делает `UserKey::from_str(s)` для каждого lookup. DashMap
  honors `Borrow<str>`. **2.2×** на cache-hit path.

### Allocator

- **mimalloc** (`2df9650`): `#[global_allocator]` в `shamir-server`
  bin + `db_handler_rps` bench. db_handler/ping_inprocess: 578 → 294
  ns/req. Hot path алло-зависимый (msgpack codec), mimalloc free-list
  быстрее Windows HeapAlloc.

### Misc

- **record_id** (`bc28a00`): `OsRng` → `thread_rng` (1.57×).
- **`#[inline]`** (`3c294a0`): hints на маленьких leaf functions
  (compare_values, resolve_field_ref, filter_value_to_inner). LTO=false
  в bench profile — explicit hint помогает cross-crate inlining.

---

## Investigated — no win (документировано чтоб не возвращаться)

| Гипотеза | Commit / cycle | Почему отвергнут |
|----------|----------------|------------------|
| Arc<str> в Interner reverse vec | **reverted in #43** | +55% регресс. mimalloc делает String alloc дешёвым (~10 ns), Arc<str>::clone не быстрее в этом контексте. ArcSwap::load + Arc::clone суммарно проиграл. |
| MemBuffer dirty SipHash→FxHash | **reverted in #24** | Mixed bench (membuffer_concurrent_rw /2 регресс +12%, /8 win -7%). RecordKey hash не на критическом пути такой природы как fixed-size auth keys. |
| OrderBy DSU precompute | **reverted in #35** | `pdqsort` comparator уже эффективен; saved BTreeMap lookups cancel с indirection через perm[] + bookkeeping. |
| `Slot` enum size в MemBuffer | **#46 closed** | `Option<Bytes>` тот же 40 bytes (нет niche optim — `Bytes::ptr` raw `*const`). Tagged-pointer unsafe. |
| `UnixNanos::now()` × 3 в handshake | **#47 closed** | 300 ns vs 3 sec Argon2 = 0.00001%. Аргон ограничен семафором ~10/sec. |
| Filter compile cache LRU | **#48 closed** | compile_filter ~1-10 µs / query — не доминирует. Cache invalidation при interner state change — complex. |
| RecordKey Bytes inline (Arc<[u8;16]>) | **#49 closed** | `Bytes` crate не поддерживает inline storage для 16-byte buffers — always heap. RecordId-as-Bytes — big refactor. |
| Backend durability tuning | **#54 closed** | redb уже использует `Durability::None`; sled — flush_every_ms default. Уже оптимизировано. |
| bincode 1.x → 2.x | пропущен в Sentry разведке | Используется только для index metadata (admin path), не query hot path. ROI низкий. |
| HMAC key cache в db_handler local Option | заменён на #33 OnceLock | Лучшее решение через Session field. |
| Encode buffer reuse в BytesMut pool | **#32 closed** | Текущий паттерн `encode → Bytes::from(Vec<u8>)` (move). Reuse требовал бы `Bytes::copy_from_slice` — добавляет memcpy. Net regression. |

---

## Deferred — большие архитектурные

| Задача | Причина откладывания |
|--------|----------------------|
| **Stream-based query results** (#51) | Меняет QueryResult, batch executor, dispatch, ResponseEnvelope. Win зависит от размера result; на типичных 100-1000 records — minor. |
| **Batch plan cache** (#52) | Overlap с filter compile cache (отвергнут). Compile cost не доминирует. Cache invalidation сложна. |
| **CPU profiling samply/flamegraph** (#53) | Инструмент, не optim. Должно делаться под production-like workload — без него signal-to-noise низкий. |
| **SIMD compare** (#55) | `std::str::cmp` уже использует memcmp (LLVM авто-vectorise для длинных строк). Для коротких (<32 bytes) SIMD не помогает. Нет signal что нужно. |
| **Codec negotiation postcard/cbor** (#56) | Wire protocol breaking change. Versioning, client compat, security review. msgpack уже 3× прямой stream. |
| **Backends compare bench** (#37) | InMemoryStore — единственный с direct `Store::new()`. Остальные backends через `Repo::store_get(name)`. Требует Repo-wiring setup. |
| **WAL fsync batching / group commit** (#38) | Многодневная архитектурная работа: bench с durable backend, implement group commit window, trade-off durability ↔ throughput. |
| **Tokio runtime tuning** (#40) | Defaults обычно OK. Требует production-like load + tokio-console для observation. |

---

## Инфраструктурные коммиты (нет /opti win, но enabling)

- `7ae4a46` — `/opti` skill (local skill для cycle bench→opt→test→bench→commit).
- `0679bd7` — `[profile.bench]` без LTO + `BENCH_QUICK` runtime mode. **15×** faster bench iteration (compile-time).
- `a86f924`, `dd86e85`, `12df89b`, `67db1ed` — micro-benches
  (cache_hit_get, steady_state_insert_10k, wal_high_qps,
  membuffer_concurrent_rw) для regression detection.
- `fae0b2e` — `db_handler_rps` in-process bench (упрощённый RPS upper
  bound, без TLS/TCP/Argon2). Baseline 1.66 → 3.40 Mreq/s после mimalloc.

---

## Структура bench-файлов

```
crates/
├── shamir-types/benches/
│   ├── record_id.rs               — RecordId::new()
│   ├── codec_msgpack.rs           — inner_to_msgpack
│   ├── codec_json_encode.rs       — inner_to_json
│   └── codec_json_roundtrip.rs    — to_vec+parse vs direct walk
├── shamir-engine/benches/
│   ├── wal_recovery.rs            — list_inflight scan
│   ├── interner_concurrent.rs     — touch_ind / get_str под потоками
│   ├── membuffer_concurrent.rs    — concurrent_rw 1/2/4/8 readers
│   ├── filter_eval.rs             — Eq на 1000 records, top-level + nested
│   ├── group_by_keys.rs           — apply_group_by 1000 records
│   ├── distinct.rs                — apply_distinct unique + half-dup
│   └── select_pipeline.rs         — apply_select + apply_order_by
├── shamir-db/benches/
│   └── engine_perf.rs             — full-engine paths (read/set/index)
├── shamir-server/benches/
│   └── db_handler_rps.rs          — ShamirDbHandler::handle in-process RPS
└── shamir-connect/benches/
    └── hot_paths.rs               — dispatch / session_store / envelope / crypto
```

---

## Workflow для следующих циклов

1. Сначала глянуть в этот файл — гипотеза уже отвергнута?
2. Если нет — добавить новую таску, прогнать /opti цикл.
3. На win — обновить **сводную таблицу** + **группу** в этом
   journal'е. На no-win — добавить в **investigated**.
4. На большой архитектурный — записать в **deferred** с
   мотивацией. Не делать, пока нет profiler-signal под production
   workload.
