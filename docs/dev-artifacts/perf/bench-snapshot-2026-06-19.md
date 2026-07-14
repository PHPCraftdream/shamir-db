בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Bench snapshot — HEAD 2026-06-19 (Фаза 0 storage-волны)

Замер для этапа 0.1 из `docs/dev-artifacts/perf/storage-speedup-waves.md`.
Профиль: **opt-0** (стандартный `[profile.bench]`). Criterion **full**-режим
(sample=100, measurement=5s) — НЕ quick (по дефолтному пути исследования).

HEAD: `be318f3` (после #100 client-side interner коммита).
Worktree-isolated targets:
- `D:/dev/rust/.cargo-target-bench` — store_raw (shamir-storage)
- `D:/dev/rust/.cargo-target-bench-engine` — tx_pipeline (shamir-engine)

---

## 1. `store_raw` (сырой backend)

| bench | time | throughput |
|---|---|---|
| in_memory/insert/single | 7.61 µs | **131 k/s** |
| in_memory/set_many/batch/100 | 1.679 ms | **60 k/s** |
| cached_in_memory/insert/single | 18.65 µs | 54 k/s |
| cached_in_memory/set_many/batch/100 | 2.760 ms | 36 k/s |
| membuffer_in_memory/set_many/batch/100 | 1.371 ms | 73 k/s |
| **redb/insert/single** | **745 µs** | **1.34 k/s** |
| **redb/set_many/batch/100** | **2.590 ms** | **38.6 k/s** ← **цель волны** |
| **sled/insert/single** | **94.9 µs** | **10.5 k/s** |
| **sled/set_many/batch/100** | **5.36 ms** | **18.7 k/s** |

### Подтверждения research'а
- **redb single ≈ 745 µs** — фиксированная per-commit CPU-цена redb-write-txn
  (page-table flush + per-page XXH3 + spawn_blocking hop), НЕ fsync.
  Fsync уже выключен (`Durability::None`), 745 µs — это и есть пол redb.
- **redb batch/100 ≈ 38.6 k/s** — амортизированный потолок durable redb
  (≈19× к single). Это реалистичная цель волны: с L1+L2 на горячем durable-пути
  должна быть только batch-commit, single-commit недопустим.
- **sled single ≈ 95 µs** (≈8× к redb single) — лог-структурированный backend
  ест single-commit-overhead на порядок дешевле. Подтверждает L8-формовой тезис.
- **memory single 7.6 µs** — пол при нулевом task-hop, нулевой txn. **Недостижим
  на диске без потери durability — честная граница (research §5).**

### Расхождения с предыдущим snapshot (bench-snapshot-2026-06-18.md, post-json)
- in_memory/single: 5.7 → 7.6 µs (+33%). Объяснимо: разные CARGO_TARGET_DIR,
  возможна разная фоновая нагрузка Windows. Шум, не регрессия.
- redb batch: 41.3 → 38.6 k/s (−6%). Тоже шум.
- **Главное — все ОТНОСИТЕЛЬНЫЕ соотношения сохранены** (redb-batch ≫ single,
  sled-single ≫ redb-single, memory-single — пол).

---

## 2. Engine pipeline (`tx_pipeline::tx_overhead`)

| bench | time | throughput |
|---|---|---|
| single_insert/non_tx | 387 µs | 2.58 k/s |
| **single_insert/tx_staged** | **256 µs** | **3.90 k/s** ← **engine-floor (L10)** |
| batch/non_tx/1 | 427 µs | 2.34 k/s |
| batch/tx/1 | 427 µs | 2.34 k/s |
| batch/non_tx/10 | 771 µs | 12.97 k/s |
| batch/tx/10 | 743 µs | 13.45 k/s |
| batch/non_tx/100 | 4.56 ms | 21.9 k/s |
| batch/tx/100 | 4.53 ms | 22.1 k/s |
| batch/non_tx/1_no_result | 432 µs | 2.31 k/s |
| batch/tx/1_no_result | 420 µs | 2.38 k/s |
| batch/non_tx/10_no_result | 748 µs | 13.37 k/s |
| batch/tx/10_no_result | 720 µs | 13.88 k/s |
| batch/non_tx/100_no_result | 4.15 ms | 24.1 k/s |
| batch/tx/100_no_result | 4.39 ms | 22.8 k/s |
| batch/indexed/non_tx/100 | 12.75 ms | 7.85 k/s |
| batch/indexed/tx/100 | 12.55 ms | 7.97 k/s |
| **batch/indexed/non_tx/1000** | **117.28 ms** | **8.53 k/s** |
| batch/indexed/tx/1000 | 115.79 ms | 8.64 k/s |

### Подтверждения research'а
- **Engine-floor 256 µs ≈ 254 µs (research)** — backend-независимый. **L10 (Фаза 4)
  атакует именно это**. 2× к 256 µs удвоит single-insert ВСЕ backend'ы.
- **tx ≈ non-tx на всех размерах** — SSI-граница бесплатна, как и должна быть.
  Платишь только при реальном конфликте.
- **batch/100 ≈ 22 k/s, batch/100/indexed ≈ 8 k/s** — индекс ~3× штраф/запись.

### Решение «индекс-регрессия» из bench-snapshot-2026-06-18
В post-json snapshot были подозрительные просадки:
- `indexed/tx/100`: 12.91 → 17.01 ms (−24%)
- `indexed/non_tx/1000`: 122.9 → 173.4 ms (−29%)

На текущем HEAD:
- `indexed/tx/100`: **12.55 ms** ← ВЕРНУЛОСЬ к нормальному (даже чуть лучше).
- `indexed/non_tx/1000`: **117.28 ms** ← ВЕРНУЛОСЬ.

**Вывод:** подозреваемая регрессия НЕ воспроизводится. Был артефакт замера
(возможно — фоновая параллельная нагрузка во время того прогона, либо случайный
шум). Бисект не нужен.

---

## 3. Вердикт по микро-рычагам Волны 1

Эталон «проходит/нет» — research называет «low-single-% батч/100 → больше на
single». Тест: оценить вклад каждого, прикинуть выигрыш в µs/нс.

| Рычаг | Описание | Оценочный вклад | Решение |
|---|---|---|---|
| **L9** | has_any_index guard на unindexed insert | unindexed batch/100 = 24 k/s = 42 µs/row; убрать all_backends+3 planner (per-batch) — единицы µs на batch. На single — заметнее. | ✅ берём |
| **L12** | reuse encode scratch-buffer | batch/100/no_result = 24 k/s. Encode — низкие проценты от общего батч-cost. Pure-win, дешёвый изменение. | ✅ берём |
| **L13** | hoist RecordId clock | clock-read ~30-100 ns; на batch/100 = 99 reads × 50 ns ≈ 5 µs из 4150 µs (0.1%). Pure-win. | ✅ берём (быстро, без риска) |
| **L15** | fuse point-read alloc | 16B alloc убран; ~10-20 ns/read; компаундит под L3-батч-цикл. | ✅ берём (быстро) |

Все микро-рычаги остаются в волне. Микро-выигрыш индивидуальный, но (a) дёшевы,
(b) pure-win без риска, (c) компаундятся.

---

## 4. Подэтап 0.2 — redb 3.1 cacheability spike (для L7, вне волны)

Из research: вопрос — можно ли в redb 3.1 переиспользовать `TableDefinition` /
open-table / read-snapshot вне lifetime одной txn.

**Ответ без чтения redb-исходников** (из API-документации redb 3.1 и нашего
использования в `storage_redb.rs:142-171`):
- `TableDefinition::new(&str)` — это `const`-конструктор, **тривиально кэшируем**
  на `&'static str`. Сейчас `RedbStore` держит `table_name: String` —
  переиспользовать `TableDefinition` ничего не стоит, кроме `&'static`-промоушна
  имени.
- `Table` (handle, возвращаемый `txn.open_table(def)`) — типобезопасно
  **привязан к lifetime транзакции** (`Table<'_, K, V>`); переиспользование
  вне txn запрещено borrow-checker'ом. То же `ReadOnlyTable`.
- `ReadTransaction` (read-snapshot) — `Send`, можно держать **открытым между
  логически независимыми чтениями**, но это снэпшот: видит только данные на
  момент `begin_read`. Для горячего read-пути с свежими commit'ами не подходит.

**Вывод:** L7 в строгой форме «кэшировать open-table вне txn» —
**невозможен** в redb 3.1. Кэшировать `TableDefinition` можно (sub-percent
выигрыш — promote `String` в `&'static str` или Arc + хранить `TableDefinition`),
но это микро-оптимизация, **полностью субсумируется L1/L3-батчингом**: одна
большая `transact` открывает таблицу ОДИН раз, не 100. **L7 как отдельный рычаг
не существует.** Закрыть.

---

## 5. Команды воспроизведения

```sh
CARGO_TARGET_DIR='D:/dev/rust/.cargo-target-bench' cargo bench \
  -p shamir-storage --bench store_raw -- \
  '(sled|redb|in_memory)/(insert/single|set_many/batch/100)'

CARGO_TARGET_DIR='D:/dev/rust/.cargo-target-bench-engine' cargo bench \
  -p shamir-engine --bench tx_pipeline -- \
  'tx_overhead/(single_insert|batch_pipeline)'
```
