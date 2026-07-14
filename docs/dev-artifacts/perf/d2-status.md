# D2 — Status & Handoff

Живой статус кампании D2 («materialize вне ack-пути»). Обновляется по ходу.
Базовая ветка `master`. Pre-D2 baseline = `5c6eaf9` (F5e, единый WAL-хребет).

---

## 0. Конечная цель

**Расцепить видимость и долговечность в commit-пути.** Сегодняшняя боль (регресс
D0a): на коммите дорогая физическая запись данных в durable `history`
version-log (`history.transact`) выполняется **inline под `uwl`** (unique-write-
lock), поэтому same-table коммиттеры сериализуются на всю длину materialize.

Цель — превратить систему в **полноценный ARIES** (она была на 90% им):
- **ack-путь** коммитит только в **WAL** (durable, источник истины) и в
  **in-memory overlay** (dirty-page cache для мгновенного RYOW) — без диска,
  без локов.
- **долговечность** (`history`) materialized **лениво в фоне** одним владельцем
  (drainer = `recover_inflight_v2`, прокрученный в цикле).
- видимость синхронна (overlay), физический I/O асинхронен. Дихотомия «RYOW vs
  throughput» ложна — overlay даёт оба.

Контракт уровня-2 (ack после WAL `write()`) **сохранён** — расцепляется только
производный `history`, не WAL.

После D2 — закрыть остаток дорожной карты WAL: **F6** (реальная truncation),
**D4** (полнота crash-тестов), **CAPSTONE** (WAL полностью lock-free).

---

## 1. Что имеем сейчас (архитектура после cutover)

Коммит (tx-путь), пост-cutover:
```
Phase 4: wal.begin_grouped(entry)              [DURABLE — точка ack, не менялась]
ack:  apply_committed_visible (overlay.insert + publish_cell)   [видимо, in-RAM]
      counter(5b) + index(5c) inline (info_store)
      guard.commit()  → visibility_watermark (last_committed)   [читатель видит]
      drainer.wake()
[фон, single-owner Drainer]:
      replay_v2_entry(entry) → history.transact + record_ts(commit-time)
      gate.mark_durable(V)   → durable_watermark
      A5-gate → wal.commit(txn_id)   [truncation — пока NO-OP до F6]
      overlay.gc_upto(durable_watermark)   [P1e — в работе]
```

**Два watermark** (оба — contiguous-prefix `CompletionTracker`, lock-free):
- `visibility_watermark` (= `last_committed`) — двигается на ack. Гейтит снапшоты.
- `durable_watermark` — двигается дренажом. Гейтит overlay-GC и (будущую)
  WAL-truncation. Инвариант `durable ≤ visibility` по построению.
- Зазор `(durable_wm, visibility_wm]` = содержимое overlay = inflight-хвост WAL =
  множество «грязных страниц». Три описания одного множества.

**Три представления committed-записи:**
| Слой | Роль | Долговечность |
|---|---|---|
| WAL-entry (`__tx__` file WAL) | источник истины, очередь дренажа, источник recovery | durable (ур.2 на ack) |
| overlay (`VersionedOverlay`, per-table, scc::TreeIndex) | RYOW read-cache + ничего | RAM (теряется на крахе — законно) |
| `history` version-log (per-table) | лениво-materialized «страницы» | durable после дренажа |

**Читатель** (`resolve_read`/`get_current`/`current_stream`) пробует overlay →
durable `history`. RYOW: писатель читает через тот же seam, его версия в `cells`,
значение в overlay — мгновенно, без локов.

**Crash-контракт (нерушимый):** `wal.commit(txn_id)` (снятие inflight-маркера /
право на truncation) НИКОГДА не раньше, чем данные версии durable в `history`.
Контрапозиция: *маркер снят ⟹ данные в history*; значит *данных нет в history ⟹
маркер жив ⟹ `recover_inflight_v2` доиграет*. Потерять нельзя. Дренаж и recovery —
**одна функция** (replay → history → mark_durable → truncate); крах — это просто
стационарное состояние «WAL durable, history отстаёт», через которое проходит
каждый коммит.

---

## 2. Что сделали (журнал, 6 коммитов поверх `5c6eaf9`)

**Аудиты (предусловия, обязательные перед версионной видимостью):**
- **R1** `d2-r1-readpath-audit.md` — 0 живых bypass; overlay видим из ТРЁХ seam'ов.
- **R2** `d2-r2-abortpath-census.md` — H5 BLOCKER (non-tx не метил tracker) +
  RAII `VersionGuard` как лекарство.
- План: `d2-execution-plan.md` (+ Addendum), `d2-p1d2-subplan.md` (+ §8 refinement).

| Этап | Что | Коммит | Гейт |
|---|---|---|---|
| **P0a** | RAII `VersionGuard` (Drop→Aborted, commit()→Materialized) — закрывает leak-дыры H1-H4 по построению | `609a3ae` | @oracle |
| **P0b** | non-tx (`set_versioned*`) через тот же `CompletionTracker` — закрывает H5 BLOCKER | `609a3ae` | @oracle @engine |
| **P0c** | assign версии отложен за SSI/phantom/empty-tx (аборты не жгут версию) | `609a3ae` | @oracle |
| **P1a** | `VersionedOverlay` (scc::TreeIndex, lock-free) — scaffold | `609a3ae` | -p shamir-tx |
| **P1b** | три читателя overlay-aware (overlay пуст → байт-идентично) | `609a3ae` | @oracle @engine |
| **P1c** | overlay наполняется на ack + dual-write history inline (байт-паритет) | `609a3ae` | @oracle @engine @e2e |
| **P1d-1** | `durable_watermark` machinery (второй tracker; durable==visibility под inline; ноль изменений поведения) | `223329c` | @oracle @engine |
| **P1d-2a** | `Drainer` = обобщённый `recover_inflight_v2` (additive, не подключён) | `5da89b8` | @oracle @engine |
| **P1d-2b** | **CUTOVER**: history с ack-пути → дренаж; split `apply_committed_ops`; `pending_ts` (commit-time штамп); single-owner drainer (leak-free); overlay.remove гейт; **+ фикс AsyncIndex data-loss мины** | `b705357` | @oracle @engine @e2e |
| **P1d-2c** | crash-seam (e): phase7 → дренаж; child гонит drain_all; phase5a/6 теперь тестируют overlay-not-history | `916d451` | @oracle @engine --full @e2e |

**Три бага/дыры, пойманные ревью (которые гейты под-агентов пропустили):**
1. **AsyncIndex data-loss мина** — `materialize_async_tail` срезал WAL inline,
   а ack-путь больше не писал history → данные только в volatile overlay
   (латентно безвредно лишь потому, что `wal.commit` пока no-op; реальная потеря
   + wedge при F6). Исправлено: дренаж — единственный truncator.
2. **Пропущенный `--full` crash-suite** — агент гонял `@engine` (lib), не гонял
   интеграционные `crash_recovery.rs`; `crash_at_phase7` падал.
3. **Сломанный phase7-seam** — cutover убрал inline-phase7; перенесён в дренаж.

---

## 3. Ключевые инварианты (the contract)

1. **WAL — единственный источник истины.** overlay/cells/history/индексы —
   производный кэш; на крахе overlay теряется, recovery реплеит WAL в history.
2. **Три seam'а overlay-aware** (R1): `resolve_read`, `get_current`,
   `current_stream`. Обходных читателей committed-данных нет.
3. **Каждая версия терминально помечена** (R2) — по построению через
   `VersionGuard` в ОБОИХ трекерах (Drop→Aborted на обоих; commit()→visibility
   Materialized, durable метит вызывающий/дренаж после физической записи).
4. **Один watermark-механизм**: tx и non-tx через `CompletionTracker`.
5. **`cells[key].version` на ACK** (covering index-only freshness fallback).
6. **`durable ≤ visibility` всегда**; truncation/overlay-GC только по
   `durable_watermark`.
7. **`wal.commit` только после durable history** (crash-контракт).
8. Тесты только через `./scripts/test.sh`; gate на каждом шаге; без
   commit/push/агентов без явной просьбы пользователя.

---

## 4. Что в работе

**P1e — overlay GC + backpressure** (агент достраивает; фундамент в дереве,
компилируется):
- **Overlay GC — ГОТОВО** (в рабочем дереве): `MvccStore::gc_overlay_to(durable_wm)`
  дропает overlay version ≤ durable_watermark + чистит `pending_ts`; дренаж зовёт
  по всем таблицам после каждого pass. Ограничивает overlay окном
  `(durable_wm, last_committed]`.
- **Backpressure — достраивается**: инфра готова (`durable_progress: Notify` в
  gate, сигнал в `mark_durable`, tunable `MAX_UNDRAINED_VERSIONS=10_000` с
  гистерезисом /2). Осталось: `apply_backpressure` (async-yield на gap>порог, с
  deadlock-защитой по таймауту) + call-site в `commit_tx` + тесты + гейт.

---

## 5. Что дальше

| # | Задача | Суть | Зависит |
|---|---|---|---|
| **P1e** | overlay GC + backpressure | ограничить рост RAM | — (в работе) |
| **#2 / F6** | реальная WAL-truncation | сейчас `wal.commit` = no-op; сделать ротацию/усечение сегмента после `durable_watermark` (ограничить рост WAL на диске; activates AsyncIndex-фикс) | после P1e |
| **#3 / D4** | crash-injection completeness | расширить (e)-покрытие, non-flaky над 20 прогонами, drain-crash seam, F4/F6-варианты | после F6 |
| **#4 / CAPSTONE** | WAL полностью lock-free | убрать два микро-лока (`WalGroupCommit.pending: tokio::Mutex`, `WalSegment.file: std::Mutex`) → single-writer-task + lock-free MPSC; **measure-first, последним**, после стабилизации WAL | после F6/D4 |

Порядок: **P1e → F6 → D4 → CAPSTONE**. F6 «активирует» AsyncIndex-фикс
(no-op truncation становится реальной). После CAPSTONE — `/opti`-цикл на бенче
D0b (подтвердить закрытие регресса same-table concurrent-commit).

---

## 6. Артефакты (docs/dev-artifacts/perf/)

- `durability-model.md` — ARIES WAL-spine, 3 уровня, D-план (исходный).
- `version-oracle-design.md` / `-execution-plan.md` — lock-free commit, оракул.
- `d2-execution-plan.md` (+Addendum) — P0..P1e, инварианты R1/R2.
- `d2-r1-readpath-audit.md`, `d2-r2-abortpath-census.md` — аудиты.
- `d2-p1d2-subplan.md` (+§8) — cutover-дизайн, drain=recovery.
- `wal-refactor.md` — F0-F6 (F5 закрыт, F6 остался).
- **`d2-status.md`** — этот файл.

---

## 7. Риски / остаточные долги

- **Индекс (Phase 5c) пока inline под uwl.** Первый срез расцепил только DATA.
  Если индекс — со-bottleneck, нужен index-overlay (отдельная волна); измерить
  бенчем D0b после P1e.
- **F6 truncation — no-op.** WAL растёт неограниченно на диске до F6. overlay уже
  ограничивается (P1e), но WAL-сегмент — нет.
- **Drain-poison.** Устойчивая ошибка durable-записи: маркеры остаются inflight,
  overlay+WAL растут; backpressure тормозит (deadlock-защита по таймауту);
  recovery корректен (WAL цел). Нужен circuit-breaker/метрика (часть F6/D4).
- **Lifecycle латентность.** Дренаж-таск держит inner-Arc'и до ≤1 интервала
  (50ms) после дропа последнего foreground-клона; абсорбируется retry-helper'ом
  `open_sled` (Windows file-lock).
- **`StagingStore::get`** (R1 #2) — test/bench-only, не заводить в read-path.
- **temporal/purge** читают history напрямую → дренируют на границе запроса
  (P1d-2b); корректно, но дополнительная латентность для не-`Latest` чтений.
