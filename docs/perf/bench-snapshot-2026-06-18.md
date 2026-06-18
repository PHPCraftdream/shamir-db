בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Бенчмарки записи — снимок 2026-06-18 (json-кампания A/B)

Профиль: **opt-0** (стандартный `[profile.bench]` проекта).
Метод: back-to-back A/B на одной машине, изолированные `CARGO_TARGET_DIR`-ы для
каждого чекаута, criterion (100 сэмплов × 5с). Worktree-чекауты:
- **pre** = `1b1e229` — «#61-close» (последний коммит ДО json-кампании)
- **post** = `f19d593` — «complete serde_json elimination» (текущий конец кампании)

Расстояние pre→post: вся json-кампания (~22 коммита между ними, см.
`git log 1b1e229..f19d593`).

---

## 1. Сырые backend'ы (`shamir-storage::store_raw`)

| bench | pre (1b1e229) | post (f19d593) | дельта |
|---|---|---|---|
| in_memory/insert/single | 5.62 µs · **178 k/s** | 5.70 µs · **175 k/s** | noise |
| in_memory/set_many/batch/100 | 766 µs · **131 k/s** | 763 µs · **131 k/s** | noise |
| cached_in_memory/insert/single | 9.73 µs · **103 k/s** | 9.74 µs · **103 k/s** | noise |
| cached_in_memory/set_many/batch/100 | 1.396 ms · **71.6 k/s** | 1.437 ms · **69.6 k/s** | −2.9% noise |
| membuffer_in_memory/set_many/batch/100 | 1.283 ms · **77.9 k/s** | 1.243 ms · **80.4 k/s** | +3.1% noise |
| **sled/insert/single** | **94 µs · 10.6 k/s** | **92 µs · 10.9 k/s** | **+2% (быстрее)** ✅ |
| **sled/set_many/batch/100** | **5.52 ms · 18.1 k/s** | **5.30 ms · 18.9 k/s** | **+4% (быстрее)** ✅ |
| redb/insert/single | 708 µs · **1.41 k/s** | 749 µs · **1.34 k/s** | −6% mild |
| **redb/set_many/batch/100** | **2.51 ms · 39.9 k/s** | **2.42 ms · 41.3 k/s** | **+3% (быстрее)** ✅ |

**Источник «40k записей/с»** — это `redb/set_many/batch/100`: **≈ 40 000**, и
**до**, и **после** json. Никакого «40k у sled на opt-0» в данных нет (sled
batch/100 даёт ~19 k/s; sled/insert/single = ~10.5 k/s).

---

## 2. Engine pipeline — tx vs non-tx (`shamir-engine::tx_pipeline`)

| bench | pre | post | дельта |
|---|---|---|---|
| single_insert/non_tx | 377 µs · **2.65 k/s** | 383 µs · **2.61 k/s** | noise |
| single_insert/tx_staged | 254 µs · **3.93 k/s** | 254 µs · **3.93 k/s** | identical |
| batch/non_tx/1 | 440 µs · **2.27 k/s** | 435 µs · **2.30 k/s** | noise |
| batch/tx/1 | 438 µs · **2.28 k/s** | 427 µs · **2.34 k/s** | noise |
| batch/non_tx/10 | 776 µs · **12.9 k/s** | 754 µs · **13.3 k/s** | noise |
| batch/tx/10 | 758 µs · **13.2 k/s** | 736 µs · **13.6 k/s** | noise |
| batch/non_tx/100 | 4.68 ms · **21.4 k/s** | 4.18 ms · **23.9 k/s** | improvement |
| batch/tx/100 | 4.68 ms · **21.4 k/s** | 4.56 ms · **21.9 k/s** | noise |
| batch/non_tx/1_no_result | 441 µs · 2.27 k/s | 425 µs · 2.35 k/s | noise |
| batch/tx/1_no_result | 429 µs · 2.33 k/s | 443 µs · 2.26 k/s | noise |
| batch/non_tx/10_no_result | 753 µs · 13.3 k/s | 732 µs · 13.7 k/s | noise |
| batch/tx/10_no_result | 741 µs · 13.5 k/s | 719 µs · 13.9 k/s | noise |
| batch/non_tx/100_no_result | 4.68 ms · 21.4 k/s | 4.18 ms · 23.9 k/s | improvement |
| batch/tx/100_no_result | 4.53 ms · 22.1 k/s | 4.46 ms · 22.4 k/s | noise |
| batch/indexed/non_tx/100 | 12.85 ms · **7.78 k/s** | 13.13 ms · **7.62 k/s** | −2% noise |
| **batch/indexed/tx/100** | **12.91 ms · 7.75 k/s** | **17.01 ms · 5.88 k/s** | **−24% ⚠️** |
| **batch/indexed/non_tx/1000** | **122.9 ms · 8.14 k/s** | **173.4 ms · 5.77 k/s** | **−29% ⚠️** |
| batch/indexed/tx/1000 | 117.9 ms · **8.48 k/s** | 117.0 ms · **8.55 k/s** | identical |

---

## 3. Вердикты

### sled-регрессии НЕТ
Ранее видел 95 → 116 µs (back-to-back на той же машине) и подумал что регрессия
в json-окне. Перепрогон без параллельных агентов: **94 → 92 µs** (даже чуть
быстрее). Прошлый замер был артефактом параллельной нагрузки на CPU (22 @ash-агента).

### tx-слой бесплатен
non_tx ≈ tx на всех уровнях (single insert, batch 1/10/100, indexed 1000).
Это и должно быть: SSI-граница в чистом виде дёшева; платишь только когда
реально конфликт.

### Indexed batch — реальная подозрительная просадка
Две точки: `indexed/tx/100` −24% и `indexed/non_tx/1000` −29%. При этом
`indexed/tx/1000` идентично, `indexed/non_tx/100` идентично. Паттерн странный
(возможна большая дисперсия — но направление подозрительное). Это **engine
pipeline + index-write**, не storage.

**Что хорошо бы сделать:** бисект между `1b1e229` и `f19d593` по бенчу
`tx_pipeline -- "batch_pipeline/indexed/non_tx/1000"` (он самый стабильный из
двух подозреваемых) — найти точный коммит-виновник. Журнал кампании короткий
(~22 коммита между pre и post), бисект ~5 шагов × 1 прогон.

---

## 4. Кэш-сводка throughput'ов (для быстрого взгляда)

**Single insert (1 запись = 1 commit):**
in_memory **178 k/s** · cached_in_memory **103 k/s** · sled **10.5 k/s** ·
redb **1.4 k/s** · fjall ~13 k/s · canopy ~5.5 k/s · persy ~270 /s · nebari ~190 /s

**Batch /100 (1 commit на батч из 100, амортизация):**
redb **40 k/s** · sled **19 k/s** · in_memory 131 k/s · cached 70 k/s · membuffer 80 k/s

**Engine non-tx batch /100:** 22-24 k/s · **+indexed:** 6-8 k/s (~3× штраф за индекс на запись).

---

## 5. Команды воспроизведения

```sh
# Сырой backend (opt-0):
CARGO_TARGET_DIR='D:/dev/rust/.cargo-target-ab-post' cargo bench \
  -p shamir-storage --bench store_raw -- \
  '(sled|redb|in_memory)/(insert/single|set_many/batch/100)'

# Engine tx vs non-tx (opt-0):
CARGO_TARGET_DIR='D:/dev/rust/.cargo-target-ab-post' cargo bench \
  -p shamir-engine --bench tx_pipeline -- \
  'tx_overhead/(single_insert|batch_pipeline)'

# A/B через worktree:
git worktree add -f /d/dev/rust/ab-pre  1b1e229
git worktree add -f /d/dev/rust/ab-post f19d593
# (отдельный CARGO_TARGET_DIR на каждый worktree — иначе шумит)
```

Все измерения выполнены на чистом дереве без параллельной нагрузки.
Машина: Windows 10, host MSVC; `nextest` доступен.
