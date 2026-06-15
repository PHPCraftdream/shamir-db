# D2 — Execution Plan: versioned overlay (visibility ≫ durability decoupling)

Снимает stop'ы из `durability-model.md` (D2: materialize вне ack-пути) и
`version-oracle-design.md` (contiguous-prefix publish). Опирается на два
аудита: `d2-r1-readpath-audit.md` (GO, 0 live bypass) и
`d2-r2-abortpath-census.md` (1 BLOCKER + RAII-фикс).

---

## 0. Идея

Сегодня видимость прибита к диску в одном месте: чтобы вернуть значение,
оно обязано лежать в durable `history` version-log (`resolve_read` →
`history.get`). Materialize (`apply_committed_ops`: `history.transact`
**затем** `publish_cell`) выполняется inline на ack-пути.

D2 вставляет **versioned in-memory overlay** между ячейкой и durable-логом:
- **ack-путь** (мгновенно, lock-free): `overlay.insert((table,key,ver)→value)`
  + `publish_cell` + `completion.mark(ver, Materialized)`.
- **чтение**: overlay-probe ПЕРЕД durable backing.
- **фон** (дренаж-лидер, вне ack): `history.transact` батчами + `fsync` +
  `durable_watermark`.

Значение уже в памяти на момент коммита (payload WAL-entry,
`wal_ops_from_tx` → `Put{rid,body}`). Overlay — не новый буфер, а
удержание этого payload, индексированного по `(key,version)`, пока
durable-лог не догонит. RYOW выпадает по построению: писатель читает через
тот же seam, его версия в `cells`, значение в overlay.

**Два watermark.** `visibility_watermark` (= сегодняшний `last_committed`,
двигается на ack) гейтит snapshot'ы читателей; `durable_watermark` (новый,
двигается в фоне) гейтит overlay-GC и WAL-truncation. Инвариант
`durable_watermark ≤ visibility_watermark`; зазор = содержимое overlay.

Дихотомия «RYOW vs full async» ложна: overlay даёт оба — видимость
синхронна, физический I/O асинхронен. «Eventually» остаётся только у
невидимого, защищённого WAL durable-хвоста.

---

## 1. Инварианты (STOP при сомнении)

1. **WAL — единственный источник истины.** Overlay/cells/history/индексы —
   производный кэш. Крах → overlay потерян → recovery реплеит WAL в history.
2. **Три seam'а overlay-aware** (R1): `resolve_read`, `get_current`,
   `current_stream`. Любой обходной читатель committed-данных запрещён.
3. **Каждая аллоцированная версия терминально помечается** (R2): по
   построению через RAII `VersionGuard` (Drop→Aborted, commit()→Materialized).
   Пропуск → `visibility_watermark` виснет навсегда.
4. **Один watermark-механизм** (R2 H5): и tx, и non-tx идут через
   `CompletionTracker`. Прямой `publish_committed_max` на write-пути убран.
5. **`cells[key].version` публикуется на ACK**, не на дренаже (R1 §2): иначе
   covering index-only freshness gate не свалится в overlay-aware fallback.
6. **Truncation только после durable-materialize** (`durable_watermark`).
7. **Тесты только через `./scripts/test.sh`**; gate на каждом шаге; без
   commit/push/агентов без явной просьбы.

---

## 2. Фазы

Две группы. **P0** — пред-условия безопасности; КАЖДЫЙ шаг семантически
NO-OP под текущим inline-materialize (видимость не меняется), только
унифицирует watermark-механику и делает аллокацию версий leak-proof. P0
должен лечь и позеленеть ПОЛНОСТЬЮ до P1. **P1** — собственно расцепление.

### P0 — watermark unification + leak-proofing (safe under inline materialize)

#### P0a — RAII `VersionGuard` из `assign_next_version`
- Новый тип `VersionGuard { version: u64, tracker: Arc<CompletionTracker>, armed: bool }`
  (в `shamir-tx`, рядом с `completion_tracker.rs` / `repo_tx_gate.rs`).
  `Drop`: если `armed` → `tracker.mark(version, Aborted)`. `commit(self)`:
  `tracker.mark(version, Materialized)` + `sync_last_committed_from_watermark`,
  снимает `armed`. `Drop` синхронный (mark — атомики/scc, не async) ✓.
- `assign_next_version` возвращает `VersionGuard` (или добавить
  `assign_next_version_guarded`, старый оставить для recovery-seed).
- Мигрировать tx-сайты B/C (`pre_commit.rs:198,286`): убрать явные
  `mark(Aborted)` на SSI/phantom/empty-tx/WAL-fail — теперь ранний return
  роняет guard → Aborted. Успех: `materialize`/legacy_async вызывают
  `guard.commit()` вместо `mark(Materialized)`.
- Закрывает H1, H2, H3, H4 по построению.
- Gate: `@oracle` (SSI/phantom/recovery/concurrent — все зелёные).
- Риск: ВЫСОКИЙ (меняет abort-механику). Под-шаги: тип+тесты → миграция B →
  миграция C → удаление старых явных mark.

#### P0b — non-tx унификация на CompletionTracker (H5 BLOCKER)
- Сайты D/E/F (`mvcc_store/mod.rs:247,303,349`): после durable history-write
  взять `VersionGuard` из P0a и `guard.commit()` ВМЕСТО прямого
  `publish_committed_max(new_v)`. Теперь non-tx двигает watermark через
  tracker, как tx.
- Тонкость batch (`set_versioned_many`): N версий — N guard'ов, commit
  каждого после общего `transact`; либо групповой `mark_many`. Решить в
  под-шаге, по-умолчанию N guard'ов (просто, lock-free).
- Gate: `@oracle @engine` (non-tx CRUD + watermark-монотонность).
- Риск: СРЕДНИЙ.

#### P0c — отложить assign до точки перед WAL begin (R2 Option B)
- Перенести `gate.assign_next_version()` из начала `pre_commit_locked*` к
  точке прямо перед сборкой WAL-entry (`pre_commit.rs:252` / `:369`).
  SSI/phantom/empty-tx abort'ы перестают жечь версию; окно assign→mark
  сжимается до «assign → WAL begin → enqueue» без `.await` до WAL.
- Чисто механический перенос; validate-фазы `commit_version` не используют.
- Gate: `@oracle`.
- Риск: НИЗКИЙ.

### P1 — versioned overlay (the decoupling)

#### P1a — `VersionedOverlay` scaffold (additive, не подключён)
- `scc::HashMap<(u64 table_token, Bytes key, u64 version), Bytes value, THasher>`
  (tombstone = `Bytes::new()`, как в history). Методы: `insert`, `get(key,ver)`,
  `range_for_key(key, ≤snapshot)` (для fallback), `drain_prefix(≤durable_wm)`,
  `len`/`bytes` (для backpressure). Lock-free.
- Юнит-тесты: insert/get, tombstone, prefix-drain, конкурентные insert'ы.
- Gate: `@oracle`. Риск: НИЗКИЙ.

#### P1b — три seam'а overlay-aware (overlay ещё пуст → поведение идентично)
- `resolve_read` (mod.rs:215): перед `history.get(version_key)` пробовать
  `overlay.get(key, cur_v)`; range-fallback — overlay-range ∪ history-range,
  newest ≤ snapshot.
- `get_current` (mod.rs:398): тот же probe перед `history.get`.
- `current_stream` (mod.rs:435): merge overlay-entries в поток (overlay
  держит новейшие версии; group-by по key берёт max version ≤ floor из
  overlay ∪ history). Самая нетривиальная часть — merge-join; overlay мал
  (ограничен окном), грузим в map и мёржим.
- Overlay не наполняется → все чтения идентичны. Доказывает отсутствие
  регресса от probe.
- Gate: `@oracle @engine` (CRUD/scan/index/temporal).
- Риск: СРЕДНИЙ (current_stream merge).

#### P1c — наполнять overlay на ACK + dual-write history inline
- На ack-пути коммита (в `apply_committed_ops` / commit-path): `overlay.insert`
  для каждого (key, commit_version, value) ПЛЮС оставить `history.transact`
  inline (dual-write, overlay избыточен). Читатели предпочитают overlay.
- Доказывает: overlay отдаёт байт-в-байт то же, что history, под реальной
  нагрузкой. publish_cell остаётся на ack (уже так).
- Gate: `@oracle @engine @e2e`.
- Риск: СРЕДНИЙ.

#### P1d — дренаж-лидер: history.transact OFF ack-path + `durable_watermark`
- `history.transact` уезжает с ack в фоновый дренаж-лидер (CAS-leader +
  `watch`/`Notify`, как в WAL group-commit). Дренирует contiguous-префикс
  Materialized-версий батчами: `history.transact(batch)` + `record_ts`,
  затем `durable_watermark.advance()`.
- Overlay становится ЕДИНСТВЕННЫМ источником значения для версий в
  `(durable_wm, visibility_wm]`. ЭТО расцепление видимости и долговечности.
- Crash-seam'ы (D4): (e) WAL durable, value ещё в overlay, не в history →
  recovery реплеит из WAL. Overlay потерян на крахе — законно.
- Gate: `@oracle @engine @e2e` + crash. Бенч D0b: regress закрыт.
- Риск: ВЫСОКИЙ.

#### P1e — overlay GC + F6 truncation + backpressure
- GC: `overlay.drain_prefix` версий `≤ durable_watermark` И
  `< min_active_snapshot` (живой читатель держит снэпшот ниже — нельзя
  выкинуть). `min_active_snapshot` — маленький `ArcSwap`/atomic-min трекер
  активных снэпшотов.
- F6 (поглощает task #2): WAL-truncation сегмента после `durable_watermark`
  проедет за entry (значение durable в history). Гейт A5-interner-hwm уже
  есть (`materialize.rs:354`).
- Backpressure: мягкий порог на `(visibility_wm − durable_wm)` или
  overlay-байты; превышение → новый коммит делает async-yield на `Notify`
  дренаж-лидера (не лок — адаптивный темп под bandwidth). `log()` дропы.
- Gate: crash + growth-limit (overlay/WAL не растут unbounded под нагрузкой).
- Риск: ВЫСОКИЙ.

---

## 3. Секвенсинг и зависимости

```
P0a (VersionGuard) ─→ P0b (non-tx unify) ─→ P0c (defer assign)
        │                                          │
        └────────────── all green ────────────────┘
                              ▼
P1a (overlay) ─→ P1b (3 seams) ─→ P1c (ack populate + dual-write)
                              ▼
P1d (drain leader + durable_wm)  ← D4 crash seams thread here
                              ▼
P1e (GC + F6 truncation + backpressure)  ← absorbs task #2 (F6)
                              ▼
        task #3 (D4) — финальное расширение crash-injection
        task #4 (CAPSTONE) — measure-first, после стабилизации
```

P0 целиком зелёный перед P1. P1d — точка, где видимость реально обгоняет
диск; до неё (P1c) — dual-write, поведение неотличимо. D4 (#3) финализирует
crash-покрытие после P1e; F6 (#2) растворяется в P1e.

---

## 4. Что НЕ входит / границы

- Версионный лог не переписываем — он уже version-indexed.
- `get_at` API не меняем — меняется только источник значения.
- Wire-формат не трогаем.
- `StagingStore::get` (R1 #2) — оставить test/bench-only, не заводить в
  read-path.
- Индексы/HNSW: первый разрез — overlay только для данных; covering
  index-only валится в overlay-aware `get_current` (R1). Index-постинги в
  overlay — возможное расширение, вне первого слайса.

---

## 5. Риск-резюме

| Фаза | Риск | Почему |
|---|---|---|
| P0a | высокий | меняет abort-механику (RAII guard на всех сайтах) |
| P0b | средний | non-tx watermark unification |
| P0c | низкий | механический перенос assign |
| P1a | низкий | additive scaffold |
| P1b | средний | current_stream merge-join |
| P1c | средний | dual-write, проверка байт-паритета |
| P1d | высокий | реальное расцепление, crash-контракт |
| P1e | высокий | GC vs живые снэпшоты, truncation, backpressure |

Data integrity > performance. На любой неопределённости в P0a/P1d/P1e —
STOP с цифрами, как со всеми структурными стопами кампании.
