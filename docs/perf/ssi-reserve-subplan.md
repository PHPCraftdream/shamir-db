בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# SSI cell-reservation — явная точка сериализации (fix task #24)

**Баг (подтверждён, коммит-улика + repro24):** SSI «exactly one wins» НЕ держится
для **non-unique** таблиц под параллелизмом. `repro24_concurrent_ssi_storm_multithread`
(`acceptance_tests.rs`, `#[ignore]`, multi_thread×4) → **40/40 раундов >1 победителя**
(ok_count 3–20 при ожидаемом 1). Причина: «кто победил» решается публикацией
cell-версии в **Phase 5a — ПОСЛЕ** точки коммита (Phase 4 WAL begin). N коммиттеров
валидируют read-set против ОДНОЙ pre-commit cell-версии (никто ещё не опубликовал),
все проходят, все публикуют. Для non-unique нет `uwl_guard` (только unique,
`pre_commit.rs:127-136`), нет `commit_mutex` в lockfree-пути, нет version-CAS.

**Форма фикса:** инверсия порядка «фиксируй→решай» в «**реши→фиксируй**». Решение
«кто победил» переносится из publish (Phase 5a) в **атомарное притязание ДО Phase 4**,
так что проигравший отваливается `SsiConflict`'ом, **не коснувшись WAL**.

---

## 1. Механизм — притязание на ячейке (lock-free, без Mutex)

`RecordCell` (`mvcc_store/mod.rs:90`) хранится в `cells: SccHashMap<Bytes, RecordCell,
THasher>` (`:105`). scc `entry`/`entry_async` даёт **per-entry эксклюзив** — атомарную
check-and-update «бесплатно» (как уже использует `publish_cell_sync:336`). Поле +1:

```rust
pub(crate) struct RecordCell {
    version: u64,        // последняя ОПУБЛИКОВАННАЯ версия (как сейчас)
    reserved_by: u64,    // 0 = свободна; иначе txn_id притязателя
}
```

Три атомарных акта, все через `cells.entry(key)`:

1. **Claim** (pre-commit, ПОСЛЕ read-validate, ДО assign/WAL):
   `if version <= my_snapshot && reserved_by == 0 → reserved_by = my_txn; WON
    else → SsiConflict`. Первый побеждает; конкуренты видят `reserved_by != 0` или
   ушедшую `version` → abort. Раздор НЕ ждёт — отваливается.
2. **Finalize** (publish, Phase 5a, тот же `publish_cell_sync`):
   `version = commit_version; reserved_by = 0` — в одном `entry`.
3. **Release** (abort, RAII Drop): `if reserved_by == my_txn → reserved_by = 0`.

Читатели притязание **игнорируют** (читают `version`) — оно блокирует только писателей.

---

## 2. Где (file:line)

| Шов | Место |
|---|---|
| read-validate (DETECT) | `pre_commit.rs:208` (`validate_read_set` → `version_of` → `MvccStore::current_version` `mvcc_store/mod.rs:787`) |
| **claim write-set** (новое) | `pre_commit`, рядом с `uwl` (`:127-136`), ПОСЛЕ read-validate, ДО assign (`:249`) и ДО Phase 4 WAL (`commit.rs:608`) |
| `RecordCell` + finalize | `mvcc_store/mod.rs:90`; `publish_cell_sync:336` (+ async `publish_cell:320`) |
| publish call-site (Phase 5a) | `apply_committed_visible` → `publish_cell_sync` (`mvcc_history.rs:317`) |
| RAII release | зеркало `VersionGuard` (`crates/shamir-tx/src/version_guard.rs`) → новый `CellReservationGuard` |

---

## 3. Декомпозиция (тропа D2: аудит → additive → cutover → стресс)

- **R0 — фиксация дизайна** (этот doc). Открытый вопрос для S1-агента: **экспонирован
  ли read-write skew** (две tx читают X, пишут Y/Z — чистое write-skew) или баг строго
  write-write? Repro write-write. Cell-reservation покрывает write-write (подтверждённый
  баг); read-write skew — отметить, при экспозиции — отдельная волна.
- **S1 — примитив притязания** (additive, НЕ подключён): `reserved_by` в `RecordCell`;
  `try_reserve(key, snapshot, txn) / finalize(key, version) / release(key, txn)` на
  `MvccStore` (атомарны через `entry`); `CellReservationGuard` (RAII, Drop→release всех
  взятых). Юнит-тесты `shamir-tx`. *Gate: @oracle — байт-идентично (поле есть, никто не
  зовёт; `RecordCell` init `reserved_by: 0`).*
- **S2 — cutover**: claim write-set в `pre_commit` после read-validate, до WAL;
  проигравший → `SsiConflict` ДО Phase 4; finalize в publish (Phase 5a); guard release
  на abort. **Снять `#[ignore]` с `repro24` → зелёный.** *Gate: repro24 20× зелёный +
  @oracle + @e2e + crash-suite.*
- **S3 — стресс/краш/lifecycle**: multi-key write-set (no deadlock — раздор отваливается);
  multi-thread storm-варианты; краш не оставляет утечки притязания (volatile cell-state,
  восстанавливается из WAL-победителей); полный crash-suite 20× non-flaky.
  *Gate: @e2e + crash.*

Каждый шаг: `fmt` + `clippy --all-targets -D warnings` + `./scripts/test.sh`, коммит по вехе.

---

## 4. Инварианты (proof-обязательства)

1. **I-Atomic** — check-no-conflict и claim = один атомарный акт (scc per-entry эксклюзив).
2. **I-PreWAL** — проигравший abort'ит ДО Phase 4 → **WAL держит только победителей**.
3. **I-NoWait** — раздор → `SsiConflict` (abort), никогда block → нет deadlock/livelock
   (multi-key claim в любом порядке: провал освобождает взятое и отваливается).
4. **I-Reader** — притязание невидимо читателям (читают `version`).
5. **I-Crash** — притязание volatile; краш → перестроено из WAL (победители) → нет утечки.
6. **I-Compose** — композится с `VersionGuard` (abort освобождает оба); overlay/drainer/
   history нетронуты (притязание ортогонально durability — это freshness/serialization
   marker, не данные).

---

## 5. Открытые тонкости (S1-агент проверяет)

- **Версия в claim-условии.** `version <= my_snapshot` отвергает писателя, чья ячейка
  УЖЕ ушла вперёд снапшота (кто-то опубликовал) — это и есть stale-write detection.
  Проверить: `RecordCell.version == 0` для никогда-не-публиковавшейся ячейки (Vacant) —
  claim на Vacant: вставить `RecordCell { version: 0, reserved_by: my_txn }`.
- **txn_id уникальность.** `reserved_by = my_txn` — txn_id монотонен (`fresh_txn_id`),
  годен как owner-маркер для release-проверки.
- **Где взять snapshot и write-set.** `tx.snapshot_version`; write-set — ключи, которые
  tx пишет (из `tx`-footprint / ops). Свериться с тем, что `validate_read_set` уже знает.
- **Финализация версии vs reserved.** Между claim (reserved_by=my_txn, version старая) и
  finalize (version=commit_version) ячейка показывает СТАРУЮ version читателям — корректно
  (новое ещё не durable/visible). publish переводит атомарно.
- **uwl_guard сосуществует** — он сериализует unique-ИНДЕКС (enforce uniqueness), это иной
  инвариант, чем write-write на data-cell. НЕ удалять; cell-reservation добавляется рядом.
