# Vector Search → Production-Ready: исполняемый план

## Context

У ShamirDB есть рабочий Layer 0 векторного поиска (HNSW in-memory + brute-force
fallback + SIMD + tx-staging), но он **не production-ready для мира**: граф
теряется на рестарте (полный rebuild-скан), нет фильтрованного ANN, нет
квантизации (1M×768D f32 ≈ 3 GB), нет батчевого upsert (1M векторов строятся
часами), нет per-query контроля и `$score`, и — по мотивам разбора анонса
Platform V Vector DB — нет релизного бенч-пакета с recall/latency/memory.

Существующий дизайн `VECTOR_PRODUCTION_PLAN.md` описывает 6 фаз, но разошёлся с
реальным кодом. Этот документ — его **ревизия по факту кода** (разведка
2026-07-04) + разбивка на исполняемые листы (лист = бриф суб-агента + verify +
commit по prompt-first процессу). Цель: довести до production-grade — persisted,
filtered, quantized, batched, benchmarked.

---

## Решения (зафиксировано 2026-07-04)

1. **Объём — только ядро P0–P5.** P6 Hybrid (vector+FTS) и Layer 1 embedders
   в эту кампанию НЕ входят. P6 разблокирована (FTS в коде) — добавляема позже
   отдельным спринтом.
2. **Бенчи — только QUICK-режим.** FULL-заглушку в `shamir-bench-utils` НЕ
   снимаем (полные прогоны на векторных ступенях нигде реально не отработают).
   Отдельный «строгий» инструмент не делаем; публикуемые цифры — QUICK-tier,
   воспроизводимые; ступени реалистичные (10K/100K, 1M — не гоним).
3. **`$score` как магичное метаполе — отвергнут.** Отдача ранжирующего числа
   (дистанция/BM25) — это НЕ векторная фича, а языковая: «как в OQL достать
   значение, вычисленное предикатом в `where`». Дизайн вынесен в отдельную
   заметку **`docs/design/where-select-binds.md`** (механизм `bind` на
   предикате + форма возврата, склон — отдельный `$meta`-канал) и реализуется
   **отдельным треком ПОСЛЕ этой кампании**. В ядро P0–P5 не входит; лист 1.2
   удалён. Сегодня ранжированный результат и так приходит в порядке
   релевантности (`read_exec.rs:327-330`), чего для векторной кампании
   достаточно.

---

## Поправки к плану-доку (проверено кодом)

| # | В плане-доке / первой разведке | Реальность (проверено) | Следствие |
|---|---|---|---|
| K1 | «hnsw_rs 0.3 не имеет dump/load, batch, seed» | Cargo.lock: **hnsw_rs 0.3.4**. Наличие `file_dump`/`HnswIo`/`parallel_insert`/`search_filter` — **требует проверки спайком 0.0** (источники противоречат) | Лист 0.0 де-рискует фазу persist ДО кода |
| K2 | «Фаза 6 блокирована: FTS нет кода» | **Устарело.** Есть `crates/shamir-index/src/{bm25,fts_backend,fts_ranked_backend}.rs`, `Filter::Fts` в query-types | P6 РАЗБЛОКИРОВАНА |
| K3 | seed API у hnsw_rs нет | Подтверждено (`hnsw_adapter.rs:29-37`) | `BRUTE_FORCE_MAX=256` остаётся; recall-тесты статистические на ≥1k |
| K4 | дистанция отбрасывается | Подтверждено: `read_planner.rs:52-59` + `read_exec` `for (rid,_)` | точка внедрения `$score` |
| K5 | FULL-режим бенчей hard-disabled | Подтверждено: `shamir-bench-utils` `is_full()`≡false | нужно решение R1 |
| K6 | нет atomic-rename; есть chunk-persist | Подтверждено: `InternerManager` (HWM + zero-padded chunks + MetaEnvelope SDB2), `Store::transact` атомарен | снапшот — в info_store chunks, не отдельным файлом |
| K7 | нет checksum в index | `crc32fast` уже в workspace (shamir-wal) | добавить dep, xxh3 не тащить |
| K8 | — | `HnswAdapter::len()` зовёт `deleted.len()` (O(N)) на КАЖДОМ search (выбор brute-vs-graph) | скрытый O(N) на hot path → atomic-зеркало (лист 4.1) |

Плюс: `hnsw_rs = "0.3"` (`crates/shamir-index/Cargo.toml:45`) — плавающий минор;
пин `=0.3.4` (лист 0.0) ради стабильности dump-формата.

---

## Последовательность фаз (скорректирована)

```
P0  Bench-фундамент + спайк hnsw_rs + upsert_batch-энейблер   ~4–5 д
P1  Query-time: per-query ef_search                           ~1–1.5 д (бывш. фаза 5, сдвинута вперёд)
P2  Persisted HNSW (snapshot + delta-log в info_store)        ~5–6 д   (бывш. фаза 1)
P3  Filtered ANN (pre / co(FilterT) / post+oversample)        ~4–5 д
P4  Batch commit-path + deleted_ratio + компакция             ~3–4 д
P5  Quantization SQ8 (формат снапшота v2)                     ~4–5 д
```
(P6 Hybrid RRF — вне кампании, см. Решения.)

**Почему так:** `upsert_batch` вытащен в P0 (без него 100K ступени baseline
не построить). P1 перед persist/filter — сигнатура `search` расширяется до
`SearchOpts` ОДИН раз, до навешивания на неё персиста и фильтров. P5 после P4 —
компакция обязана быть quantization-aware (снапшот v1 резервирует поля под v2).

---

## Ключевые дизайн-решения

### Персистентность HNSW (P2) — hnswio dump/load в нашем конверте, rebuild как fallback
Зеркалит `InternerManager`:
- **Base snapshot**: `hnsw.file_dump()` (spawn_blocking) → чанки ~1 MiB + crc32 в
  info_store + sidecar (MetaEnvelope: dim/metric/params/next_id/rid_map/tombstones/
  hnsw_rs-версия/reserved-quantization); **манифест** флипит поколение одним
  `Store::transact` (атомарность из коробки).
- **Delta-log** (не WAL-tail — у Store нет LSN-scan): Phase 5d после promote
  аппендит delta-чанк `Vec<(RecordId, Op)>`; порог (tunable) → фоновый снапшот
  нового поколения → транзакционный флип + prune.
- **Load**: манифест → verify crc → temp-файлы → `HnswIo::load_hnsw_with_dist` →
  replay delta. Любая ошибка → warn + текущий `rebuild()` (после P0 — parallel).
- **Lifetime**: `Box::leak(HnswIo)` — один осознанный boot-only leak на load
  индекса (компакция строит insert'ами, не reload → не накапливается).

### Бенч-фундамент (P0) — раздельные инструменты, QUICK-режим
- **Criterion** (`vector_search.rs`) — только latency/QPS; `tune_tiered` с
  обязательным `max_wall_secs` (чинит нарушение CLAUDE.md); QUICK-tier (FULL не
  снимаем); ступени 10K/100K, dim {128,768}, metric {Cosine,L2}. 1M — только
  опциональный ручной эксперимент за `BENCH_VECTOR_1M=1`, не в DoD.
- **vector_report** (`examples/vector_report.rs`) — recall@1/@10 (ground truth
  брутфорсом на 100 seeded запросов), build time, RSS → markdown в
  `docs/benchmarks/vector/<date>-*.md`. Recall детерминирован — criterion не нужен.
- **Датасет**: кластеризованный генератор в `shamir-bench-utils` (K центроидов +
  гаусс-шум, seeded), общий для обоих инструментов.
- **`scripts/bench-vector.sh`**: единый вход, `CARGO_TARGET_DIR=…\.cargo-target-bench`.

### Wire/builder-изменения (все аддитивные optional msgpack + Rust/TS parity-фикстура)

| Поле | Где | Лист |
|---|---|---|
| `ef_search: Option<u32>` | `Filter::VectorSimilarity`, `IndexQuery::Vector`; `.ef_search()`/`.efSearch()` | 1.1 |
| `oversample: Option<f32>` | `Filter::VectorSimilarity`; `.oversample()` | 1.1 поле / 3.1 семантика |
| `quantization: Option<String>` | DDL create_index + `VectorConfig`; `.quantization("sq8")` | 5.2 |
| `bind: Option<String>` + `{"$bound": name}` в select | where-бинды (вариант B) — дизайн зафиксирован, реализация ОТДЕЛЬНЫМ треком, вне кампании | — |
| `HybridSearch{vector,text,k,fusion}` | новый op query-types + оба билдера — вне кампании (P6) | — |

Каждое (из входящих в кампанию) — с back-compat тестом «старый msgpack без поля читается».

---

## Разбивка на листы

Каждый лист: бриф → `docs/prompts/vector/<NN>-<name>.md` (коммит ДО запуска,
verbatim-запрет git-мутаций), TDD red-first, гейт = `fmt --check` +
`clippy --all-targets -D warnings` + `./scripts/test.sh`, verified → commit.

### P0 (~4–5 д)
- **0.0 Спайк-контракт hnsw_rs 0.3.4** — пин `=0.3.4` + dep `crc32fast`; contract-тесты `file_dump`↔`load_hnsw_with_dist(ShamirDist)` round-trip, `Box::leak`-паттерн, `parallel_insert` эквивалентность, `search_filter`/FilterT семантика, `Hnsw<i8,_>` компилируемость (для P5). Файлы: `crates/shamir-index/Cargo.toml`, новый `vector/tests/hnsw_rs_contract_tests.rs`.
- **0.1 Bench-инфра** — R1-решение по FULL, `@vector` scope в `scripts/test.sh`, кластеризованный генератор в `shamir-bench-utils/src/vector_data.rs`.
- **0.2 `upsert_batch`** — default-метод в `adapter.rs` + override `HnswAdapter` (валидация dim заранее, D12-инвариант сохранить, один `spawn_blocking`+`parallel_insert`); `rebuild()` переводится на батч. Файлы: `vector/adapter.rs`, `hnsw_adapter.rs`, `vector_backend.rs`.
- **0.3 Criterion-бенч rewrite** — `benches/vector_search.rs`: `tune_tiered`, ladder, dim, clustered data, build через `upsert_batch`.
- **0.4 vector_report + `scripts/bench-vector.sh`** — recall/RSS/build, каталог `docs/benchmarks/vector/`.
- **0.5 Baseline-отчёт + гигиена доков** — прогон, коммит baseline; правки `EMBEDDINGS_AND_VECTORS.md` (статус) и `VECTOR_PRODUCTION_PLAN.md` (K1–K8).

### P1 (~1–1.5 д)
- **1.1 Per-query `ef_search`** — `Filter::VectorSimilarity{…, ef_search: Option<u32>}` (+`oversample` поле сразу), `SearchOpts` в адаптере, прокид в `hnsw.search`, билдеры Rust+TS+parity, clamp (DoS).
- ~~1.2 `$score`~~ — удалён. Отдачу ранжирующего значения решает отдельный трек «where-бинды» (см. Решения, п.3), вне этой кампании.

### P2 (~5–6 д)
- **2.1 Snapshot-кодек** — новый `vector/snapshot.rs`: dump→чанки+crc32→info_store, load→`Box::leak`, sidecar в MetaEnvelope. Round-trip / битый crc / чужая версия / tombstones.
- **2.2 Startup-интеграция + fallback** — `table_manager.rs` open-путь: try load → fallback `rebuild()`+warn; AtomicU64 rebuild-счётчик для тестов (==0 при валидном снапшоте).
- **2.3 Delta-log + триггер снапшота** — delta-чанки, generation flip одним `transact`, prune; Phase 5d append; tunable-порог; фоновый снапшот single-flight. Краш-инъекции flip/prune.
- **2.4 Crash-тесты + cold-start бенч + отчёт** — truncated/битый манифест→fallback; DoD: рестарт 1M ≤ единиц секунд.

### P3 (~4–5 д)
- **3.1 План-распознавание + post-filter+oversample** — `And([VectorSimilarity, preds])`→`FilteredVectorQuery`, retry k′=k×oversample до `MAX_TOPK`. Корректность vs brute-force ground truth.
- **3.2 Pre-filter (exact SIMD) + co-filter (`search_filter`) + cost-based выбор** — candidate set из вторичного индекса; property-тест эквивалентности трёх путей.
- **3.3 Selectivity-бенч + отчёт** — 0.1/1/10/50%; DoD recall@10≥0.9 при 1%, latency≤2× unfiltered.

### P4 (~3–4 д)
- **4.1 Батчевый promote + deleted_ratio-зеркала** — `apply_committed_vectors`→`upsert_batch`; AtomicUsize live/deleted; **починить `len()` (K8)**.
- **4.2 Фоновая компакция** — порог deleted_ratio → rebuild-aside через `upsert_batch` + дельта-доигрывание (паттерн brute_force-актора) → ArcSwap swap; форс снапшота после. Корректность поиска ВО ВРЕМЯ компакции.
- **4.3 Бенчи bulk/компакции** — DoD bulk ≥5× vs по-одному.

### P5 (~4–5 д)
- **5.1 SQ8-квантайзер + int8 SIMD** — `vector/sq8.rs` (per-dim min/max asymmetric), `simd.rs` `dot_i8` (AVX2 maddubs/AVX-512 VNNI/NEON sdot/scalar). Ядра==scalar reference; recall drop ≤2%.
- **5.2 Квантованный граф + rescoring + DDL** — `Hnsw<i8,_>` + f32 для rescoring; `quantization` в `VectorConfig`+DDL+билдеры+parity. Строго opt-in.
- **5.3 Снапшот v2 + миграция + компакция-aware + бенч** — format bump; v1@quant→rebuild+warn; RSS/QPS/recall f32 vs sq8.

### V6 — клиентская поверхность полного стека (~2–3 д)

Сквозное требование кампании: **доработки включают ВСЕ уровни** — unit-тесты,
OQL, Query Builders (Rust+TS), e2e-тесты, e2e TypeScript-тесты, бенчмарки.
Листы фич несут свои уровни сами (V1.1/V3.1/V5.2 — wire+билдеры+parity);
эта фаза добирает то, что размазано:

- **6.1 Node e2e `tests/e2e/tests/18-vectors.test.js`** — полный чёрный ящик
  по образцу 16/17: DDL vector-индекса (dim/metric/quantization), insert,
  top-k, порядок, filtered ANN, per-query ef_search/oversample. Требует
  MSVC-хост для прогона (как 17-й).
- **6.2 TS e2e расширение `e2e-vector.test.ts`** — с toy dim=4 на 128/768D,
  все метрики {Cosine,L2,Dot}, efSearch/oversample, filtered ANN,
  quantization; + vitest-юниты новых билдеров, где не добавили листы фич.
- **6.3 OQL-поверхность + guide** — сверка OQL-форм векторных фич (+ парсер-
  юниты, если есть текстовый синтаксис), `docs/guide/06-search.md`
  (ef_search, oversample, filtered, quantization, персистентность/компакция).
  Примеры в доках — builder-only.

### Hybrid RRF — вне кампании (дизайн сохранён на будущее)
Hybrid разблокирован (FTS в коде), но в этот объём не входит. Дизайн на
будущее: RRF-комбайнер — чистая функция над двумя `Ranked`,
`score=Σ1/(60+rank)`, юнит детерминизм/tie-breaking; `HybridSearch` op +
билдеры + executor (`tokio::join!` над vector+FTS, деградация до одного).

---

## Закрытые вопросы

- **R1 (FULL-режим бенчей) → РЕШЕНО: только QUICK.** Заглушку не снимаем,
  отдельный строгий инструмент не делаем; публикуемые цифры — QUICK-tier.
- **R6 (отдача ранжирующего значения) → РЕШЕНО: `$score` отвергнут, принят
  дизайн where-биндов (вариант B), реализация — отдельным треком после кампании**
  (см. Решения, п.3).
- **Объём → РЕШЕНО: ядро P0–P5.** P6 и embedders — вне кампании.

Рабочие дефолты (можно пересмотреть в брифах соответствующих листов):
R2 — пин `hnsw_rs = "=0.3.4"` (да). R3 — `Box::leak` HnswIo (boot-only, ок).
R4 — dev-dep для RSS (`memory-stats`). R5 — delta-log = двойная запись вектора
между снапшотами (ограничено порогом). R7 — 1M-ступень только ручной
эксперимент за env, не в DoD.

---

## P2 follow-ups (отложено из V2.4)

- **gap#1 — tx-committed vector deletes НЕ применяются к HNSW-графу — KNOWN
  DURABILITY+CORRECTNESS LIMITATION (задача #416).** Это НЕ косметический
  «deferred»: ревью V2.4 установило точную границу. `VectorBackend::
  plan_delete_tx` при `tx_id==Some` — **no-op**, и промоут-пути для delete нет
  (`staged_vector` стейджит только insert). Значит tx-committed удаление записи
  с vector-индексом вообще НЕ доходит до графа: ghost-вектор виден в ANN-поиске
  И ДО рестарта (живой граф не тумбстонит), И после (в delta нет `DeltaOp::
  Delete`). Vector search возвращает `RecordId` удалённой записи. Это
  **PRE-EXISTING HIGH-6 ограничение** (не регресс кампании — tx-путь векторных
  удалений никогда не был проведён), которое P2-персистентность лишь вскрыла.
  Отложено как приоритетная задача **#416** (проводка + graph-delete + delta),
  не как follow-up. Механизм delta доказан тестом; чинить вместе с V4.1/#407
  (batch promote path) или отдельно. Детали ниже:
- **gap#1 механика — tx-path vector deletes.** Лист V2.3 оставил проводку
  tx-удалений вектора в delta-log пустой: `commit_phases::apply_vector_batch`
  передаёт `deleted = &[]` в `append_vector_delta`. Причина: в `TxContext`
  нет `staged_deletes`-поля для векторов; tx-удаления строки с vector-индексом
  идут через `plan_delete_tx` → `index_write_set` (RemovePosting-style), а не
  через HNSW-promote-путь. Полноценная проводка вариант-A требует: собрать
  удалённые rid из tx write_set/index_write_set, отфильтровать таблицы с
  `VectorBackend`, убедиться что graph-side delete уже отработал (delta-чанк —
  durable-эхо in-memory мутации, см. контракт `DeltaOp::Delete` в
  `snapshot.rs`). Это многослойная работа, которую P2-закрывающий лист
  обоснованно не рискнул сделать. **Механизм доказан** unit-тестом
  `delta_log_tests::append_vector_delta_with_deleted_slice_persists_and_replays_delete`
  (фиксирует, что `append_vector_delta(.., deleted=[rid])` пишет
  `DeltaOp::Delete`, применяемый при restart). Когда вариант-A.land-нет,
  заменить `&[]` в `commit_phases.rs` на собранный slice. Явный комментарий
  с roadmap-ссылкой стоит в `commit_phases.rs` у самого вызова.

---

## Verification

- Каждый лист: `./scripts/test.sh @vector` (+`--full`/`@oracle`/`@e2e` где указано),
  `fmt --check`, `clippy --all-targets -D warnings`.
- Бенчи (QUICK): `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench scripts/bench-vector.sh`
  (SMOKE в CI, QUICK локально).
- Фазовые DoD-отчёты в `docs/benchmarks/vector/` — воспроизводимы одной командой.
- Cross-language wire-parity фикстуры (Rust↔TS) для каждого нового поля/op.
- Release Benchmark Checklist (версии/SHA, железо+SIMD, seed, QPS, p50/95/99,
  recall@1/@10, build, RSS, filtered selectivity) публикуется на фазах 0/2/3/4/5
  в QUICK-режиме.
