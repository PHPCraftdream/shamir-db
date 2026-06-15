# F6 — WAL Truncation / Segment Rotation (subplan)

Закрывает последний этап дорожной карты WAL (`wal-refactor.md`, F0–F6).
Базовая ветка `master`; предусловие — **D2 завершена** (`durable_watermark`
существует и продвигается дренажом). Pre-F6 baseline = коммит P1e.

---

## 0. Проблема

Сегодня WAL — **один растущий файл** (`<name>.shamirwal/repo.wal`, либо
`MemSink` Vec для in-memory репо). `RepoWalManager::commit(txn_id)` — **no-op**:
записи живут в сегменте вечно. Два следствия:

1. **Диск растёт безгранично.** Каждая транзакция дописывает запись; ничто её
   не удаляет. БД, прожившая долго, исчерпает диск.
2. **Дренаж — скрытый O(N).** `Drainer::drain_step` каждый проход вызывает
   `wal.recover()` = `replay()`, читающий **весь** сегмент, и фильтрует окно
   `(durable_wm, last_committed]`. По мере роста сегмента каждый дренаж читает
   всё больше — стоимость прохода растёт линейно с историей жизни БД, не с
   размером недренированного хвоста. То же для cold-recovery на открытии.

P1e ограничил **overlay** (RAM) окном `(durable_wm, last_committed]`, но **WAL
на диске** и **объём, читаемый дренажом**, остаются неограниченными. F6 их
ограничивает.

---

## 1. Целевое состояние

WAL репо — не один файл, а **директория нумерованных сегментов**
`<name>.shamirwal/NNNNNNNN.wal` (zero-padded seq, лексикографически =
хронологически). В каждый момент:

- **ровно один active сегмент** — принимает append'ы (хвост);
- **ноль или более sealed сегментов** — закрыты, только для replay/удаления.

Жизненный цикл:
```
append → active сегмент
  active.bytes >= WAL_SEGMENT_MAX_BYTES  ⟹  seal(active) + open(new active)
durable_watermark продвинулся дренажом:
  для каждого sealed S с  max_commit_version(S) <= durable_watermark:
    данные S уже durable в history  ⟹  delete файл S
```

`replay()` = конкатенация всех сегментов в seq-порядке (sealed по возрастанию +
active последним). Active сегмент удалять нельзя НИКОГДА (в него пишут и он
держит недренированный хвост).

**Почему ротация сегментов, а не in-place compaction одного файла:** удаляются
только **запечатанные** (не-active) сегменты — append-путь и truncation-путь
никогда не трогают один файл одновременно, нулевая координация
writer↔truncator. In-place rewrite потребовал бы эксклюзивной блокировки файла
на время перезаписи (стойл commit-пути). Совпадает с §2/§3.6 `wal-refactor.md`
(`wal/NNNNNN.log`) и с lock-free идеологией.

---

## 2. Инварианты (STOP при сомнении)

I1. **Truncation только после durable-materialize.** Сегмент S удаляется лишь
    когда КАЖДАЯ его запись durable в data-store (history), т.е.
    `max_commit_version(S) <= durable_watermark`. `durable_watermark`
    продвигается дренажом ТОЛЬКО после `replay_v2_entry` → `history.transact`.
    Контрапозиция: данных нет в history ⟹ версия > durable_wm ⟹ её сегмент жив
    ⟹ recovery доиграет. Нельзя потерять.

I2. **Durability data-store перед удалением (power-loss).** `mark_durable`
    означает «записано в history», но не обязательно `fsync`'нуто на диск.
    Для контракта уровня-3 (power-loss) перед физическим `delete` сегмента
    data-store должен быть `flush`/`fsync`'нут до `max_commit_version(S)`.
    F6 ставит **checkpoint-fsync** data-store на границе truncation (не
    per-tx — амортизируется по сегменту). Для уровня-2 (process-crash) этого не
    требуется (history переживает крах процесса), но gate ставим безусловно —
    дёшево (один fsync на сегмент) и закрывает power-loss.

I3. **Active сегмент неприкосновенен.** Никогда не удаляется и не
    перезаписывается; только append + (при ротации) seal.

I4. **Рваный хвост — только на active.** Torn-tail (крах во время write)
    возможен лишь в конце active сегмента; sealed сегменты дописаны целиком до
    seal. `replay` отбрасывает рваный хвост active (как сейчас в `WalSegment`),
    sealed читаются целиком.

I5. **`commit_version > 0` для всякой живой записи.** Truncation по версии
    предполагает, что у каждой записи есть монотонный `commit_version`. Записи
    с `commit_version == 0` (legacy/не-версионные) НЕЛЬЗЯ удалять по версии:
    они «пинят» сегмент (его `max_commit_version` считается `u64::MAX`, т.е.
    сегмент с любой v=0 записью не truncatable пока не подтверждён иной
    механизм). **Реализация обязана проверить**, что живые prod-пути
    (`commit.rs`, `group_commit.rs` batch, `pre_commit.rs`, non-tx
    `set_versioned`) всегда ставят `commit_version > 0`; если какой-то путь
    эмитит 0 — это пин (безопасно: не теряем), но фиксируем как долг.

I6. **Idempotent replay (без изменений).** Удаление сегмента + повторный replay
    оставшихся идемпотентен (last-write-wins). Крах в любой момент
    delete-последовательности безопасен: файл либо есть (replay переиграет —
    данные уже в history, идемпотентно), либо удалён (его данные durable).
    Удаление одного файла — атомарная FS-операция.

I7. **Mem-sink.** Для in-memory репо durability бессмысленна, но RAM растёт.
    `MemSink` получает то же truncation-API (drop frames с
    `commit_version <= durable_watermark`), чтобы in-process replay-паритет и
    RAM-границы держались. fsync-gate (I2) для Mem — no-op.

---

## 3. Декомпозиция

Ритм кампании: **additive scaffold → cutover → crash-tests**, каждый шаг
gated+reviewed (как P1d-2a → P1d-2b → P1d-2c).

### F6a — Segmented sink (additive, НЕ подключён)

Крейт `shamir-wal`. Чисто аддитивно: новый сегментированный сток существует
рядом, prod ещё на одиночном `WalSink::File`.

- **Тред `commit_version` через append-путь.** Сигнатуры (additive param):
  - `WalSegment::append_batch(payloads, max_version: u64)` — пишет батч,
    обновляет `max_committed: AtomicU64 = max(prev, max_version)`, возвращает
    `last_seq` (как сейчас).
  - `WalSink::append_batch(payloads, max_version)` — File делегирует, Mem
    обновляет свой `max_committed` (для frame-level GC).
  - `WalGroupCommit`: `Pending = (Vec<u8>, u64 /*version*/, WalDurability,
    Arc<Waiter>)`; лидер берёт `max` версий батча и передаёт в
    `sink.append_batch(payloads, batch_max_version)`. `append(payload,
    version, durability)`.
  - `RepoWalManager::begin_grouped(entry, durability)` уже знает
    `entry.commit_version` → `group.append(encoded, entry.commit_version,
    durability)`; `begin_grouped_many` аналогично.
- **`SegmentSet`** (новый тип, один файл = один primary export) — владеет
  директорией: `Vec` sealed (seq, path, max_version) + active `WalSegment`.
  - `open(dir)` — сканирует `*.wal`, сортирует по seq, последний = active
    (остальные sealed; их `max_version` вычисляется один раз при открытии
    через `replay`-of-segment — редко, амортизируется).
  - `append_batch(payloads, max_version)` → active; если
    `active.bytes >= WAL_SEGMENT_MAX_BYTES` → `seal` (записать max_version,
    переложить в sealed) + `open` нового active с `seq+1`.
  - `replay()` → конкатенация sealed (по seq) + active.
  - `truncate_below(durable_version) -> usize` — удаляет файлы sealed S c
    `max_version(S) <= durable_version` (но `max_version==0`/pin не трогает,
    I5); возвращает число удалённых.
  - `sync()` → active.sync() (sealed уже на диске).
- **`WalSink::File`** становится `File(SegmentSet)` (вместо `WalSegment`).
  `WalSink::truncate_below` / `Mem` frame-GC.
- **Тюнабл** `WAL_SEGMENT_MAX_BYTES` в `shamir-tunables` (старт: 8 MiB —
  достаточно крупный, чтобы ротация была редкой; задокументировать trade-off).
- **Тесты `shamir-wal`** (`tests/`): rotation-on-size, replay-across-segments
  байт-идентичен одиночному, truncate-drops-drained-keeps-undrained,
  truncate-never-drops-active, torn-tail-on-active-only, v=0-pins-segment,
  Mem frame-GC. Gate: `@storage`.

### F6b — Cutover (truncation подключена)

- `repo_instance.rs` строит `WalSink::File(SegmentSet::open(wal_dir))` вместо
  одиночного `WalSegment::open(repo.wal)`. Миграция: существующий
  `repo.wal` — это один сегмент seq=0; `SegmentSet::open` должен принять
  legacy-имя ИЛИ open-path переименовывает `repo.wal` → `00000000.wal` один
  раз. (Решение: `SegmentSet::open` распознаёт legacy `repo.wal` как seq=0.)
- `RepoWalManager`: новый `truncate_below(durable_version)` (делегат в
  sink); `commit(txn_id)` остаётся no-op (per-entry маркеров нет) — truncation
  теперь по версии, не по txn_id.
- **data-store fsync-gate (I2):** перед `truncate_below` дренаж
  `flush`'ит history до `durable_watermark`. Найти/добавить
  durability-seam data-store (`flush_buffers`? backend `sync`?); если history
  на sled — `flush`; на in-memory — no-op.
- `Drainer::drain_step`: после overlay-GC (P1e) вызвать
  `wal.truncate_below(durable_watermark)` (амортизированно — труанкатор лишь
  удаляет уже-sealed сегменты; обычно no-op, срабатывает при пересечении
  границы сегмента). Порядок: history-fsync → truncate.
- **drain-cost:** опционально `recover()` дренажа сужается до недренированного
  хвоста (раз sealed-drained удалены, `replay()` уже не отдаёт их — стоимость
  падает «бесплатно» от F6a). Замерить.
- Gate: `@oracle @e2e` + существующий crash-suite (`crash_recovery.rs`) зелёный.

### F6c — Crash-injection + growth-limit

- Новые crash-seam'ы вокруг truncation: `maybe_crash("pre_truncate")`,
  `maybe_crash("post_seal")`, `maybe_crash("mid_delete")` (между unlink двух
  сегментов). Сценарии в `crash_recovery.rs`: крах до/после seal, до/после
  delete; recovery восстанавливает БД байт-идентично, ноль потерь.
- **Growth-limit тест** (D4-смежный): прогнать N≫сегмент коммитов с дренажом,
  утвердить, что число файлов сегментов ограничено ≈
  `undrained_window / segment_size + O(1)`, не растёт с N.
- non-flaky над 20 прогонами.
- Gate: crash-suite + growth-limit.

---

## 4. Риски

- **Миграция legacy `repo.wal`.** Существующие БД имеют одиночный `repo.wal`.
  `SegmentSet::open` обязан его подхватить (как seq=0), иначе при апгрейде
  recovery «потеряет» историю. Покрыть тестом open-over-legacy.
- **data-store durability seam.** Если у history нет дешёвого `flush`-до-версии,
  I2-gate может оказаться грубым (полный fsync БД на truncation). Приемлемо
  (редко), но измерить; не блокирует корректность.
- **Батч, пересекающий ротацию.** Решение: ротацию проверяем ПОСЛЕ записи всего
  батча (сегмент может слегка превысить max-bytes; батч никогда не straddl'ит
  два файла) — упрощает torn-tail (I4) и max_version-учёт.
- **Seq-переполнение / порядок.** zero-pad 8 цифр (до 10^8 сегментов × 8 MiB =
  огромный объём); при необходимости u64-hex. Лексикографический порядок =
  хронологический.
- **CAPSTONE-связь.** F6 вводит `SegmentSet` поверх `Arc<Mutex<File>>` (active)
  + `Vec` sealed. CAPSTONE (single-writer-task) перепишет это; F6 не должен
  цементировать лишних локов — sealed-список меняется только труанкатором и
  лидером ротации (редко); держать его за дешёвым примитивом, не на hot-append.

---

## 5. Порядок и gate

`F6a (@storage) → F6b (@oracle @e2e + crash) → F6c (crash + growth)`.
Каждый шаг: `fmt --check` + `clippy --all-targets -D warnings` +
`./scripts/test.sh` (через центральную точку), коммит по согласованию.
После F6 — D4 (crash-completeness, поглощает F6c-сценарии), затем CAPSTONE.
