בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Backend decision — сокращение до 2 production backends

Дата: 2026-06-19. На основании `backend-matrix-snapshot-2026-06-19.md`.

---

## Решение

Сократить с 6 до 2 production backends:

### ✅ Keep (production-grade)

- **fjall** — default durable LSM. 188K e/s на batch=100/w=32 (steady-state).
  LSM-форма (memtable→SST + leveled compaction) ложится ровно в наш
  drain-профиль: per-transact cost ~30µs, sequential append, lock-free reads.
  Активно поддерживается (Marvin Schmidt).

- **redb** — B-tree alternative, system_store, embedded small-data workload.
  13K e/s на batch=100/w=32 — 14× медленнее fjall на drain-профиле, но:
  - system_store у нас сейчас на redb (Phase 4 не свапнул; малый объём,
    B-tree оптимален для метаданных).
  - B-tree range-read обычно быстрее SST scan на read-heavy с малыми writes.
  - Embedded use case с фиксированным маленьким объёмом — никогда не
    trigger flush/compact циклов LSM.
  - Кросс-валидация Store trait abstraction: тесты на двух backend формах
    ловят баги в trait API, которые на одном не видны.

### ❌ Drop (медленные / нестабильные / unmaintained)

| Backend | Причина | Числа (batch=100/w=32) |
|---------|---------|------------------------|
| **sled** | unmaintained (0.34.7, 2021) + огромная variance (15K..248K e/s на w128/b100, 16× spread). p99 latency непредсказуема, SLA не построишь. | 205K mean (обманчиво) |
| **nebari** | 38× медленнее fjall на batch. BonsaiDb append-only B+ tree. | 5.3K e/s |
| **persy** | 20× медленнее fjall. Single-file transactional B+ tree. | 9.3K e/s |
| **canopy** | 3× медленнее fjall, не уникален в нише (B-tree variant). | 56K e/s |

---

## Обоснование

### Почему B-tree формы (redb/nebari/persy) медленные под наш профиль

Каждый `Store::transact()` для B-tree = полный transaction ceremony:
begin_write → global write lock → page splits → B-tree balance → per-page
checksums (xxh3 у redb) → page-table flush → release lock. ~745µs minimum
per transaction (Phase 0 measured).

LSM (fjall): append в memtable (in-memory skip-list) → ~30µs per transact.
15-50× дешевле per-transact.

Наш L1 drainer в realtime с 32 concurrent коммиттерами обычно собирает
1-3 tx per drain pass (не 32) → per-transact cost **доминирует**. LSM
per-transact cost минимален → 14× разница.

### Почему sled обманчиво быстрый

sled mean = 205K e/s, но distribution катастрофический:
- `sled/w128/b10`: min 3.3K, max 90K (27× spread)
- `sled/w128/b100`: min 15K, max 248K (16× spread)

p50 от p99 отличается на порядок. Нестабильность не лечится (unmaintained
codebase, log-structured GC unpredictable). SLA на нём невозможен.

---

## Что даёт сокращение

- ~50K LOC меньше в зависимостях (sled + nebari + persy + canopy).
- Быстрее clean build (несколько минут экономии).
- Меньше supply-chain attack surface.
- Тестовая матрица упрощается.
- Маленькая стоимость поддержки — redb уже работает, тесты зелёные.

---

## План действий (если принято)

1. Удалить `storage_sled.rs`, `storage_nebari.rs`, `storage_persy.rs`,
   `storage_canopy.rs` из `crates/shamir-storage/src/`.
2. Убрать соответствующие `[dependencies]` и features из `Cargo.toml`
   (shamir-storage, shamir-engine, shamir-db).
3. Убрать `BoxRepoFactory::{sled,nebari,persy,canopy}{,_raw}` методы.
4. Переключить bench'и которые ещё на `sled_raw` на `fjall_raw`.
5. Обновить docs/research где они упоминаются.
6. Удалить `backend_matrix` ячейки для удаляемых backends (или оставить
   как «история» с пометкой).
7. Гейт (@oracle + @e2e --full) + push.

---

## Открытый вопрос

Если sled maintenance когда-либо возобновится (или появится stable fork) —
пересмотреть: его абсолютный потенциал (mean 205K) выше всех backend'ов.
Но без maintenance guarantee и с variance которую имеем — это **азартная
игра, не инженерное решение**.
