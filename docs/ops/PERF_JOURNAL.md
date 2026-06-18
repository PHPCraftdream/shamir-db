# Performance Journal

Полный хронологический журнал всех `/opti` циклов ShamirDB —
**77 perf/bench коммитов через 6 раундов** (2026-05-08 → 2026-05-17).

Файл нужен чтобы не ходить по второму кругу. Если гипотеза помечена
"reverted" или "no-easy-win", смотрим **почему** до повторных попыток.

Замеры — на машине разработчика под Windows 10 + mimalloc (с раунда
6). Цифры могут смещаться, но **порядок** и **характер** wins
сохраняются.

---

## Раунды (chronological)

| Раунд | Даты | Тема | Commits |
|-------|------|------|---------|
| **R0** | 2026-05-08 | shamir-connect / transport-tcp hot paths | 10 |
| **R1** | 2026-05-10–11 | Engine algorithmic wins + storage backends | ~17 |
| **R2** | 2026-05-11–12 | MemBuffer feature build + tuning | ~10 |
| **R3** | 2026-05-11 | Bench infra + record_id + interner + WAL | ~6 |
| **R4** | 2026-05-15 | Query path + codecs | ~10 |
| **R5** | 2026-05-15–16 | Session/connect + index/select + allocator | ~10 |
| **R6** | 2026-05-16–17 | Interner Borrow / FilterNode enum / inlining | ~5 |

---

## R0 — shamir-connect / transport-tcp (Optim #1–10)

Один последовательный round сетевых hot paths. Связан с
`docs/ops/PERF_OPTIMIZATIONS.md` (full bench numbers).

| Commit | Сделано |
|--------|---------|
| `edb7e76` | `read_frame_into` — pooled buffer + skip zero-fill (unsafe set_len после reserve, безопасно потому что read_exact fills) |
| `9578c55` | TicketPlain fixed-size fields → `serde_bytes::ByteArray<N>` (no heap Vec) |
| `c5d121e` | Cache pre-scheduled Aes256Gcm ciphers в ResumeConfig (не пере-schedule на каждый resume) |
| `02bcfbd` | `RequestEnvelopeView` + `dispatch_request_view` — zero-copy server dispatch |
| `8073740` | `touch_at` + `lookup_at` + lock-free version check (caller-supplied timestamp, экономия 1 UnixNanos::now() per request) |
| `9963acd` | Resume: move plain fields instead of cloning (Optim #6) |
| `a4f7f4e` | Framing: single `write_all` + `write_frame_into` pooled variant |
| `3dead97` | Ticket: in-place AES-GCM decrypt (zero-copy decrypt) |
| `e0683d0` | `RequestEnvelopeRef<'a>` — zero-copy client encode |
| `27df64b`, `00ade40` | docs: PERF_OPTIMIZATIONS.md updates |

Затрагивает: framing (TCP), envelope (msgpack), session_store
(DashMap), ticket (AES-GCM), resumption.

---

## R1 — Engine + storage backends (opt-A..opt-Q)

Большой round алгоритмических wins. Многие — **порядки величины**
(818×, 1115×, 3383×) потому что заменяет full-scan на index lookup.

| Commit | Имя | Win |
|--------|-----|-----|
| `ee9b03b` | Criterion bench suite + baseline | (enabling) |
| `a7d7a05` | **opt-A** — debounce interner persist | −10% bulk insert |
| `3bb20c9` | **opt-B** — `execute_set` uses single-field index | **818×** |
| `3769483` | **opt-C** — `execute_update` / `execute_delete` через read planner | **1115×** |
| `f8552a0` | **opt-D-#2** — `SELECT count(*)` → `RecordCounter` | **3383×** at N=10K |
| `66d7b5d` | **opt-D-#2.5** — `count(*) WHERE indexed_eq` = `BTreeSet::len()` | **22×** at N=10K |
| `a3013c7` | **opt-D-class** — counter cache + revert parallel-stages | bulk_insert −29% |
| `7afe259` | **opt-Q1** — `MIN(field)` fast-path via sorted-index `lookup_min` | **22×** at N=10K |
| `bc60476` | **opt-Q2** — `Filter::Gt` / `Lt` routed через sorted-index planner | (algorithmic) |
| `a6abe93` | **opt-F** — interner reverse-lookup as `Vec` | 10–16% wide read-heavy |
| `c8abbd7` | **opt-G** — in-memory cache for index posting lists | **4.3×** count-with-filter |
| `3158390` | sled: remove per-write fsync | **36×** bulk insert |
| `0736f99` | sorted-index `lookup_range/min/first_k` via `iter_range_stream` | 1.4–2.5× sled |
| `a088bbe` | index: key-per-record posting layout — O(K)→O(1) writes | 1.3–1.5× |
| `a85e9a4` | redb amortised durability | **14.6×** bulk_insert |
| `92093b7` | `Store::insert_many` — bulk write API | persy **44×**, nebari **67×**, redb **95×** |
| `de1400d` | release profile `opt-level "z"` → `3` | 1.3–1.7× CPU-bound |
| `57ea656` | sorted-index `ORDER BY ASC LIMIT K` fast path | **40×** at N=10K |
| `7d4af73` | reverse-iter sorted index — `DESC LIMIT K` + `MAX` | **10–200×** |
| `aecd80b` | `Store::get_many` | **3.4×** indexed range queries on sled |
| `a36196b` | native `iter_range_stream_reverse` on fjall + nebari | (parity for DESC fast path) |
| `796cdbf`, `06e41d3`, `23499fe`, `6cb149b` | docs / bench setup |

Самый крупный signal — algorithmic wins (B/C/D/Q1/Q2) превращают
full-scan в index lookup. Storage backend wins — durability tuning
(redb / sled fsync) + bulk APIs (`insert_many`, `get_many`).

---

## R2 — MemBuffer cycle

Полный build-and-tune для write-back buffer слоя:

| Commit | Сделано |
|--------|---------|
| `67966e1` | MemBufferStore passthrough proxy + factory composition (skeleton) |
| `76aa765` | Real LRU + write-back implementation |
| `b664996` | docs: wins per backend — persy **2.25×**, canopy **3.62×**, nebari **1.82×** |
| `ada3f0e` | MemBuffer как default, CachedStore composable, real-disk crash tests |
| `fa38a44` | bytes-bounded LRU + TTL eviction |
| `0d7c893` | flush_interval_ms 50/100 → 500 |
| `a44ee41` | hot-reloadable config via `apply_config` |
| `c9f5de1` | **batched byte-cap eviction** | 1.81× под pressure |
| `38e69ac` | **bounded approximate TTL sweep** | 3.17× под pressure |
| `67db1ed` | bench `membuffer_concurrent_rw` — proves cache.lock contention |
| `57198d2` | **migrate to moka** (W-TinyLFU + LRU window cache) | **3.23×** concurrent |
| `40b87a2` | set/remove skip inner.get probe | −24% byte-pressure |
| `cf56c2a` | dirty owns values authoritatively, no listener | regressions closed (−12% from pre-moka) |

Финальная архитектура: `ArcSwap<moka::Cache<RecordKey, Slot>>`
(read accelerator, lock-free reads) + `DashMap<RecordKey, Slot>`
(write-back buffer, owns values, drained по `flush_interval_ms`).

---

## R3 — Bench infra + small wins

| Commit | Сделано |
|--------|---------|
| `0679bd7` | `[profile.bench] lto=false` + `BENCH_QUICK` runtime | **15×** faster bench iteration |
| `7ae4a46` | `/opti` skill — local skill для bench→opt→test→bench→commit cycle |
| `bc28a00` | `RecordId::new()` — `thread_rng` replaces `OsRng` | **1.57×** (226 → 144 ns) |
| `91883fd` | Interner reverse map — `RwLock<Vec<...>>` → `ArcSwap` | **3.6×** под 4 потоками |
| `a395552` | WAL `list_inflight` scan batch 64 → 1024 | (recovery faster) |
| `a86f924` | bench `cache_hit_get` для MemBuffer hot path |
| `dd86e85` | bench `steady_state_insert_10k` |
| `12df89b` | bench `wal_high_qps` |

---

## R4 — Query path + codecs

Главная цель — устранить лишние allocations / value-tree
intermediate'ов на per-record hot path.

| Commit | Сделано | Δ |
|--------|---------|----|
| `8164582` | **msgpack encode** — `InternedRef<'a>` direct serde stream (no rmpv::Value tree) | **2.99×** (13.09 → 4.38 ms) |
| `d1e49ca` | **HAVING** — `legacy_value_to_inner` direct walk вместо `to_vec`+parse | **1.66×** (13.57 → 8.18 ms) |
| `655ba4c` | **Filter `resolve_field`** — borrow leaf вместо `Option<InnerValue>` clone | **1.22–1.25×** (243→195 µs, 698→573 µs) |
| `5f1e793` | **GROUP BY** — typed `GroupKeyItem` enum + lazy `inner_to_query_value` через Entry::Vacant + `IndexMap::sort_keys()` | **2.48–4.70×** (1.37–6.07 → 0.55–1.29 ms) |
| `4448909` | **Filter pre-resolve literals** — `Option<InnerValue>` cached на compile (Compare/Contains/Between) | **~1.2×** (160→137 µs, 404→330 µs) |
| `49b41c1` | **DISTINCT** — `HashableQueryValue` structural walk + FxHash вместо `record.to_string()` | **1.5–1.66×** (6.69→4.04 ms, 7.41→4.94 ms) |
| `30b7947` | **legacy text encode** — `InternedRef<'a>` direct serde stream (no intermediate value tree) | **3.27×** (11.65 → 3.57 ms) |

Главный приём: replaced **intermediate value tree** на **direct
serde stream**. Это убирает один pass (build tree) + per-node alloc.

---

## R5 — Session/connect + index/select + allocator

| Commit | Сделано | Δ |
|--------|---------|----|
| `caacf03` | **SessionStore + RateLimiter** SipHash → FxHasher | **1.94×** session lookup_at_hit, **1.96×** lookup_miss |
| `db89eb3` | **posting_cache** `Mutex<HashMap>` → `DashMap` (concurrent lookups) | refactor |
| `fae0b2e` | bench: `db_handler_rps` in-process RPS (1.66 Mreq/s) | (enabling) |
| `2f877af` | **Index batch** borrow leaves (`extract_index_values_ref`) + reuse computed `index_key: Bytes` для invalidation | refactor (O(N·I) alloc removed) |
| `84dac1d` | **Session HMAC** `OnceLock` cache в Session, pre-fill в `SessionStore::insert` | **11.08×** (134 → 12 ns) |
| `8ed07e7` | **Session permissions** — drop never-written `RwLock<SessionPermissions>` (dead lock) | clean-up |
| `42720a4` | **SelectProjection** borrow leaves + pre-built output keys (alias/last segment) при `new()` | **3.06–3.13×** (7.32→2.39 ms, 10.45→3.34 ms) |
| `d9c632f` | bench: **OrderBy DSU** precompute tried, no win, reverted (records the no-win) | — |
| `aa0774b` | **Sorted index `resolve_path`** — был `record.clone()` (full deep clone tree!), заменён на iterative borrow walk | refactor (big alloc removed) |
| `2df9650` | **mimalloc** as global allocator (`shamir-server` bin + bench) | **1.96×** db_handler/ping_inprocess (578→294 ns/req) |

---

## R6 — Interner Borrow + FilterNode enum + inlining + SmallVec

| Commit | Сделано | Δ |
|--------|---------|----|
| `ecffd54` | **Interner Borrow<str>** на UserKey — `DashMap::get(s)` принимает `&str` напрямую, no `String` alloc на cache-hit | **2.2×** touch_ind (340→142 ns) |
| `749b47a` | **FilterNode enum dispatch** — 14 `*Callback` структур + `Box<dyn FilterCallback>` → один `FilterNode` enum со static dispatch | refactor (вин on complex trees, parity на simple) |
| `3c294a0` | **`#[inline]`** hints на горячих leaf functions (compare_values, resolve_field_ref, filter_value_to_inner) | hint cross-crate |
| `eb10b41` | **SmallVec<[u64; 4]>** для FilterNode field_path — inline storage до 4 segments, no heap для типичных 1-2 уровневых путей | **1.36–1.46×** filter_eval (eq_int 186→137 µs, eq_str_nested 400→274 µs) |
| `06c128f` | **Realistic Execute bench** добавлен в db_handler_rps — filter+select+order+limit на 100 records (32 µs/req) + full_scan на 100 records (21 µs/req). Realistic baseline зафиксирован — Execute path ≈108× медленнее Ping. | bench (enabling) |
| `a45dc67` | **InMemoryStore iter/scan_prefix_stream** — собирать (key, value) в один pass (раньше делал второй `data.get` per record). Найдено crush storage агентом. | **1.21×** full_scan (21.29 → 17.64 µs) |
| `f0f9513` | **apply_pagination** in-place `split_off` + `truncate` (вместо `into_iter().skip().take().collect()`). Найдено crush allocs агентом, реализовано crush impl-агентом. | **1.47%** execute_read (41.43 → 40.82 µs) |
| `fdce5a8` | **batch executor ownership** — `mem::take(plan.stages)` + `filter_results` consume `all_results` через `retain` (вместо clone-into-new-map). Найдено crush batch агентом, реализовано crush impl-агентом. | **1.36×** execute_read (38.5 → 28.3 µs) |
| `8b51fbb` | **interner with_str + TouchInd::into_key** — закрытие двух double-alloc patterns: `deintern_key` через closure `with_str(|s|...)` (1 alloc вместо 2), `intern_string_key` через `TouchInd::into_key()` (no atomic ref bump). Найдено crush codecs агентом, реализовано crush impl-агентом. | **1.11×** codec_msgpack (5.46→4.90 ms), **1.08×** execute_read (38.7→35.7 µs) |

---

## Сводная таблица всех wins (по группам)

### Network / session / connect
| # | Win | Commit |
|---|-----|--------|
| `read_frame_into` pooled buffer | (round 0 baseline) | `edb7e76` |
| TicketPlain fixed-size | no heap Vec | `9578c55` |
| Aes256Gcm cipher cache | no per-resume schedule | `c5d121e` |
| `RequestEnvelopeView` zero-copy | no per-request Vec | `02bcfbd` |
| `touch_at` / `lookup_at` lock-free | exception of 1 syscall/req | `8073740` |
| Resume: move plain fields | no clone | `9963acd` |
| Single `write_all` + `write_frame_into` | (round 0) | `a4f7f4e` |
| In-place AES-GCM decrypt | zero-copy | `3dead97` |
| `RequestEnvelopeRef<'a>` client | zero-copy encode | `e0683d0` |
| SessionStore FxHasher | **1.94×** lookup | `caacf03` |
| RateLimiter FxHasher | (same commit) | `caacf03` |
| Session HMAC OnceLock | **11.08×** hmac_key | `84dac1d` |
| Session permissions drop RwLock | dead lock removed | `8ed07e7` |

### Storage / WAL
| # | Win | Commit |
|---|-----|--------|
| sled remove per-write fsync | **36×** bulk insert | `3158390` |
| redb amortised durability | **14.6×** bulk_insert | `a85e9a4` |
| `Store::insert_many` bulk API | persy 44×, nebari 67×, redb 95× | `92093b7` |
| `Store::get_many` | **3.4×** indexed range | `aecd80b` |
| native `iter_range_stream_reverse` (fjall, nebari) | parity DESC fast | `a36196b` |
| WAL `list_inflight` batch 64→1024 | recovery faster | `a395552` |

### MemBuffer
| # | Win | Commit |
|---|-----|--------|
| Build + LRU + TTL + bytes-bounded | persy 2.25×, canopy 3.62×, nebari 1.82× | `76aa765`, `fa38a44`, `b664996` |
| Hot-reloadable config | (enabling) | `a44ee41` |
| Batched byte-cap eviction | **1.81×** | `c9f5de1` |
| Bounded TTL sweep | **3.17×** | `38e69ac` |
| Migrate to moka | **3.23×** concurrent | `57198d2` |
| Skip inner.get probe | −24% byte-pressure | `40b87a2` |
| Dirty owns values (no listener) | −12% from pre-moka | `cf56c2a` |

### Index (regular + sorted + posting cache)
| # | Win | Commit |
|---|-----|--------|
| Sorted-index `lookup_range/min/first_k` via stream | 1.4–2.5× sled | `0736f99` |
| Key-per-record posting layout | O(K)→O(1) writes, 1.3–1.5× | `a088bbe` |
| ORDER BY ASC LIMIT K via sorted-index | **40×** at N=10K | `57ea656` |
| DESC LIMIT K / MAX via reverse-iter | **10–200×** | `7d4af73` |
| MIN(field) via `lookup_min` | **22×** at N=10K | `7afe259` |
| Filter Gt/Lt через sorted-index planner | algorithmic | `bc60476` |
| Posting-list cache in-memory | **4.3×** count-with-filter | `c8abbd7` |
| posting_cache `Mutex<HashMap>` → `DashMap` | concurrent | `db89eb3` |
| Batch borrow leaves + reuse `index_key` | O(N·I) alloc removed | `2f877af` |
| Sorted-index borrow walk (no full record.clone) | refactor | `aa0774b` |

### Query pipeline
| # | Win | Commit |
|---|-----|--------|
| execute_set use single-field index | **818×** | `3bb20c9` |
| execute_update/delete use read planner | **1115×** | `3769483` |
| count(*) via RecordCounter | **3383×** at N=10K | `f8552a0` |
| count(*) WHERE indexed_eq via BTreeSet::len | **22×** at N=10K | `66d7b5d` |
| Counter cache | bulk_insert −29% | `a3013c7` |
| HAVING legacy-value direct walk | **1.66×** | `d1e49ca` |
| Filter resolve_field borrow | **1.22–1.25×** | `655ba4c` |
| GROUP BY typed key + lazy QueryValue | **2.5–4.7×** | `5f1e793` |
| Filter pre-resolve literals | **~1.2×** | `4448909` |
| DISTINCT structural hash | **1.5–1.66×** | `49b41c1` |
| SelectProjection borrow + pre-keys | **3.06×** | `42720a4` |
| FilterNode enum dispatch | refactor | `749b47a` |
| `#[inline]` hints | hint | `3c294a0` |
| SmallVec field_path | **1.36–1.46×** | `eb10b41` |

### Codecs
| # | Win | Commit |
|---|-----|--------|
| msgpack direct serde stream | **2.99×** | `8164582` |
| legacy text direct serde stream | **3.27×** | `30b7947` |

### Interner
| # | Win | Commit |
|---|-----|--------|
| Reverse-lookup as Vec | 10–16% read-heavy | `a6abe93` |
| Debounce persist | −10% bulk insert | `a7d7a05` |
| ArcSwap for reverse | **3.6×** 4 threads | `91883fd` |
| Borrow<str> на UserKey | **2.2×** touch_ind | `ecffd54` |

### Misc
| # | Win | Commit |
|---|-----|--------|
| RecordId thread_rng | **1.57×** | `bc28a00` |
| release profile opt-level 3 | 1.3–1.7× CPU-bound | `de1400d` |
| mimalloc global allocator | **1.96×** RPS | `2df9650` |

---

## Investigated — no win (documented so we don't loop)

| Гипотеза | Cycle | Почему отвергнут |
|----------|-------|------------------|
| Arc<str> в Interner reverse vec | #43, reverted | +55% регресс. mimalloc делает String alloc дёшевым (~10 ns); Arc<str>::clone не быстрее. ArcSwap::load + Arc::clone суммарно проиграл. |
| MemBuffer dirty SipHash → FxHash | #24, reverted | Mixed bench (/2 +12%, /8 −7%). RecordKey hash не на критическом пути той же природы что fixed-size auth keys. |
| OrderBy DSU precompute | #35, reverted | pdqsort comparator уже эффективен; saved BTreeMap lookups cancel с indirection через perm[] + bookkeeping. |
| Slot enum tagged-pointer (MemBuffer) | #46 closed | `Option<Bytes>` тот же 40 bytes (нет niche optim — `Bytes::ptr` raw `*const`). Tagged-pointer unsafe не оправдан. |
| UnixNanos::now() × 3 в handshake | #47 closed | 300 ns vs 3 сек Argon2 = 0.00001%. Argon2 ограничен семафором ~10/sec. |
| Filter compile cache LRU | #48 closed | compile_filter ~1-10 µs / query — не доминирует. Cache invalidation при interner state change — complex. |
| RecordKey Bytes inline (Arc<[u8;16]>) | #49 closed | `Bytes` crate не поддерживает inline storage для 16-byte buffers — always heap. RecordId-as-Bytes — big refactor, ломает Copy. |
| bincode 1.x → 2.x | пропущен в разведке | Используется только для index metadata (admin path), не query hot path. ROI низкий. |
| HMAC key cache local Option в db_handler | заменён на #33 OnceLock | Лучше через Session field. |
| Encode buffer reuse (BytesMut pool) | #32 closed | Текущий паттерн `encode → Bytes::from(Vec)` (move ownership). Reuse требовал бы `Bytes::copy_from_slice` — добавляет memcpy. Net regression. |
| `run_blocking` audit (db_handler) | #60 closed | Уже использует `tokio::task::block_in_place + Handle::current().block_on`. Правильный паттерн — НЕ spawn_blocking (без context switch). |
| Audit log sync emission | #59 closed | `AuditAppender` уже имеет Strict + Batched modes. Production default = Batched (mutex.lock + Vec::push). Уже оптимально. |
| TableManager lookup кэш в Session | #64 closed | DashMap<String, ...>::get ~100ns на 32µs request = 0.3%. Cache добавит invalidation complexity (table drop). Win не оправдан без profile signal. |
| apply_select streaming (без Vec<QueryValue>) | #63 closed | QueryResult.records: Vec<QueryValue> — публичный API. Streaming требует менять QueryResult shape (impl Serialize), broad refactor в query/batch/dispatch. Win неясен без profile (msgpack encode walks equally). Откладываю до architectural redesign. |

---

## Deferred — большие архитектурные

| Задача | Причина откладывания |
|--------|----------------------|
| Stream-based query results (#51) | Меняет QueryResult, batch executor, dispatch, ResponseEnvelope. На типичных 100-1000 records / response — minor. |
| Batch plan cache (#52) | Overlap с filter compile cache (отвергнут). Compile cost не доминирует. Cache invalidation сложна. |
| CPU profiling samply/flamegraph (#53) | Инструмент, не optim. Под production-like workload — без него signal-to-noise низкий. Setup на стороне оператора. |
| SIMD compare (#55) | `std::str::cmp` уже использует memcmp (LLVM авто-vectorise для длинных строк). Для коротких (<32 bytes) SIMD не помогает. Нет signal. |
| Codec negotiation postcard/cbor (#56) | Wire protocol breaking change. msgpack уже **3×** прямой stream. Win 30-50% не оправдывает breaking. |
| Backends compare bench (#37) | Только `InMemoryStore` с direct `Store::new()`; остальные через `Repo::store_get(name)`. Требует Repo-wiring setup. |
| WAL fsync batching / group commit (#38) | Многодневная архитектурная работа: bench с durable backend, implement group commit window. |
| Tokio runtime tuning (#40) | Defaults обычно OK. Требует production-like load + tokio-console. |
| Backend durability further tuning (#54) | redb уже `Durability::None`; sled `flush_every_ms` default. Уже оптимизировано — следующий шаг требует production write-heavy bench. |
| `rmp_serde::to_vec_named` → `to_vec` compact (#61) | Wire-breaking change. Decoder hot path для DbRequest/DbResponse. Win ~10-30% bytes + faster encode, но требует coordinated client SDK migration. Откладываю до major protocol revision. |
| PGO build (#58) | Production-build concern. Setup: llvm-tools-preview + 2 builds + workload run. Win 5-15% broadly. Не для single /opti cycle — это release engineering. |

---

## Bench-файл карта

```
crates/
├── shamir-types/benches/
│   ├── record_id.rs               — RecordId::new() (R3)
│   ├── codec_msgpack.rs           — inner_to_msgpack encode (R4)
│   ├── codec_legacy_encode.rs     — inner_to_legacy encode (R5)
│   └── codec_legacy_roundtrip.rs  — to_vec+parse vs direct walk (R4)
├── shamir-engine/benches/
│   ├── wal_recovery.rs            — list_inflight scan (R3)
│   ├── interner_concurrent.rs     — touch_ind / get_str под потоками (R3)
│   ├── membuffer_concurrent.rs    — concurrent_rw 1/2/4/8 readers (R2)
│   ├── filter_eval.rs             — Eq на 1000 records, top-level + nested (R4)
│   ├── group_by_keys.rs           — apply_group_by 1000 records (R4)
│   ├── distinct.rs                — apply_distinct unique + half-dup (R4)
│   └── select_pipeline.rs         — apply_select + apply_order_by (R5)
├── shamir-db/benches/
│   └── engine_perf.rs             — full-engine paths (read/set/index, R1)
├── shamir-server/benches/
│   └── db_handler_rps.rs          — ShamirDbHandler::handle in-process RPS (R5)
├── shamir-connect/benches/
│   └── hot_paths.rs               — dispatch / session_store / envelope / crypto (R0)
└── shamir-transport-tcp/benches/
    └── framing.rs                 — read_frame_into / write_frame_into (R0)
```

---

## Workflow для следующих циклов

1. **Глянуть в этот файл** — гипотеза уже отвергнута или сделана?
2. Если нет — добавить таску, прогнать `/opti` цикл
   (`baseline bench → optim → tests → after bench → commit`).
3. **На win** — обновить **группу** + **сводную таблицу**.
4. **На no-win** — добавить в **investigated** с rationale (чтоб не
   повторять).
5. **На большой архитектурный** — записать в **deferred** с
   мотивацией. Не делать, пока нет profiler-signal под production
   workload.

---

## Связанные документы

- `docs/ops/PERF_OPTIMIZATIONS.md` — full bench numbers для R0
  (shamir-connect + transport-tcp).
- `docs/ops/PERF_BASELINE.md` — raw baseline numbers до начала
  оптимизаций.
- `docs/ops/PERF-PLAN.md`, `docs/ops/PERF-PLAN-NEXT.md` — planning
  docs для R1 / R3.
- `docs/roadmap/PERF_OPPORTUNITIES.md` — найденные opportunity в R1.
