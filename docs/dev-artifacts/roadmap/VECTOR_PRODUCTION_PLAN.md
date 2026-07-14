# Vector Search — Production Plan

Status: **план работ, код Layer 0 уже в дереве.**

> **⚠ Revised by `VECTOR_PRODUCTION_EXECUTION.md`.** This document is the
> original 6-phase outline; the executing plan
> (`VECTOR_PRODUCTION_EXECUTION.md`, разведка 2026-07-04) revises it against
> the actual code and re-splits it into executable sheets (P0–P5). Where the
> two disagree, **`VECTOR_PRODUCTION_EXECUTION.md` wins.** The key corrections
> are summarised in "Поправки K1–K8 (план vs код)" at the end of this file —
> read that section before trusting any phase boundary or dependency arrow
> below.

Компаньон к `EMBEDDINGS_AND_VECTORS.md` (дизайн) и
`FULL_TEXT_SEARCH.md` (пререквизит гибридного поиска). Этот документ —
пошаговый план доведения векторной подсистемы до production-grade:
персистентность, фильтры, квантизация, батчи, и — отдельным
первоклассным артефактом — релизный бенчмарк-пакет.

---

## Кратко по-русски

### Повод

Разбор анонса Platform V Vector DB (СберТех, 2026-07-01): продукт
заявлен, но публично нет ни версии апстрима (форк Qdrant), ни списка
изменений ядра, ни одного бенчмарка — ни масштабных (10M/100M), ни с
фильтрами по payload, ни single-node vs cluster. Для технической
аудитории такой релиз неотличим от несуществующего продукта.

Выводы для нас, зафиксированные как требования этого плана:

1. **Бенчмарки — часть релиза, не постфактум.** Каждая фаза ниже
   заканчивается публикуемым отчётом с цифрами до/после.
2. **Filtered search — индустриальный стандарт бенчмарка**, потому что
   это стандартный корпоративный сценарий («похожие документы, где
   `tenant_id = X`»). У нас он должен быть и в движке, и в бенчах.
3. **Масштабные ступени** (10K → 100K → 1M → 10M) с графиком
   деградации, а не одна точка «быстро на маленьком датасете».
4. **Воспроизводимость**: датасет, seed, железо, версии — в каждом
   отчёте.

### Что уже есть (инвентаризация 2026-07-03)

Layer 0 из `EMBEDDINGS_AND_VECTORS.md` реализован (статус-строка того
дока устарела; актуализация — шаг 0.6):

| Компонент | Где |
|---|---|
| `VectorConfig` / `VectorMetric` (L2, Cosine, Dot) | `crates/shamir-index/src/kind.rs` |
| HNSW-адаптер (`hnsw_rs` 0.3, m=16, ef_c=200, ef_s=50, soft-delete) | `crates/shamir-index/src/vector/hnsw_adapter.rs` |
| Brute-force fallback (≤256 записей, ArcSwap snapshot, top-k heap) | `crates/shamir-index/src/vector/brute_force.rs` |
| SIMD dot/l2 (AVX-512F, AVX2+FMA, NEON, scalar) | `crates/shamir-index/src/vector/simd.rs` |
| `Filter::VectorSimilarity { field, query, k }` | `crates/shamir-query-types/src/filter/filter_enum.rs` |
| Транзакционный staging (HIGH-6, staged_vectors в TxContext) | `crates/shamir-index/src/vector/vector_backend.rs` |
| DDL-билдер (`index_type("vector").vector_dim(..).vector_metric(..)`) | `crates/shamir-query-builder/src/ddl/create_index.rs` |
| Бенч BruteForce vs HNSW, 1K/10K, 128D | `crates/shamir-engine/benches/vector_search.rs` |
| e2e из TS-клиента | `crates/shamir-client-ts/src/__tests__/e2e-vector.test.ts` |

### Чего нет (и что закрывает этот план)

- персистентного HNSW — граф in-memory, на старте полный rebuild
  сканом (фаза 1);
- фильтрации при ANN-поиске — ни post-, ни pre-filter (фаза 2);
- квантизации — 1M×768D f32 ≈ 3 GB только на вектора (фаза 3);
- батчевого upsert в HNSW — каждый вектор своим `spawn_blocking`
  (фаза 4);
- компакции tombstone — soft-delete копится, recall деградирует
  (фаза 4);
- per-query `ef_search` / oversample (фаза 5);
- гибридного поиска — блокирован FTS, которого нет (фаза 6);
- релизного бенч-пакета с recall/latency/memory на масштабных
  ступенях (фаза 0 — с неё начинаем).

Сознательно НЕ берём (сверено с дизайн-доком): external backend
(проксирование в Qdrant/Pinecone — противоречит self-contained),
multi-vector/ColBERT (другая структура индекса), IVF (HNSW +
квантизация закрывают наши масштабы), sparse vectors / SPLADE (нет
спроса). Auto-embed слой (Layer 1) — отдельный трек по
`EMBEDDINGS_AND_VECTORS.md`, этим планом не покрывается.

---

## Порядок фаз и зависимости

```
Фаза 0 (bench baseline)
  └─→ Фаза 1 (persisted HNSW)
        └─→ Фаза 2 (filtered ANN)
              ├─→ Фаза 3 (quantization SQ8)   ← меняет формат снапшота из ф.1
              └─→ Фаза 4 (batch upsert + compaction)
                    └─→ Фаза 5 (query-time контроль)  — маленькая, можно раньше
Фаза 6 (hybrid RRF) — блокирована FULL_TEXT_SEARCH.md Phase 0, в очередь
                      после появления FTS; интерфейсный контракт готовим в ф.2
```

Каждая фаза — отдельная серия коммитов по prompt-first процессу
(бриф в `docs/dev-artifacts/prompts/vector/<NN>-<name>.md` коммитится ДО запуска
агента; verified stage → immediate commit).

---

## Фаза 0 — Бенчмарк-фундамент и baseline

Цель: до любых изменений ядра зафиксировать воспроизводимый baseline,
чтобы каждая следующая фаза доказывала «до/после» цифрами.

- **0.1. Масштабные ступени.** Расширить
  `crates/shamir-engine/benches/vector_search.rs`: 10K / 100K / 1M
  векторов × {128D, 768D} × {Cosine, L2}. Обязательно
  `shamir_bench_utils::tune(..)` — QUICK-дефолт; полные прогоны через
  `BENCH_FULL=1`.
- **0.2. Датасеты.** Детерминированная генерация с фиксированным seed
  (кластеризованное распределение, не uniform — uniform нереалистично
  льстит recall). Seed и параметры генератора — в отчёте.
- **0.3. Recall-метрика.** Ground truth брутфорсом по подвыборке
  запросов (например, 100 запросов); отчёт recall@1 / recall@10.
- **0.4. Метрики отчёта.** QPS, latency p50/p95/p99, recall@k, build
  time, RSS. Формат отчёта — markdown в
  `docs/dev-artifacts/benchmarks/vector/<date>-baseline.md`.
- **0.5. Скрипт `scripts/bench-vector.sh`.** Один вход: прогон пакета
  с `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench` (изоляция кэша
  — см. CLAUDE.md), сбор результатов в отчёт-заготовку.
- **0.6. Гигиена доков.** Обновить статус-строку
  `EMBEDDINGS_AND_VECTORS.md` («Layer 0 implemented, см.
  VECTOR_PRODUCTION_PLAN.md») и сверить имена типов в доке с кодом
  (`Filter::VectorSimilarity`, `VectorMetric::{L2, Cosine, Dot}`).

Тесты: сам бенч-код — smoke-запуск в CI не нужен (QUICK-режим руками).
DoD: baseline-отчёт закоммичен; повторный прогон одной командой
воспроизводит цифры в пределах шума.

Оценка: 2–3 дня.

---

## Фаза 1 — Persisted HNSW

Цель: рестарт = O(open + WAL tail), не O(rebuild скана). Пробел №1 —
без него «тест на 10M» упирается в холодный старт на минуты/часы.

Подход: **snapshot + WAL-хвост** (симметрично остальному движку).
Снапшот графа пишется на checkpoint/shutdown; на старте — load
снапшота + доигрывание дельты из WAL поверх загруженного графа;
любая ошибка → fallback на текущий полный rebuild.

- **1.1. Формат снапшота** в `__vec_idx__<table>__<idx>` (layout из
  дизайн-дока): версионированный заголовок (magic, format version,
  dim, metric, hnsw params, count), секции: mapping RecordId ↔
  internal id, слои графа, сырые вектора, tombstone-set. Checksum
  per-section (пиллар «checksums everywhere»). **Design decision в
  брифе:** обёртка над `hnsw_rs::hnswio` dump/load внутри нашего
  конверта vs собственная сериализация слоёв; критерии — контроль
  формата, стабильность между версиями hnsw_rs, скорость load.
  Заголовок обязан иметь reserved-поля под фазу 3 (квантизация).
- **1.2. Snapshot writer.** Фоновая задача через `spawn_blocking`;
  запись во временный файл → fsync → atomic rename. Триггеры:
  graceful shutdown + периодический checkpoint (порог по числу
  изменений с последнего снапшота — tunable в `shamir-tunables`).
- **1.3. Recovery path.** `init()`: снапшот есть → verify checksums →
  load → replay WAL tail (upsert/delete после LSN снапшота). Снапшота
  нет / битый / версия формата чужая → полный rebuild сканом
  (сегодняшнее поведение) + warn в лог.
- **1.4. Tombstones.** Soft-delete set сериализуется в снапшот;
  replay WAL-дельты применяет и удаления.
- **1.5. Тесты (TDD, red first).**
  - restart сохраняет индекс без полного скана (fail-инструментация:
    счётчик full-scan rebuild, ассерт == 0);
  - crash-injection: обрезанный файл снапшота → fallback rebuild, без
    паники;
  - повреждение checksum → fallback rebuild;
  - e2e: create → insert 10K → рестарт → search, recall тот же;
  - tx-изоляция: staged vectors не попадают в снапшот до коммита.
- **1.6. Бенч.** Cold-start 100K / 1M: snapshot-load vs rebuild.
- **1.7. DoD.** Рестарт на 1M векторов ≤ единиц секунд (точный порог
  зафиксировать по факту в отчёте фазы); все crash-тесты зелёные;
  отчёт `docs/dev-artifacts/benchmarks/vector/<date>-persisted-hnsw.md`.

Тесты гонять: `./scripts/test.sh -p shamir-index --full` и
`./scripts/test.sh @engine`.

Оценка: 4–6 дней.

---

## Фаза 2 — Filtered ANN

Цель: `vector_similarity(..) AND предикаты` — корректно и быстро.
Дизайн-док относил pre-filter к «research-level» — снимаем это:
post-filter с адаптивным oversample — просто, pre-filter по
селективному вторичному индексу — тоже просто и точен. Сложен только
co-filter (обход графа с маской) — его в этой фазе НЕ делаем.

- **2.1. Семантика в плане запроса.** Движок распознаёт паттерн
  `And([VectorSimilarity, ...predicates])` и компилирует во внутренний
  `FilteredVectorQuery`. Сам enum `Filter` не ломаем; wire — только
  аддитивные опциональные поля.
- **2.2. Post-filter с адаптивным перезапросом.** Поиск k′ = k ×
  oversample (default 2×) → применить предикаты → если прошло < k,
  retry с k′ ×= 2 до cap (`MAX_TOPK`), затем вернуть что есть.
- **2.3. Pre-filter путь.** Если предикаты покрыты вторичным индексом
  и candidate set мал (порог, например ≤ 4096 — tunable), скорить
  кандидатов напрямую SIMD-ядрами (точный результат, HNSW не нужен).
- **2.4. Выбор пути.** Cost-based: оценка кардинальности candidate
  set из вторичного индекса; мал → pre-filter, велик/нет индекса →
  post-filter с oversample.
- **2.5. Builder API.** Комбинирование — существующим `and(..)`;
  новые методы запроса `.ef_search(n)`, `.oversample(f)` (Rust +
  TS-клиент, wire-parity тест как у replication DDL).
- **2.6. Тесты.**
  - корректность: filtered top-k == brute-force filtered ground truth
    (10K, разные селективности);
  - фильтр отсекает всё → пустой результат, без бесконечного retry;
  - recall под фильтром ≥ порога;
  - tx: staged vectors + фильтр по staged-полям;
  - property-тест на эквивалентность pre-/post-путей.
- **2.7. Бенч «payload filters»** (тот самый, которого не было у
  Сбера): селективность 0.1% / 1% / 10% / 50% на 100K и 1M, отчёт
  QPS + recall по каждой точке.
- **2.8. DoD.** recall@10 ≥ 0.9 при селективности 1% и latency ≤ 2×
  unfiltered (пороги уточнить по baseline фазы 0); отчёт в
  `docs/dev-artifacts/benchmarks/vector/`.

Оценка: 4–6 дней.

---

## Фаза 3 — Квантизация (SQ8, затем опционально binary)

Цель: память ×4 меньше при потере recall ≤ 1–2% — без этого 10M+
векторов не влезают в разумное железо.

- **3.1. Scalar quantization int8.** Per-dimension min/max
  (asymmetric); параметры квантизации — в заголовке снапшота (formata
  version bump, reserved-поля из 1.1). Хранение: квантованные вектора
  в графе + оригинальные f32 на диске для rescoring.
- **3.2. SIMD int8 kernels.** Расширить `simd.rs`: dot-product int8
  (AVX2 `maddubs`-путь, AVX-512 VNNI где есть, NEON `sdot`), scalar
  fallback. Та же дисциплина runtime-dispatch через `OnceLock`.
- **3.3. Rescoring.** top-k′ по квантованным расстояниям → пересчёт
  точных f32-расстояний кандидатов → финальный top-k.
- **3.4. DDL.** `.quantization("sq8")` в create_index билдере (Rust +
  TS), опциональное wire-поле; default — без квантизации.
- **3.5. Тесты.** recall drop ≤ 2% vs f32 на 100K (seeded); round-trip
  снапшота с квантизацией; DimMismatch и миграция форматов (снапшот
  без квантизации читается после включения — rebuild или lazy).
- **3.6. Бенч.** RSS / QPS / recall: f32 vs sq8 на 1M×768D.
- **3.7. Binary quantization** — отдельным шагом ПОСЛЕ SQ8 и только
  при реальной потребности в 100M+ (32× память, грубый префильтр +
  обязательный rescoring).

DoD: отчёт с тройкой (память, скорость, recall); квантизация строго
opt-in. Оценка: 4–5 дней (без binary).

---

## Фаза 4 — Batch upsert и компакция tombstone

Цель: bulk-загрузка миллиона эмбеддингов — минуты, не часы; recall не
гниёт от накопленных удалений. Наш пиллар O(x→0): batched+amortized.

- **4.1. `upsert_batch(&[(RecordId, &[f32])])`** в `VectorAdapter`:
  один `spawn_blocking` на пакет; внутри — параллельная вставка
  (`hnsw_rs` parallel insert). Коммит-путь `apply_committed_vectors`
  переводится на батч автоматически.
- **4.2. Интеграция с bulk-путём движка.** Батчевые insert-опы движка
  собирают вектора одного индекса в один пакет (commit Phase 5d уже
  батчует staged vectors — расширить до самого графа).
- **4.3. Метрика deleted_ratio.** `AtomicUsize`-зеркала live/deleted
  (НЕ `scc::len()` — банен как O(N), см. clippy.toml).
- **4.4. Фоновая компакция.** Порог deleted_ratio > N% (tunable) →
  rebuild графа рядом → atomic swap (ArcSwap-паттерн). Reads не
  блокируются; writes на время rebuild — в дельту с доигрыванием
  перед swap. **Design decision в брифе:** дельта-доигрывание vs
  короткая write-пауза на один индекс; критерий — сложность против
  реальной частоты компакций.
- **4.5. Тесты.** bulk 100K: время vs по-одному (ассерт на ускорение);
  корректность поиска во время компакции (concurrent property-тест);
  компакция не теряет staged/committed вектора.
- **4.6. Бенч.** Bulk-load 1M: до/после; поиск при 30% deleted:
  до/после компакции.
- **4.7. DoD.** Bulk-загрузка ≥ 5× быстрее пути по-одному (порог
  уточнить по baseline); отчёт в `docs/dev-artifacts/benchmarks/vector/`.

Оценка: 3–5 дней.

---

## Фаза 5 — Query-time контроль качества

Маленькая фаза, можно вставить раньше по обстоятельствам.

- **5.1. Per-query `ef_search`.** Опциональное поле в
  `IndexQuery::Vector` и `Filter::VectorSimilarity` (аддитивный wire);
  default — из конфига индекса. Клиент крутит recall/latency
  trade-off на запрос, не пересоздавая индекс.
- **5.2. `$score` до конца.** Довести runtime-оценку в результатах по
  дизайн-доку: engine → сервер → TS-клиент, e2e-тест на наличие и
  порядок.
- **5.3. Лимиты.** Пересмотреть `MAX_TOPK` с учётом oversample из
  фазы 2 (совокупный cap, защита от DoS сохраняется).
- **5.4. Доки.** Обновить `docs/guide-docs/guide/06-search.md` (ef_search,
  oversample, $score, квантизация).

Оценка: 1–2 дня.

---

## Фаза 6 — Hybrid search (FTS + vector, RRF)

**Блокирована**: требует FTS (`FULL_TEXT_SEARCH.md` — planned, no
code). Когда FTS Phase 0 появится:

- **6.1. Контракт.** Оба поиска уже отдают `IndexResult::Ranked` —
  зафиксировать это как интерфейс fusion ещё в фазе 2 (комментарий в
  коде, без новых типов).
- **6.2. RRF combiner.** `score = Σ 1/(rrf_k + rank_i)`, `rrf_k = 60`
  default; чистая функция над двумя ranked-списками, юнит-тесты на
  детерминизм и tie-breaking.
- **6.3. Query op.** `HybridSearch { vector, text, k, fusion }` —
  новый op в query-types + билдеры (Rust/TS, wire-parity).
- **6.4. Executor.** Два поиска конкурентно (`tokio::join!`), fusion,
  top-k. Один из поисков пуст → деградация до второго.
- **6.5. Бенч.** hybrid vs pure-vector vs pure-FTS: качество на
  размеченном мини-корпусе + латентность.

Оценка: 2–3 дня (после FTS).

---

## Сквозные требования (каждая фаза)

- **TDD**: red → green → refactor; тесты в `tests/`-каталогах модулей
  по принятой раскладке.
- **Prompt-first**: бриф в `docs/dev-artifacts/prompts/vector/<NN>-<name>.md`,
  коммит брифа до запуска агента, в брифе — стандартный запрет
  git-мутаций (verbatim-блок из CLAUDE.md).
- **Тесты** только через `./scripts/test.sh` (`-p shamir-index`,
  `@engine`, `-- <filter>`); добавить scope `@vector`
  (shamir-index + shamir-engine) в `scripts/test.sh::scope_args`.
- **Бенчи** только с `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench`;
  гейт (fmt --check, clippy -D warnings, test --lib) — один раз в
  конце цикла, не между прогонами.
- **Пиллары**: lock-free (ArcSwap/scc/atomics), `spawn_blocking` для
  CPU-bound, batched+amortized, `THasher`, запрет `scc::len()`.
- **Wire-совместимость**: только аддитивные опциональные msgpack-поля;
  каждый новый билдер — Rust + TS с wire-parity фикстурой (паттерн
  replication DDL).
- **Формат снапшота** версионируется с фазы 1; фаза 3 повышает
  версию — миграция явная, старый формат читается или честно
  отправляется в rebuild.

---

## Release Benchmark Checklist

Публикуется с каждым релизом, затрагивающим векторную подсистему —
прямой ответ на вопросы, оставшиеся без ответа в кейсе Platform V:

- [ ] версии: shamir-db, `hnsw_rs`, rustc; commit SHA прогона;
- [ ] железо: CPU (+флаги SIMD), RAM, диск, ОС;
- [ ] датасет: генератор + seed (или публичный датасет + версия),
      размеры ступеней 10K / 100K / 1M (+10M stretch), dim 128 / 768;
- [ ] QPS, latency p50/p95/p99, recall@1/@10 на каждой ступени;
- [ ] build time и cold-start time (snapshot vs rebuild);
- [ ] RSS по ступеням; с квантизацией и без;
- [ ] filtered search: селективность 0.1% / 1% / 10% / 50%;
- [ ] regression-сравнение с предыдущим релизом (та же машина, тот же
      seed);
- [ ] single-node vs replicated read path — когда репликация
      (см. `REPLICATION.md`) доедет до векторных индексов;
- [ ] инструкция воспроизведения одной командой
      (`scripts/bench-vector.sh`).

---

## Суммарная оценка

| Фаза | Что | Оценка |
|---|---|---|
| 0 | Bench baseline + гигиена доков | 2–3 дня |
| 1 | Persisted HNSW | 4–6 дней |
| 2 | Filtered ANN | 4–6 дней |
| 3 | Quantization SQ8 | 4–5 дней |
| 4 | Batch upsert + compaction | 3–5 дней |
| 5 | Query-time контроль | 1–2 дня |
| 6 | Hybrid RRF (после FTS) | 2–3 дня |

Итого ядро (0–5): **~3–4 недели** сфокусированной работы; фаза 6 —
после FTS Phase 0.

---

## Поправки K1–K8 (план vs код)

> Разведка 2026-07-04 проверила каждое утверждение этого плана против кода.
> Полная таблица поправок живёт в
> **`VECTOR_PRODUCTION_EXECUTION.md` → «Поправки к плану-доку (проверено кодом)»**
> (K1–K8); этот раздел — краткая выжимка, чтобы старый план не вводил в
> заблуждение. Не дублирует весь EXECUTION-план — только факты, на которых
> завязаны фазные границы и зависимости выше.

| # | План утверждал | Код показывает | Влияние на фазы |
|---|---|---|---|
| **K1** | «hnsw_rs 0.3 не имеет dump/load, batch, seed» | `Cargo.lock` = **hnsw_rs 0.3.4**. Спайк V0.0 подтвердил: `file_dump` / `HnswIo::load_hnsw_with_dist` / `parallel_insert` / `search_filter` + `FilterT` **есть**; seed API нет; **`Distance<i8>` ОТСУТСТВУЕТ** (только `DistL1`/`DistL2` для u8 — НЕ Dot/Cosine). | Фаза 1 (persist) разблокирована через hnswio-обёртку; фаза 3 (SQ8) идёт через `Hnsw<i8,_>` + L1/L2, не через Dot-int8. Пин `=0.3.4` ради стабильности dump-формата. |
| **K2** | «Фаза 6 (Hybrid) блокирована: FTS нет кода» | **Устарело.** FTS уже в дереве: `crates/shamir-index/src/{bm25,fts_backend,fts_ranked_backend}.rs`, `Filter::Fts` в query-types. | **P6 (Hybrid RRF) разблокирована.** В текущую кампанию (P0–P5) всё равно не входит, но больше не ждёт FTS Phase 0. |
| **K6** | «snapshot writer → atomic rename» (фаза 1.2) | Atomic-rename паттерна в движке нет. Есть **chunk-persist** через `InternerManager` (HWM + zero-padded chunks + `MetaEnvelope` SDB2); `Store::transact` атомарно флипает поколение манифеста. | Снапшот HNSW пишется **в info_store chunks**, не отдельным файлом с rename. Это меняет реализацию фазы 1, но не её DoD. |
| **K8** | (не было) | `HnswAdapter::len()` зовёт `self.deleted.len()` (O(N)) на **каждом** search — это точка выбора brute-vs-graph на hot path. | Скрытый O(N) на hot path. Чинится atomic-зеркалами live/deleted (`AtomicUsize`) в **P4 / EXECUTION лист 4.1 (#407)** — не отдельная фаза, а часть batch-upsert+compaction. |
| K3 | seed API у hnsw_rs нет | Подтверждено (`hnsw_adapter.rs`). `BRUTE_FORCE_MAX=256` остаётся; recall-тесты статистические на ≥1k запросов. | Без изменений — recall@10 в baseline зафиксирован честно (см. ниже). |
| K7 | нет checksum в index | `crc32fast` уже в workspace (shamir-wal). | Снапшот использует crc32 (не xxh3) — dep уже есть. |

### Что это значит для recall-чисел baseline

Baseline (фаза 0 / `docs/dev-artifacts/benchmarks/vector/2026-07-05-baseline.md`) фиксирует
**recall@10 ≈ 0.55–0.72 при `ef_search=50`** (дефолт индекса). Это **честный
сигнал**, не дефект: clustered-датасет не льстит recall, а поиск идёт на
статическом `ef_search=50`. **Per-query `ef_search` (P1 / EXECUTION лист 1.1,
#399)** — это тот самый рычаг, который поднимет recall на том же графе за счёт
латентности; baseline служит «до» для этого сравнения. Не трактовать эти числа
как потолок recall.

### Сдвиг фазных границ (исполнительский план)

Фазы переупорядочены в EXECUTION-плане: `upsert_batch` вытащен в P0 (без него
100K-ступени baseline не построить); per-query `ef_search` (бывш. фаза 5)
сдвинут в **P1** — сигнатура `search` расширяется до `SearchOpts` один раз, до
навешивания персиста и фильтров; квантизация (P5) после компакции (P4), т.к.
компакция обязана быть quantization-aware. См. диаграмму в
`VECTOR_PRODUCTION_EXECUTION.md` → «Последовательность фаз (скорректирована)».

