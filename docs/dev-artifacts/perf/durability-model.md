# Durability Model — WAL как безотказный хребет

## Контракт

- **default** → уровень 2 (OS page cache): ack после `write()` в ОС.
  Переживает крах процесса; теряется **только при потере питания**.
- **Synced (hard)** → уровень 3 (stable storage): ack после `fsync`.
  Переживает и потерю питания.

WAL — единственный безотказный хребет. Всё остальное (data-store,
индексы, HNSW, маркеры) — производный кэш, восстановимый из WAL.

---

## Три уровня долговечности

| Уровень | Что | Крах процесса | Потеря питания |
|---|---|---|---|
| 1. In-process (MemBuffer dirty) | память процесса | ❌ теряется | ❌ |
| 2. OS page cache (после `write()`) | ядро владеет байтами | ✅ выживает | ❌ |
| 3. Stable storage (после `fsync`) | на диске | ✅ | ✅ |

**Текущее состояние (correctness-баг):** WAL `__tx__` info_store
буферизован (`repo_instance.rs:376–379`, flush на строке 748) →
`wal.begin` пишет в in-process MemBuffer → **уровень 1**. Дефолт на один
уровень слабее контракта: крах процесса теряет неслитые WAL-маркеры.
MemBuffer имеет eviction-listener — может вытеснить под давлением
памяти. Это не безотказный буфер, а кэш.

---

## Архитектура: ARIES WAL-spine

```
default write → WAL append через write() (уровень 2) → ACK
             → [фон] батчевый fsync WAL (→ уровень 3)
             → [фон] materialize в data-store (производное)

Synced write → WAL append + fsync (уровень 3) → ACK
```

### Безотказность WAL-append

WAL-append — последовательный `write()` в ОС:
- нельзя отклонить по backpressure;
- нельзя вытеснить по памяти (NO eviction);
- переживает крах процесса.
- Единственный отказ — потеря питания до фонового fsync.

### Data-store остаётся буферным (уровень 1)

Data-store не источник истины. Потерял неслитое при крахе →
recovery перепроигрывает WAL и пересобирает. MemBuffer для данных —
законный кэш.

### RYOW (Read Your Own Writes)

Обеспечен overlay-буфером: данные видны из буфера сразу после publish,
до durable-drain. Никакого ожидания фонового flush для чтения
собственных записей.

---

## Граница defer / sync

| Синхронно на ack (видимость/порядок) | В фон + батч (физика/долговечность) |
|---|---|
| WAL `write()` — точка ack для default | WAL fsync (ур.2→3, батчевый) |
| SSI footprint (`record_commit_writes`) | drain буфер→durable backing |
| assign_version + publish/watermark | index postings, HNSW, маркеры |
| materialize-в-overlay (дёшево, для RYOW) | WAL truncation |
| — | Synced: fsync ждётся inline |

SSI footprint + publish **обязаны** быть синхронными — это
видимость/порядок, не durability. На крахе восстанавливаются из WAL.

---

## Регресс: два узких места (вывод D0a, подтверждён из кода)

Регресс concurrent-commit на высоком N имеет ДВА независимых источника на
разных слоях — и единая staging-модель закрывает оба:

1. **Durable-бэкенд: fsync per-tx.** Каждый коммит делает свой fsync.
   Лечится **D1** (GroupFsync — батчевый fsync на окно).
2. **In-memory: materialize под uwl.** `commit_tx_lockfree` передаёт
   `uwl_guards` *внутрь* `materialize` (commit.rs:474) — per-table
   unique-write-lock держится через весь materialize, поэтому same-table
   коммиттеры сериализуются на полную длину materialize + scheduler-churn
   32 независимых таск. fsync тут ни при чём (in-memory). Лечится **D2** —
   переносом materialize с ack-пути в фоновый батчер: ack наступает после
   WAL `write()` + publish (дёшево), materialize-в-overlay остаётся
   синхронным и дешёвым, drain-в-durable уходит в фон. uwl тогда покрывает
   только короткий validate, а не materialize.

**Важно для D2:** перенос materialize-в-overlay должен сохранить
same-table MVCC-упорядочивание по commit-version (overlay versioned →
concurrent same-table на разных версиях не конфликтуют; uwl сужается до
validate). Магнитуда (доля каждого узкого места) — не измерена; D0a дал
структурный вердикт, эмпирику снимаем валидирующим бенчем после D1d/D2.

---

## Tiers (после реализации)

| Tier | Уровень | Ack-точка | Поведение |
|---|---|---|---|
| **default** | 2 | после WAL `write()` | фоновый fsync + materialize |
| **Synced** | 3 | после WAL `fsync` | inline fsync, materialize в фоне |

`AsyncIndex` поглощается: его контракт (ack после WAL+data+publish,
индексы/маркеры в фоне) — это default + publish, что уже так.

---

## Реализация B — файловый WAL (ВЫБРАНО)

Бэкенды (sled/redb) не дают чистого уровня 2: `set` = user-space буфер
(уровень 1), `flush` = write+fsync (уровень 3). Крейт `shamir-wal` тоже
KV-бэкан (маркеры в `info_store`, lib.rs:34) — не файловый. Поэтому для
контракта «default = уровень 2» строим настоящий append-only файловый WAL.

### Новый примитив: `WalSegment` (в крейте shamir-wal)
- Append-only сегмент-файл в каталоге репозитория (`wal/NNNNNN.log`).
- Кадр: `[u32 len][payload][u32 crc32]`. payload = `WalEntryV2::encode()`
  (переиспользуем существующую кодировку). CRC — «checksums everywhere».
- `append(payload) -> seq`: `write()` в page cache (**уровень 2**), без
  fsync. Точка ack для default.
- `sync()`: fsync (**уровень 3**). Точка ack для Synced. Батчем + фоновый
  таймер ограничивает окно потери питания для default-записей.
- `replay() -> Vec<WalEntryV2>`: чтение от начала, проверка CRC, декод;
  обрыв на первом рваном кадре (хвост краха — отбросить, не ошибка).
- Ротация/усечение: после чекпойнта (всё materialized+fsync'd в data store).

### Группировка
Конкурентные append'ы — через вращающегося лидера (механизм D1b GroupFsync
переиспользуем, но цель = WalSegment, не `Store`): N payload'ов → один
`write()`. Двухуровневое завершение: default-waiters просыпаются после
`write()` (уровень 2), Synced-waiters — после `fsync()` (уровень 3).

### Интеграция
- `RepoWalManager` строит `WalSegment` над файлом в каталоге репо.
- **InMemory-репо: файла нет → durability не обещается** (всё в RAM).
  РЕШЕНИЕ: для InMemory используем no-op/in-memory sink (текущий
  KV-маркер) — durability-контракт применим только к disk-репо.
- Recovery: `replay()` файла → применить committed tx в data store
  идемпотентно (как текущий V2-recovery через `touch_with_id` + ops).
- commit path: default ack после append-`write()`, Synced ждёт `sync()`.

### Задачи реализации B (re-scope D1)
- **W1** (=D1a) `WalSegment` primitive: формат + append/sync/replay + CRC + тесты. Additive.
- **W2** (=D1b ✅ → адаптировать) двухуровневый group-append/fsync поверх `WalSegment` (Waiter с tier default/Synced).
- **W3** (=D1c) wire в `RepoWalManager` + InMemory no-op sink + путь каталога репо.
- **W4** (=D1d) commit ladder: default→`write()`, Synced→`fsync()`.
- **W5** (новая) recovery replay файлового WAL + ротация/усечение после чекпойнта.

---

## Recovery

На крахе:
1. Прочитать WAL (уровень 2+ пережил крах процесса).
2. Для каждого undrained entry: перепроиграть — `touch_with_id`
   (interner) + apply ops → data-store.
3. Пересобрать CompletionTracker / watermark из WAL-маркеров.
4. Данные в data-store, не подтверждённые WAL → отбросить.

Инвариант: после recovery состояние = все committed записи из WAL,
ни больше ни меньше.

---

## План реализации (D-план)

### Секвенсинг

```
D-DOC (этот документ) ← done
  │
D0a ─┐ research: in-memory профиль per-phase n_32
D0b ─┘ research: проверка WAL-уровня + durable baseline
  ▼
D1a → D1b → D1c → D1d   KEYSTONE: WAL write-through + GroupFsync + лестница
  ▼
D2   run_leader → фоновый батчер (вне ack-пути)
  ▼
D3   tiers: default(ур.2) / Synced(ур.3); AsyncIndex поглощён

D4 (crash-injection) ─┐
D5 (стресс-качество) ─┼─ параллельно после D1d
D6 (nextest pin) ─────┘
```

### Фазы

#### D0 — Измерить и проверить (research)

**D0a — профиль регресса (in-memory).**
Разложить `wire_pipelining/sync/n_32` same-table по фазам:
`wal.begin` / `record_commit_writes` / materialize-setup /
uwl-contention / scheduler-churn. Назвать доминирующий член.
- Файлы: `crates/shamir-engine/benches/`, инструментовка `commit.rs:417–475`.

**D0b — WAL-уровень + durable baseline.**
1. Подтвердить: `__tx__` info_store = MemBuffer → WAL на ур.1 (баг).
2. Durable baseline: N∈{1,8,32}, same/disjoint, fsync-count.
- Файлы: `repo_wal_manager.rs`, `repo_instance.rs`, `storage_membuffer.rs`.
- Done: таблица per-phase + вердикт + go/no-go. <400 слов.

#### D1 — WAL-хребет + лестница ack (KEYSTONE)

**D1a — вынуть WAL из буфер-семейства.**
WAL-append = write-through в ОС (ур.2). Никакого MemBuffer/eviction на
WAL-пути. Data-store остаётся за MemBuffer.
- Файлы: `repo_instance.rs` (создание `__tx__` store), `repo_wal_manager.rs`.
- Безотказность: отказ только при потере питания.

**D1b — GroupFsync (scaffold, additive, `#[allow(dead_code)]`).**
Lock-free: SegQueue/scc pending + AtomicU64 cur_gen + watch<u64> flushed
+ AtomicBool leader-election.
- Файлы: новый `crates/shamir-tx/src/group_fsync.rs`.
- Тесты: один append→flush; N concurrent→один generation; follower wakes;
  re-election mid-flush.

**D1c — wire в RepoWalManager.**
`begin_grouped(entry)` через GroupFsync. Round-trip: durable-байты
идентичны текущему `begin`.

**D1d — переключить commit_tx_lockfree на лестницу.**
- default → ack после WAL write() (ур.2);
- Synced → ack после fsync (ур.3).
- Вшить rationale-комментарий: `record_commit_writes` до publish
  намеренно — пропущенный конфликт строго хуже ложного abort'а.
- Тесты: `@oracle` + `@e2e`. Бенч D0b → fsync/окно ≈ 1, регресс закрыт.

#### D2 — run_leader → фоновый батчер

1. Решить судьбу AsyncIndex (гипотеза: поглощается лестницей). STOP если неясно.
2. run_leader: снять с ack-пути → фоновая задача: drain буфер→durable,
   index, HNSW, маркеры, WAL truncation. Lock-free (CAS-leader + watch).
3. Удалить мёртвое (PendingCommit, conflicts_with — если фоновому не нужны).
- Файлы: `group_commit.rs`, `commit.rs`, `repo_tx_gate.rs`.
- Done: батчер вне ack-пути; AsyncIndex задокументирован; LOC-дельта.

#### D3 — Рационализация tiers

Свести Durability: **default (ур.2)** / **Synced (ур.3)**.
AsyncIndex поглощён. Проплести через query-builder + batch_execute.
Если меняется wire-формат — согласовать отдельно.

#### D4 — Crash-injection (параллельно, после D1d)

Seam'ы:
- (c) mid-materialize (между data и index writes);
- (d) after-materialize / before-mark;
- **(NEW)** (e) WAL durable, data ещё не materialized (фоновый drain не
  успел) → recovery перепроигрывает из WAL.
Recovery-тесты: конвергенция, watermark, нет orphaned версий.

#### D5 — Стресс-качество (параллельно)

- Реальный SSI-provider (не AlwaysConflictProvider);
- Barrier для одновременного старта;
- Пин семантики gap watermark (liveness-баг если stall);
- Flake-hunt (200 итераций).

#### D6 — nextest pin (параллельно)

Задокументировать зависимость guard'а от `$NEXTEST`; пин версии.

---

## Инварианты (STOP при сомнении)

1. **SSI-окно атомарно:** footprint (до publish) + predicate-conflict —
   порядок не менять.
2. **WAL — единственный источник истины:** всё остальное производно.
3. **Безотказность WAL-append:** отказ только при потере питания.
4. **Тесты только через `./scripts/test.sh`**; gate = fmt + clippy +
   @oracle/@e2e. Без коммита/пуша/агентов без явной просьбы.
