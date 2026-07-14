בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Obvyazka research — master synthesis + поэтапный план

Дата: 2026-06-21. Консолидация трёх раундов исследования обвязки (+ ревью
каждого) в одну карту с новым центром масс и поэтапным планом.

Парные документы:
- `obvyazka-research-review-2026-06-19.md` — review раунда 1
- `obvyazka-research-round2-review-2026-06-19.md` — review раунда 2
- (round 3 review встроен в §3 ниже)

---

## 1. Созерцание: куда сместился центр масс

Кампания началась с погони за **throughput записи**. Свапнули backend,
починили L12/L13/L6, написали drain-бенч. И backend-matrix сказал тихую,
но решающую вещь:

> **in_memory (190K) = fjall (188K)**

Write-path упёрся в обвязку, не в хранилище. Он **уже почти оптимален.**

### Три раунда копания подтверждали ровно это

Каждый раунд write-path находок оказывался либо:
- **Микро** (наносекунды markers, started_at_ns).
- **Контракт-заблокированным** (G запрещён L10a, H ломает WAL-формат,
  C упирается в задокументированную atomicity-стену).
- **Неизмеренным предположением** (B sharding — атрибуция plateau не
  доказана).

**Write-path выдоен.** В нём не осталось 10× без фундаментального
переписывания, а фундаментальное переписывание стоит на непроверенной
гипотезе.

### Раунд 3 наткнулся на настоящие обрывы — на READ-path

- **S1**: `AsOf(ts)` = O(total entries). На 10M это не «медленно»,
  это **не работает**.
- **S2**: `ORDER BY ... LIMIT 10` без covering index = полная
  материализация 10M строк в RAM + полная сортировка. **OOM-риск**,
  не просто латентность.
- **S3**: `WHERE x>=10 AND name="..."` = full scan несмотря на
  sorted-индекс.

### Красота арки

Система сама сказала, где смотреть — в своих же комментариях. S1
дословно: *"a later performance slice"*. Write-path кампания закалила;
read-path остался с FIXME-долгами, которые **подошли по сроку**.

Мы три раунда искали, где терять µs на записи, а **порядки величин
всё это время лежали на чтении** — user-facing, асимптотические, и
(что важно) **read-only → низкий риск durability**.

### Методологический кристалл (выкован трижды)

> **Наличие траты найти легко — величину и безопасность переоцениваешь
> всегда.**

Даже сильнейший раунд 3 дрейфовал оптимистично:
- S2 «6× faster» — это **память O(N)→O(K)**, не время (decode/filter
  ещё O(N)).
- S4 «unbounded» — **условно** (bounded by `min_alive`; патология
  требует долгоживущего снапшота).
- S5 «единственный path всё ещё на commit_lock» — AsyncIndex это
  **OPT-IN минор** (`tx_context.rs:27`), дефолт lockfree.

Поэтому дисциплина плана не обсуждается: **каждая фаза открывается
замером (доказать обрыв) и закрывается замером (доказать выравнивание),
с верификацией контракта между ними.** Это `/opti`-петля — единственное,
что пережило контакт с реальностью за всю кампанию.

---

## 2. Карта найденного (3 раунда × verification)

### Write-path (раунды 1–2) — почти всё в парк/отвергнуто

| Рычаг | Раунд | Что | Решение | Причина |
|---|---|---|---|---|
| A | 1 | markers в фон | **парк** | µs на уже-быстром пути; нужна recovery-верификация; низкий ROI |
| B | 1 | sharded WAL/drainer | **парк** | атрибуция plateau **не измерена** — нельзя строить большое на гипотезе |
| C | 1 | dedicated writer-thread | **отвергнут** | задокументированный откат + не уходит от atomicity-стены без смены durability-контракта |
| H | 2 | удалить `started_at_ns` | **отвергнут** в этой форме | WAL format break (persistимое bincode-поле); safe-форма = ns |
| G | 2 | skip projection при 0 subs | **отвергнут** | запрещён L10a (журналу нужен event) |
| N2 | 2 | Bytes-clones дорогие | **dismissed** | atomic refcount-bump, не memcpy — наш 5-й столп идеологии |
| K | 2 | bench fix `insert_tx_many` | **fold-in** | методологически верно, но влияние single-digit %; сделаем в Фазе 0 |

### Read-path (раунд 3) — настоящее мясо

| # | Что | Verified | Решение |
|---|---|---|---|
| **S1** | AsOf-by-ts = O(total entries) | ✅ дословно (`mvcc_history.rs:90`) | **берём** (Фаза 3) |
| **S2** | ORDER BY+LIMIT full materialize+sort | ✅ структура (`read_exec.rs:663`, `read_planner.rs:352`) | **берём** (Фаза 1) — headline = память |
| **S3** | range-in-AND игнорирует sorted index | ✅ planner gap | **берём** (Фаза 2) |
| S4 | `predicate_conflicts` O(W) под долгим снапшотом | ⚠️ conditional | **парк** до workload-сигнала |
| S5 | AsyncIndex на commit_lock + dead group_commit | 🔴 переоценён | **отложен** |
| S6 | uwl over-broad | plausible | **парк** |
| J | dual index systems (legacy + index2) | ✅ оба live | **парк** — большой refactor, ортогонален обрывам |

### Несверены / низкого приоритета (раунд 3 §2)

- S7 vacuum под snapshots, S8 distinct double-alloc, S9 filter_stream
  collect, S10 ORDER BY 4× mat, S11 GC full-scan, S12 backfill decode,
  S13 dead code cleanup. **Парк.**

---

## 3. Что берём в работу

**Берём — read-path обрывы (S1, S2, S3).** Порядки величин, user-facing,
ортогональны durability, низкий риск. **Это новый центр масс кампании.**

**Оставляем write-path в парке** до появления конкретного workload-сигнала
(write-throughput SLA на N=128 → B; Serializable analytic workload → S4/S6).

---

## 4. Поэтапный план

Каждая фаза = один `/opti`-цикл: **bench → opt → tests → bench →
verify → commit.** Никаких WIP commit'ов в master без замера-доказательства.

### Фаза 0 — Read-path measurement harness *(гейт всего)*

Прежде любой read-оптимизации — бенч, который **превращает асимптотику
в кривые**. Новый `read_path_matrix`:

- **N**: 10K / 100K / 1M (+ 10M для cliff-кейсов S1/S2 на стабильной железке).
- **Формы запросов:**
  - `ORDER BY y LIMIT 10` (no index, no WHERE) — fast-path baseline.
  - `ORDER BY y LIMIT 10` без индекса с WHERE → S2 trigger.
  - `WHERE x>5 ORDER BY y LIMIT 10` без covering — S2 + S3 combo.
  - `WHERE x>=10 AND name="foo"` с sorted index на x — S3 trigger.
  - `AsOf(Timestamp(t))` point-read — S1 trigger.
- **Метрики:** **время И peak memory** (для S2 память — настоящий
  headline, не время).

**Fold-in (раунд 2 K):** заодно fix `backend_matrix` на
`insert_tx_many` — раз уж трогаем bench infra, единый sweep.

**Done:** кривые N→{time, mem}, доказывающие обрыв на каждом из S1/S2/S3
эмпирически. Это «measure first» применённый к read-path. Гейт всех
последующих фаз.

### Фаза 1 — S2: bounded ORDER BY + LIMIT *(калибровка + реальный memory-win)*

Самый well-scoped. Два под-случая:

- **Без covering index**: top-K bounded heap (`BinaryHeap` capa K) —
  O(K) память, O(N log K) время. Заменяет full-materialize+sort в
  `read_collecting` / `apply_order_by_qv`.

- **Sorted index есть, но есть WHERE**: index-ordered scan + residual
  filter + ранний выход на K-м совпадении — O(K + scanned), **строго
  лучше heap** для этого случая.

**Риск:** низкий (read-only).
**Headline:** память O(N)→O(K) (на 10M = «не упасть», не «6× быстрее»).
Time-win < 6× потому что decode/filter всё ещё O(N), это узнаём из
Фазы 0.
**Гейт:** Фаза-0 кривая памяти выравнивается; результат **байт-идентичен**
full-sort (тот же порядок, та же страница).

### Фаза 2 — S3: range-extraction из AND → sorted-index scan

Planner-работа: извлечь range-предикаты из `And`, прогнать range через
sorted-индекс, остаток — residual filter.
`WHERE x>=10 AND name="foo"` из full-scan → index-range-scan.

**Риск:** средний (planner correctness — **ни одной строки не потерять**).
**Гейт:** Фаза-0 кривая + correctness-тест (result-set идентичен
full-scan на рандомизированных предикатах).

### Фаза 3 — S1: ts-ordered index для AsOf *(проект, не фикс)*

Новая **persistent структура**: sorted-индекс `[ts][version]` → version,
поддерживается на commit (ts уже пишется — добавить sorted-entry).
`version_at_or_before_ts` → O(log N) range-lookup вместо O(total).

**Риск:** средний (новая структура — crash-safe + rebuildable из
history-scan).
**Эффект:** крупнейший — AsOf из «не работает» в «работает».
**Гейт:** Фаза-0 AsOf-кривая + recovery-тест (индекс восстановим из
history scan на open).

### Фаза 4 — *(условно)* S4 + S6: Serializable hygiene

Только если **Serializable write-heavy — целевой workload**:
- **S4:** size-triggered prune `commit_write_log` (в дополнение к
  min_alive GC) — bound на O(W) predicate-conflicts под долгоживущим
  снапшотом.
- **S6:** сузить uwl-scope до posting-write окна.

**Риск:** средний (SSI correctness).
**Решается приоритетом workload'а**, не сейчас по умолчанию.

---

## 5. Порядок и его логика

```
Фаза 0  →  S2  →  S3  →  S1  →  (S4/S6)
(measure)  (low)   (med) (proj)  (conditional)
```

**Восходяще по effort/risk, нисходяще по well-scoped'ности** — та же
/opti-прогрессия, что задумывалась для write-path: начать с чистого
изолированного фикса (S2) для калибровки методологии на новой оси,
потом planner (S3), потом проект (S1).

**Фаза 0 может переупорядочить S1/S2/S3** — если кривая покажет, что
AsOf-обрыв острее в нашей реальной нагрузке, S1 поднимается.

---

## 6. Первый ход

**Фаза 0 — read-path bench.**

Без неё мы повторим раунд-1/2: оптимизировать по оценке. Бенч превратит
«S1/S2/S3 — обрывы» из утверждения в кривую, и каждая последующая фаза
будет иметь честный до/после.

**Это дёшево, read-only, нулевой риск — и это ровно та дисциплина,
которую три раунда выковывали.**

---

## 7. Открытые follow-ups (парк)

| Когда триггерится | Что разморозить |
|---|---|
| Write-throughput SLA не покрыт fjall@w=32 | Фаза 0 для write (markers toggle, WAL isolation, plateau attribution) → A или B по результату |
| Serializable analytic workload жалуется | S4 + S6 |
| Indexed write workload в фокусе | I (batch unique validate), J (unify index) |
| Stable перед v1.0 | H safe-форма + S13 cleanup (micro-batch) |
| AsyncIndex (vector/HNSW) heavy load | S5 lockfree route |

---

## 8. Что НЕ повторяем

Из методологического кристалла:
- **Не коммитим WIP без замера до/после.** Каждая фаза = bench-доказанный
  результат или revert.
- **Не верим «дёшево и безопасно»** без verification контракта (раунды
  2/3 показали: это сигнал проверить, а не сигнал к коммиту).
- **Не строим большое на гипотезе об атрибуции** (B — был бы повтор
  L12/L13: оптимизировать по оценке, не по числу).
- **Не оптимизируем уже-оптимальное** (write-path: 188K = in_memory
  ceiling; точить его — низкий ROI vs read-path обрывы).
