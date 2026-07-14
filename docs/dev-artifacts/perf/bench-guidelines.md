בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Bench guidelines

Дата: 2026-06-21. Bench-infra D (#135).

---

## 1. Tier'ы: SMOKE vs QUICK

| Tier  | Env trigger     | sample | measurement | warm_up | Когда               |
|-------|-----------------|--------|-------------|---------|---------------------|
| SMOKE | `BENCH_SMOKE=1` | 3      | 500 ms      | 200 ms  | CI smoke на каждый PR |
| QUICK | (default)       | 5      | 500 ms      | 500 ms  | Итеративная /opti    |

**FULL mode отключён.** `is_full()` всегда `false`. Для re-enable — правка
кода в `shamir-bench-utils/src/lib.rs`. Это сделано намеренно: FULL-режим
(sample=50-100, measurement=5-15s) убивал машину и вешал бенчи на часы.

---

## 2. Добавление нового bench — checklist

1. Файл: `crates/<crate>/benches/<name>.rs`.
2. `Cargo.toml` этого crate'а:
   ```toml
   [[bench]]
   name = "<name>"
   harness = false
   ```
3. Импорт:
   ```rust
   use shamir_bench_utils as bu;
   ```
4. Tuning (ОБЯЗАТЕЛЬНО в каждой benchmark group):
   ```rust
   bu::tune_tiered(&mut group, sample_size, measurement_secs, warm_up_secs, max_wall_secs);
   ```
   - `sample_size` — FULL-mode default (сейчас неактивно, но оставить для будущего).
   - `measurement_secs` / `warm_up_secs` — FULL-mode durations.
   - `max_wall_secs` — hard upper bound per cell (рекомендация: 30-120s).
5. Throughput: `group.throughput(Throughput::Elements(n as u64))` для каждого cell.
6. Heavy shapes (N >= 1M) — env-gate:
   ```rust
   if parse_bool_env("BENCH_HUGE") {
       sizes.push(1_000_000);
   }
   ```

---

## 3. Контракт `tune_tiered`

```rust
pub fn tune_tiered<M: Measurement>(
    group: &mut BenchmarkGroup<'_, M>,
    sample_size_default: usize,   // ignored (FULL disabled)
    measurement_secs: u64,        // ignored (FULL disabled)
    warm_up_secs: u64,            // ignored (FULL disabled)
    max_wall_secs: u64,           // per-cell guard (active only in FULL)
);
```

В текущем режиме (QUICK): всегда sample=5, measurement=500ms, warm_up=500ms.
В SMOKE: sample=3, measurement=500ms, warm_up=200ms.

Guard `max_wall_secs` — pre-computed estimate check. Если FULL
re-enable'нут и `sample × measurement > max_wall_secs`, sample
адаптивно снижается (min 3).

---

## 4. Shape design conventions

- **N tiering:** default sizes = `[10_000, 100_000]`. 1M+ gated за
  `BENCH_HUGE`. 10M+ gated за per-bench env (e.g. `BENCH_READ_PATH_HUGE`).
- **LIMIT:** каждый query-shape bench ДОЛЖЕН иметь LIMIT (10-100).
  Unbounded result sets — только для явного full-scan measurement.
- **Точки выхода:** если bench cell содержит heavy setup (repo init,
  index build), setup выносится за `b.iter()` — criterion измеряет
  только hot path.
- **Tempdir:** disk-backed benches используют `tempfile::TempDir`;
  drop ПОСЛЕ criterion finish (привязать к local variable в bench fn).

---

## 5. CI integration (рекомендация)

| Триггер | Tier | Env | Примерное время |
|---------|------|-----|-----------------|
| Каждый PR | SMOKE | `BENCH_SMOKE=1` | ~2-3 min (compile + sanity) |
| Nightly | QUICK | (default) | ~10-15 min |
| Weekly release | — | — | Не запускать без правки кода |

Команда:
```bash
# CI PR check:
BENCH_SMOKE=1 CARGO_TARGET_DIR=target-bench cargo bench --workspace

# Nightly:
CARGO_TARGET_DIR=target-bench cargo bench --workspace
```

Изоляция target dir обязательна (`CARGO_TARGET_DIR`) — bench artefacts
не должны конфликтовать с test/clippy incremental cache.

---

## 6. Запуск бенчей локально

```bash
# Итеративная работа (QUICK, default):
CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p <crate> --bench <name>

# Smoke check (compile + sanity):
BENCH_SMOKE=1 CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p <crate> --bench <name>

# С heavy shapes (1M+):
BENCH_HUGE=1 CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p <crate> --bench <name>
```
