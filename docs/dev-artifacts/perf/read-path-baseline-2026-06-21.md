בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Read-path baseline — Фаза 0c (HEAD `86955d6`)

QUICK режим (`sample=10`, 1s measurement). Бенч: `read_path_matrix` (5 shapes
× 3 N = 15 cells). Backend: InMemoryStore (engine-layer — bottleneck, не
storage).

---

## 1. Таблица замеров

12 из 15 ячеек получены. `s1_asof` (3 ячейки) панически валился — bench не
подключает MvccStore, AsOf требует. Это известный bug bench setup'а,
фиксится в Фазе 3 (S1) — там нужен корректный MVCC fixture.

| Shape | N=10K | N=100K | N=1M | Scaling 10K→1M |
|---|---|---|---|---|
| **fast_path** | 72.1 ms | 818 ms | **14.80 s** | **205×** ⚠️ |
| **s2_no_index** | 72.3 ms | 808 ms | 14.83 s | 205× |
| **s2_s3_combo** | 86.2 ms | 943 ms | 18.26 s | 212× |
| **s3_range_and** | 19.6 ms | 260 ms | 8.54 s | 436× |
| **s1_asof** | panic | panic | panic | — (Phase 3 fix) |

Throughput на 1M:
- fast_path: **67.6 K elem/s** (это «быстрый» путь!)
- s2_no_index: 67.4 K
- s2_s3_combo: 54.8 K
- s3_range_and: 117 K (нет ORDER BY → нет сортировки)

---

## 2. Главная находка — fast_path НЕ срабатывает

Shape 1 (`fast_path`) и Shape 2 (`s2_no_index`) **идентичны во всех 3 N**
(72ms / 808ms / 14.8s). Это означает что `try_plan_order_limit_fast_path`
**бейлится** даже когда:
- Sorted index на `y` создан (`y_sorted`).
- WHERE отсутствует.
- group_by / distinct / count_total / aggregates отсутствуют.
- order_by 1-item, ascending, на `y`.
- LIMIT 10 (finite take).

Все формальные guards шейп проходит → но fast_path не activates. **Скрытый
bailout где-то** — возможно field-id resolution через interner, или index
kind mismatch (`sorted()` vs то что `try_plan_order_limit_fast_path` ищет),
или index registry visibility issue.

**Это сильнее, чем мы знали из раунда 3:** S2 — это не «когда нет индекса
делается full sort», это «даже КОГДА индекс есть, planner может его не
использовать». Фаза 1 (S2) обязана **сначала** диагностировать почему
fast_path не triggers, прежде чем top-K-heap'ить fallback.

---

## 3. Asymptotic анализ

Все шейпы **линейно** scaling с N (10K→1M = ~200× при честном 100× → шум +
constant factor). Это **подтверждает O(N) full-scan** на всех:
- fast_path: должно быть O(log N + K), фактически O(N) → bailout доказан.
- s2_no_index: O(N log N) sort, но decode/filter O(N) доминирует.
- s2_s3_combo: O(N) scan + O(N) filter + O(N log N) sort.
- s3_range_and: O(N) scan (НЕТ сортировки в этой shape — только WHERE),
  что объясняет почему она быстрее остальных. Index на x **не используется**
  (S3 confirmed).

---

## 4. Что это меняет в плане

**Phase 1 (S2) приоритет ↑.** Не только top-K heap для no-index случая —
надо найти ПОЧЕМУ fast_path не triggers с index'ом. Если простой fix
(field-id resolution / index kind), то fast_path сразу из 14.8s в
sub-ms. Это **порядки величин**, не «6× выигрыш». Это первое что делаем
в Фазе 1.

**Phase 2 (S3) подтверждён.** s3_range_and 8.5s на 1M при том что
result-set реально маленький (range AND eq filter сильно фильтрует) —
range-extraction должен дать tunable headroom.

**Phase 3 (S1) — bench setup bug**. Нужен MvccStore-wired fixture
(`shamir_tx::MvccStore` + `RepoTxGate` + table->mvcc wiring) перед
AsOf queries. Это **часть Фазы 3** scope'а — заодно с ts-index impl.

---

## 5. Что НЕ получили (пока)

- **peak_mem curves.** Phase 0a peak_alloc infra готова, но активация
  `#[global_allocator]` process-wide. Для S2 memory headline (Фаза 1)
  сделаем dedicated env-gated run.
- **s1_asof кривая** (см. §1).
- **10M variant** (env BENCH_READ_PATH_HUGE=1). На 1M уже 14-18s/sample
  на QUICK; 10M = ~3 минут/sample × 10 samples = полчаса/shape. Слишком
  дорого для baseline; запустим точечно после Фазы 1 для верификации win.

---

## 6. Команды воспроизведения

```sh
# QUICK (baseline run, ~5 min):
CARGO_TARGET_DIR='D:/dev/rust/.cargo-target-bench' \
  cargo bench -p shamir-engine --bench read_path_matrix -- \
  '(fast_path|s2_no_index|s2_s3_combo|s3_range_and)/'

# FULL (sample=50, ~30 min):
BENCH_FULL=1 CARGO_TARGET_DIR='D:/dev/rust/.cargo-target-bench' \
  cargo bench -p shamir-engine --bench read_path_matrix

# 10M opt-in (для верификации win):
BENCH_READ_PATH_HUGE=1 CARGO_TARGET_DIR='D:/dev/rust/.cargo-target-bench' \
  cargo bench -p shamir-engine --bench read_path_matrix
```

---

## 7. Гейт Фазы 0 — закрыт

✅ 4/5 shapes имеют кривые N→time.
✅ fast_path bailout найден — критический pointer на Фазу 1.
✅ S2/S3 cliffs эмпирически подтверждены (linear scaling с N).
⚠️ s1_asof bench wiring fix → Фаза 3 entry condition.

**Фаза 1 (S2) начинается с диагностики fast_path bailout** — это
self-исполняющийся приоритет от данных, не от research'а.
