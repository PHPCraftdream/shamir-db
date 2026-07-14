בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-29 [perf-campaign-2-close]

## Session summary

Перф-кампания ② на write hot-path: серия ArcSwap-замен read-mostly
DashMap-реестров + закрытие membuffer-кандидатов через профиль.
Главные победы:

- **#292** — `IndexInfo` (regular + unique): DashMap → ArcSwap<Vec<...>>
  с `rcu()` COW. Differential bench tx_pipeline indexed/tx/1000:
  **−38.84% / 1.63×** (132.56 → 81.07 ms, p=0.00, диапазоны не пересекаются).
- **#304** — `SortedIndexManager::indexes`: тот же приём.
  Differential на tx_pipeline = null (sorted-iter в этом бенче амортизирован
  через `plan_records_created_batch`-snapshot-раз-на-батч). На
  tx_concurrent disjoint_inserts n_8: **−13.09% / +15% throughput**
  (1.379 → 1.199 ms, p=0.00) — именно multi-thread guard, который
  пользователь явно потребовал не регрессировать.
- **#303** — Windows file-lock race fix в `WalSegment::replay`:
  TOCTOU между snapshot'ом `SegmentSet::replay` и unlink'ом в
  `truncate_below` (delete-pending → ACCESS_DENIED). Симметричная
  toleranza к `NotFound` теперь покрывает и `PermissionDenied` (Windows).
  Verification: 10/10 PASS @engine.

Промежуточный test fix (`3f9bcbb6`): помечен `#[serial]` `wal_segment_count_*`
(env-var race) + три `ssi_stress_tests::*` (multi_thread runtime
starvation под nextest'ом).

**Закрыты как false candidates:**

- **#291** — membuffer `dirty.iter().take()` — закрыт по статическому
  доказательству (`tx_pipeline` использует `InMemoryRepo` напрямую,
  БЕЗ MemBuffer-обёртки → `drain_once` физически не вызывается в
  профиле, на котором ставился кандидат).
- **#305** — membuffer FIFO-замена — закрыт по практическому perf-профилю
  `membuffer_pump/frequent_flush` (10ms flush_interval, in-memory inner =
  максимально благоприятный сценарий): `drain_once` = **0.07% self-time**,
  Σ всех DashMap-операций ~0.5%. Для сравнения, #292 был 3.11% на
  tx_pipeline → membuffer-drain **в ~30 раз меньше**, в шуме измерения.
  Реальный hot path membuffer'а — moka cache + Arc/Bytes hash + memmove.
  Дешёвый is_empty-fast-path небезопасен (race с in-flight insert через
  shard-lock серилизацию dirty.is_empty), constraint multi-thread
  исключает.

SVG-флэймграф membuffer'а сохранён в `.flamegraphs/membuffer-pump-frequent-flush-2026-06-29.svg`.
Research-док `WRITE-HOT-PATH-PROFILE-2026-06-28.md` обновлён §10 (#291
closed), §11 (анализ #304/#305 до выполнения), §12 (#305 closed by profile).
§10-§11 закоммичены (1c382fcf); §12 + SVG **uncommitted** — пользователь
сообщил о шумной машине (параллельные бенчи в другом проекте), отложили
коммит для возможного перезамера/доверия.

В чём шум опасен: differential-бенчи (#304 commit) устойчивы — оба замера
ловили один и тот же шумовой пол, p=0.00. Профиль (§12) тоже устойчив
по природе perf-семплинга (attached к нашему PID, чужие самплы не
попадают; ratios внутри binary не страдают от внешнего CPU-contention).
Абсолютные числа (insert_single = 23.55 µs) могли быть инфлированы, но
это не влияет на выводы.

## Active goal
none (никаких Stop-hook условий не активировано)

## TaskList

### in_progress
(пусто)

### pending
- #287 Исследовать NUMA-aware реализацию работы на нескольких процессорах

### recently completed (deleted после закрытия)
- #292/#303/#304 (landed: код)
- #291/#305 (closed as false candidates: документация)
- ранее: #288/#289/#290/#295/#296/#297/#298-#301/#302 — captrack-кампания
  (часть landed, часть reverted в Партии 1)

Удалённые таски: 5 в этой сессии (#291/#292/#303/#304/#305) + ~13 в
прошлых сеансах (captrack-кампания).

## Decisions

- **#304 идти первым перед #305** — известный паттерн (#292), низкий
  риск, понятный multi-thread эффект. Reject: начать с #305 (FIFO дубликат-
  проблема + не подтверждено профилем).
- **#305 закрыть, не оптимизировать** — после практического профиля.
  Reject: продолжать к FIFO-перепроектированию (drain 0.07% не оправдывает
  риска двойного-bookkeeping).
- **#303 правильное место фикса — `WalSegment::replay` в shamir-wal**,
  не SegmentSet. Reject: лочиться через расширение mutex'а вокруг
  truncate (heavier, сериализует replay/truncate).
- **`#[serial]` group для multi_thread stress + env-var тестов** —
  правильный безопасный путь, не «увеличить timeout» (CLAUDE.md прямо
  запрещает). Reject: оставить «как Windows-флак».
- **§12 + SVG не коммитить пока — машина шумная** — qualitatив устойчиво
  (ratios внутри PID), но осторожность ради честности. Reject: коммитить
  всё прямо сейчас.

## Open questions

- **Коммитить ли §12 + SVG** — отложено до перезамера на чистой машине.
  Если будем оставлять — нужен явный disclaimer о шуме в §12.
- **CLAUDE.md §3 — добавить ArcSwap-конвенцию?** Я в §12.6 предложил
  «ArcSwap<Vec<...>> для read-mostly с редкими mutations» как
  workspace-стандарт (после двух успешных #292+#304). Не выполнено.
- **#287 NUMA — когда?** Большая исследовательская тема, горизонт «через
  одну-две кампании», не сейчас.

## Repo state

```
 M docs/dev-artifacts/research/WRITE-HOT-PATH-PROFILE-2026-06-28.md
?? .flamegraphs/membuffer-pump-frequent-flush-2026-06-29.svg
```

```
1c382fcf docs(research): §10 #291 closed + §11 анализ #304/#305
acf992cb perf(index): #304 SortedIndexManager DashMap → ArcSwap<Vec<SortedIndexDefinition>>
93e03ffc fix(wal): #303 Windows TOCTOU between SegmentSet::replay snapshot и truncate_below
3f9bcbb6 test(engine): flake mitigation — serialize env-var + multi_thread stress
7bb5d392 perf(index): #292 IndexInfo DashMap → ArcSwap<Vec<IndexDefinition>>
```

master в синке с origin (0 коммитов ahead). Working tree чистый кроме
§12-приложения к research-доку и нового SVG.
