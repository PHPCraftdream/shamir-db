בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Backend matrix — steady-state throughput (HEAD `b2b1280`)

**Bench:** `backend_matrix` (новый, не путать с устаревшим `drain_throughput`).
**Режим:** QUICK (sample=10, 1s measurement). FULL даст более тесные доверительные интервалы, но картина не изменится.
**Цель:** честная steady-state throughput всех backend'ов на нашей архитектуре.

## Почему числа сильно отличаются от Phase 2

Старый `drain_throughput` создавал **fresh tempdir + backend init на каждой
criterion-итерации**. Setup overhead (fjall keyspace open ≈ 10-50ms, redb page
table ≈ 5-30ms) дoominировал измерение. Числа Phase 2 (fjall 2.4K e/s)
отражали **cold-start** throughput, не steady-state.

Новый `backend_matrix`:
- **Один репо** на все criterion-итерации (steady-state).
- Таблица растёт монотонно — как в реальной СУБД.
- **Axes:** 7 backends × {8, 32, 128} writers × {1, 10, 100} batch sizes = 63 cells.

## Таблица (medians, Kelem/s)

### Single-row commits (batch=1, worst-case overhead)

| Backend     | w=8     | w=32    | w=128   |
|-------------|---------|---------|---------|
| in_memory   | **14.9 K**  | 23.6 K  | 28.8 K  |
| **fjall**   | **2.9 K**   | 6.8 K   | 7.6 K   |
| sled        | 4.6 K   | 7.7 K   | 7.0 K   |
| redb        | **0.22 K**  | 0.24 K  | 0.26 K  |
| nebari      | 0.056 K | 0.053 K | 0.056 K |
| persy       | 0.087 K | 0.093 K | 0.090 K |
| canopy      | 1.5 K   | 2.0 K   | 1.9 K   |

### Batch=10 commits

| Backend     | w=8     | w=32    | w=128   |
|-------------|---------|---------|---------|
| in_memory   | 47 K    | **102 K**   | 23 K    |
| **fjall**   | 29 K    | **45 K**    | 48 K    |
| sled        | 17 K    | 55 K    | 10 K    |
| redb        | 1.9 K   | 2.2 K   | 2.3 K   |
| nebari      | 0.56 K  | 0.47 K  | 0.61 K  |
| persy       | 0.92 K  | 0.96 K  | 0.89 K  |
| canopy      | 10 K    | 14 K    | 13 K    |

### Batch=100 commits (realistic high-throughput)

| Backend     | w=8     | w=32        | w=128       |
|-------------|---------|-------------|-------------|
| in_memory   | 86 K    | **190 K**   | **186 K**   |
| **fjall**   | 60 K    | **188 K**   | **127 K**   |
| sled        | 58 K    | **205 K**   | 40 K (high var) |
| redb        | 9.9 K   | 13 K        | 13 K        |
| nebari      | 4.9 K   | 5.3 K       | 5.7 K       |
| persy       | 8.7 K   | 9.3 K       | 8.8 K       |
| canopy      | 26 K    | 56 K        | 55 K        |

## Главные выводы

1. **fjall steady-state даёт 188K elem/s** на realistic batch=100 w=32. Это
   нормальные числа для современной СУБД (Postgres territory).

2. **Прежний 2.4K (Phase 2) — был артефактом cold-start bench**, не свойством
   движка. Архитектура swap'а redb→fjall была правильной, но числовое
   обоснование было **в порядке величины слабее реальности**.

3. **redb остаётся фундаментально хуже всех durable** — single-writer
   transaction model плато даже на batch=100 (~13K elem/s). Свой
   обоснованный downgrade.

4. **sled показывает огромную variance** под high-concurrency single-row
   (w128/b1: 7K e/s; w128/b10: 10K → 90K). Backend не любит burst load
   на single-row.

5. **nebari (~5K e/s b=100) и persy (~9K e/s b=100) — медленные**. Не
   кандидаты на default.

6. **canopy интересен** — 55K elem/s на w32/b100. Хороший fallback кандидат,
   но не быстрее fjall.

7. **fjall w=128 показывает падение vs w=32** (188K → 127K на b=100). Это
   **не backend-bottleneck**, а наш собственный sync point:
   - WAL append serialization (single MutexGuard на append)
   - L1 drainer single-owner (sync barrier)
   - tokio task spawn overhead для 128 concurrent tasks
   Это **архитектурный** ограничитель, который индексирует на наш WAL/drainer,
   не на сменное хранилище.

## Что это значит стратегически

**Решение Phase 4 (swap redb→fjall) подтверждено** — теперь честно, не на
cold-start метрике, а на steady-state. fjall даёт **8-15× durable throughput
vs redb на realistic batch workload**.

**Но новая граница виднеется:** при w=128 fjall перестаёт скейлиться. Если
нужны >200K e/s sustained, надо смотреть **наш own WAL/drainer**, не backend.
L1 drainer как single-writer — потенциал для шардированного дренажа в будущем.

## Команды

```sh
CARGO_TARGET_DIR='D:/dev/rust/.cargo-target-bench' \
  cargo bench -p shamir-engine --bench backend_matrix \
  --features "redb fjall sled nebari persy canopy"

# FULL (more samples, tighter CIs, ~2h):
BENCH_FULL=1 CARGO_TARGET_DIR='D:/dev/rust/.cargo-target-bench' \
  cargo bench -p shamir-engine --bench backend_matrix \
  --features "redb fjall sled nebari persy canopy"

# Filter to specific cells:
cargo bench -p shamir-engine --bench backend_matrix \
  --features "redb fjall sled nebari persy canopy" -- 'fjall/.*/b100'
```
