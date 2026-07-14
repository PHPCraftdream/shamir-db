בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Bench inventory — worst-case map (Фаза bench-infra A, #132)

Дата: 2026-06-21. Цель — зафиксировать какие benches могут залипать (>10s/cell
или unbounded). Это диагностика, не кода.

---

## 1. Общая статистика

- **41 bench-файла** в `crates/*/benches/`.
- **20 файлов** используют `shamir_bench_utils::tune()` (наша default infra).
- **9 файлов** содержат намёки на large N (1M+ записи).
- **1 файл** (`read_path_matrix`) — единственный с env-gated huge mode
  (`BENCH_READ_PATH_HUGE`).

**Главный вывод:** нет общего верхнего bound'а на per-cell wall-clock.
Конкретные benches самостоятельно решают tune()-параметры; за ними никто
не следит.

---

## 2. `bu::tune()` invocations (FULL-mode budget)

```
backend_matrix       tune(50, 5s, 3s)   — 63 cells × (15s/cell?) → unbounded в FULL
drain_throughput     tune(10, 1s, 1s)   — QUICK, OK
durable_concurrent_commit  tune(10, 1s, 1s)  ×2 calls — QUICK
read_path_matrix     tune(10, 1s, 1s)   — но shape 5 = 7s/iter, разрыв
tx_pipeline          tune(100, 5s, 3s)  ×4 calls — heavy
tx_pipeline          tune(10, 15s, 3s)  — single_insert
tx_pipeline          tune(20, 8s, 3s)
tx_pipeline          tune(30, 5s, 3s)
wire_pipelining      tune(30, 5s, 3s)
recordview_lens      tune(10, 1s, 1s)   ×3 — QUICK
wal_append           tune(50/30/20, 2s, 1s)
```

`bu::tune(sample_size_default, measurement_secs, warm_up_secs)` — параметры
применяются в FULL-режиме. В QUICK (default) — фиксированно `sample=10,
measurement=1s, warm_up=1s`. Это значит: **в QUICK режиме большинство
benches должны быть terminate'нуты в разумное время**.

**Но**: criterion warning «Unable to complete 10 samples in 1.0s» НЕ
останавливает execution — она просто warn'ит и **продолжает** до полного
завершения 10 sample'ов, что может занять часы (это мы видели Phase 3).

---

## 3. Известные slow / залипающие cells

Зафиксированы за read-path кампанию (Phase 0-3):

| Bench / cell | Wall-clock | Причина |
|---|---|---|
| `read_path_matrix/s1_asof/100K` (full Shape 5) | **~5,455s/sample est** (1.5h) | Full snapshot read at ts, не isolated ts_index |
| `read_path_matrix/s3_range_and/1M` (non-selective) | **95s/sample** | Phase 2 wrong choice tipping point |
| `read_path_matrix/s2_no_index/1M` | 16.5s/sample | Full materialize+sort |
| `read_path_matrix/s2_s3_combo/1M` | 18.3s/sample | WHERE+ORDER BY no index |
| `read_path_matrix/fast_path/1M` (pre-fix) | 14.8s/sample | Bench misconfig (fixed in Phase 1) |
| `backend_matrix/*/w=128/b=100` (FULL) | 1.5h est | sample=50, 63 cells |
| `tx_pipeline/single_insert` (FULL) | 15s × 10 samples | 15s measurement |

**Паттерн:** залипания случаются когда **первая итерация cell'а** превышает
1s (QUICK measurement budget). Criterion warn'ит и расширяет measurement до
estimated time, что может быть час+. Нет hard upper bound.

---

## 4. Категоризация по риску

### 🔴 High-risk (могут залипнуть на часы)

- `read_path_matrix/s1_asof/100K+` — gated за `BENCH_READ_PATH_HUGE` (✅ guard).
- `read_path_matrix/s3_range_and/1M` (non-selective) — НЕТ guard'а, видим
  35s × 10 samples.
- `read_path_matrix/s2_no_index/1M`, `s2_s3_combo/1M` — НЕТ guard'а.
- `backend_matrix` (FULL) — НЕТ guard'а от 63 ячейки × 15s.
- `tx_pipeline` (FULL) — несколько `tune(100, 5s, 3s)` calls.

### 🟡 Medium-risk (cell может быть медленным, но bounded)

- `drain_throughput` — sample=10, 1s measurement; concurrent paths могут
  растянуть до 30s/cell.
- `wal_append` — sample=50, 2s measurement; bounded.
- `durable_concurrent_commit` — sample=10, 1s; bounded.

### 🟢 Low-risk (явно QUICK или micro-ops)

- `recordview_lens`, `select_*`, `filter_eval`, `distinct`, `group_by_keys`
  — micro-benches на data structures.
- `record_id`, `codec_msgpack` — sub-µs ops.

---

## 5. Главные находки

1. **`bu::tune()` не имеет верхней границы на per-cell wall-clock.** Только
   number of samples + measurement budget. Если 1 iter > budget — criterion
   расширяет, не отбойничает.

2. **Только `read_path_matrix` имеет env-gate** (`BENCH_READ_PATH_HUGE`).
   Все остальные heavy benches запускают все cells по умолчанию.

3. **Нет SMOKE mode.** Нет способа быстро прогнать все benches «для проверки
   что компилируется и работает». QUICK уже занимает минуты на heavy benches.

4. **Нет documented per-bench expected wall-clock.** Cold start: «прогон
   bench X займёт Y минут» — нигде не записано.

5. **`tx_pipeline/single_insert` `tune(10, 15s, 3s)`** — намеренно медленный
   single insert, 15s measurement × 10 samples = 150s/cell минимум.

---

## 6. Что нужно для bench-infra B (#133)

На основе этой диагностики, B должен:

1. **Добавить `max_wall_secs_per_cell`** к `tune()`-семейству. Когда
   estimated time превышает hard budget — criterion должен либо снизить
   sample count (до минимума 3?), либо просто bail-out с warning.

2. **Добавить SMOKE mode** (`BENCH_SMOKE=1`):
   - sample = 3 (minimum)
   - measurement = 500ms
   - warm_up = 200ms
   - Per-cell hard timeout: 5s.

3. **`tune_tiered()` signature**:
   ```rust
   pub fn tune_tiered<M: Measurement>(
       group: &mut BenchmarkGroup<'_, M>,
       full_defaults: TuneParams,  // sample, measure_secs, warm_secs
       max_wall_secs: u64,         // hard upper bound per cell
   );
   ```

   Backward-compat с `tune()` оставить.

4. **Документировать в `shamir-bench-utils`**: явные tier'ы и contract.

---

## 7. Конкретные cells для cap'ания в bench-infra C (#134)

| Bench / cell | Текущее | Предлагаемое |
|---|---|---|
| `read_path_matrix/s2_no_index/1M+` | unbounded 16.5s | gated за `BENCH_HUGE` |
| `read_path_matrix/s2_s3_combo/1M+` | unbounded 18s | gated за `BENCH_HUGE` |
| `read_path_matrix/s3_range_and/1M` | unbounded 95s | gated за `BENCH_HUGE` |
| `backend_matrix` FULL `(50, 5s, 3s)` | 63 cells × 15s | cap N=128 за `BENCH_HUGE` |
| `tx_pipeline single_insert` `(10, 15s, 3s)` | 15s × 10 | tune'ить меньше, или env-gate |

---

## 8. Гейт #132 — закрыт

✅ 41 bench inventory.
✅ Worst-case cells найдены.
✅ Корневая причина (no per-cell wall-clock bound) задокументирована.
✅ Конкретный план для B и C сформулирован.
