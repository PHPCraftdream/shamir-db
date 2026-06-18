בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Storage speed-up — волны и этапы (agent-executable spec)

Производный план из `docs/perf/storage-speedup-research-2026-06-19.md` (15 рычагов
L1–L15) и `docs/perf/bench-snapshot-2026-06-18.md` (база замеров). Каждый **этап**
здесь — самодостаточный бриф, который может взять один Sonnet-агент: точные
якоря кода (сверены против HEAD при написании), точечное изменение, инварианты,
тесты, гейт, критерий готовности.

**Объём волны (решение пользователя): «11 — до floor».** В работе — L1, L2, L3,
L5+L14, L6, L9, L10, L12, L13, L15 + Фаза 0. **L4/L7 (ковыряние в redb) и L8
(смена backend) — вне волн** (см. §«Вне волн» и §«Граница»).

Сквозной тезис (из созерцания): мы построили LSM поверх KV (WAL=лог,
overlay=memtable, drainer=flush, backend=cold tier). Все головные рычаги — это
**обвязка** (ритм кормления + батч-чтение + read-through кэш), а НЕ backend-код.
Цель — redb-batch-потолок 41k/s на durable-пути; гарантия — ни одна single-row
redb-commit не на горячем пути. memory-178k/s на диске недостижим — честная
граница.

---

## 0. Контракт агента (ОБЯЗАТЕЛЬНО для каждого этапа)

Один этап = один агент = один коммит. Перед стартом агент читает этот §0 и свой
этап целиком.

**Модель.** Запускать агентов с `agentType:'ao46l'` (Opus 4.6 low) — это
итоговый выбор пользователя для всей волны. `agentType` меняет только системный
промпт; модель — отдельно. В Workflow указывать `model:'opus'` явно (без этого
агент наследует модель главного цикла). Исключение по строгости:
**L1 и L14+L5** (durability-критичные) — после реализации прогнать ОТДЕЛЬНЫЙ
независимый ревью-проход (тоже Opus) перед коммитом.

**Процедура каждого этапа:**
1. **Сверить якоря.** Номера строк МОГЛИ сдвинуться. Грепнуть цитируемые символы
   (имена функций/полей), убедиться что код соответствует «Текущее состояние».
   Если структура изменилась — остановиться и доложить оркестратору, не угадывать.
2. **🔴 Red.** Написать падающий `#[tokio::test]`, кодирующий целевое поведение
   (или расширить существующий тест-файл по топику — см. §«Test organisation» в
   `CLAUDE.md`; тесты живут в `tests/`-каталоге модуля, не inline).
3. **🟢 Green.** Минимальное изменение, делающее тест зелёным.
4. **🔵 Refactor.** Прибраться, держа сюиту зелёной.
5. **Гейт (scoped):**
   ```
   cargo fmt -p <crate> -- --check          # НЕ fmt --all (полирует весь репо)
   cargo clippy -p <crate> --all-targets -- -D warnings
   ./scripts/test.sh @oracle                # tx + engine; ИЛИ -p <crate>
   ```
   Тесты — ТОЛЬКО через `./scripts/test.sh` (raw `cargo test` заблокирован
   perimeter-guard'ом). Для durability-этапов (L1, L6) — дополнительно
   `./scripts/test.sh @e2e --full` в цикле под нагрузкой (см. §«Durability-чекпоинт»).
6. **Анти-додж self-review.** `git diff` не должен содержать: `#[allow(...)]` для
   обхода, закомментированных инвариантов, «починки» теста под баг, выключенных
   проверок, осиротевших TODO. Не трогать код вне задачи (discipline §`CLAUDE.md`).
7. **Доклад diff'а** оркестратору. **Коммитит оркестратор** (агент НЕ коммитит,
   НЕ пушит — см. глобальные запреты). Один рычаг = один коммит. Промежуточное
   состояние коммитить ПЕРЕД любым откатом (урок: ~850 правок потеряны на
   незакоммиченном `git checkout`).

**Бенч-изоляция.** Любой бенч — ТОЛЬКО с выделенным таргетом, иначе ломается
инкрементальный кэш test/clippy:
```
CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p <crate> --bench <name>
```
Quick-режим — дефолт (sample_size=10). Гейт прогонять ОДИН раз в конце цикла, не
между baseline/post бенч-прогонами.

**Ключевые типы/хелперы (общие для MVCC-этапов):**
- `KvOp::Set(Bytes, Bytes)` / `KvOp::Remove(Bytes)` — `shamir_storage::types::KvOp`.
- `encode_version_key(key: &[u8], version: u64) -> Bytes` — `crate::version_codec`
  (layout `key || VERSION_SEP(0xFF) || version_be`).
- `decode_version_key(&[u8]) -> Option<(orig, version)>` — отвергает ts-keys.
- `ts_key(version: u64) -> Bytes` — `mvcc_store/mod.rs:67`, layout
  `[TS_TAG(0x00)][version_be:8]` (9 байт). Значение ts — `ms.to_le_bytes()` (8 байт LE).
- `history: Arc<dyn Store>` — внутри `MvccStore`; `Store::transact(Vec<KvOp>)`,
  `Store::get_many(Vec<Bytes>)`, `Store::set/get/remove`.

---

## Карта волн

| Волна | Этап | Рычаг | Файл (verified) | Усил. | Durability-чекпоинт |
|---|---|---|---|---|---|
| **0** | 0.1 | bench-snapshot на HEAD | benches | S | — |
| **0** | 0.2 | redb 3.1 cacheability spike | — (research) | S | — |
| **1** | 1.1 | **L2** fold ts в transact | mvcc_history.rs, mod.rs | S | нет |
| **1** | 1.2 | **L15** fuse point-read alloc | mod.rs, version_codec.rs | S | нет |
| **1** | 1.3 | **L9** has_any_index guard | table_manager_tx_ops.rs | M | нет |
| **1** | 1.4 | **L13** hoist RecordId clock | table_manager_tx_ops.rs, record_id.rs | M | нет |
| **1** | 1.5 | **L12** reuse encode scratch | write_exec.rs, messagepack.rs | M | нет |
| **2** | 2.1 | **L1** coalesce Drainer | drainer.rs, recovery.rs, mvcc_history.rs | L | **ДА** |
| **2** | 2.2 | **L3** batch MVCC read | table_manager_crud.rs, mod.rs | L | нет (read-only) |
| **3** | 3.1 | **L14+L5** read-through MemBuffer | repo wiring, storage_membuffer.rs | L | **ДА** |
| **3** | 3.2 | **L6** O(1)/deferred vacuum | mvcc_gc.rs, mod.rs | L | **ДА** |
| **4** | 4.1 | **L10** engine-floor (a/b/c) | commit.rs, wal_group_commit.rs | L | **ДА** (a/b) |

Зависимости: всё после **0.1** (сначала замер). **2.1 (L1)** после **1.1 (L2)** —
L2 упрощает группу. **3.x** после **2.1**. **2.2 (L3)** ∥ **2.1** (независимая
подсистема). **4.1** последним.

---

## Волна 0 — Замер (gate всей волны) · таск #102

Все µs-доли в research — reasoned, НЕ замерены. Этот этап превращает оценки в
числа и решает, проходят ли микро-рычаги (L9/L12/L13/L15) планку.

### Этап 0.1 — Воспроизвести bench-snapshot на HEAD
**Цель:** свежие числа, breakdown горячих путей. Кода нет.
**Действия:**
```
CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-storage \
  --bench store_raw -- '(sled|redb|in_memory)/(insert/single|set_many/batch/100)'
CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-engine \
  --bench tx_pipeline -- 'tx_overhead/(single_insert|batch_pipeline)'
```
**Зафиксировать** в обновлённый `bench-snapshot`: реальную долю spawn_blocking-hop
в sled-single; долю `table.get` pre-check в redb-single; breakdown 254µs
engine-floor (для приоритизации L10 a/b/c).
**Done:** обновлённый snapshot-файл + вердикт «проходит/нет» по каждому
микро-рычагу (L9/L12/L13/L15: если их доля <1% — понизить приоритет/выкинуть).

### Этап 0.2 — redb 3.1 cacheability spike (для будущего L7, вне волн)
**Цель:** определить, можно ли в redb 3.1 переиспользовать open-table/read-snapshot
вне lifetime одной txn (TableDefinition тривиально кэшируем; вопрос — handle).
**Done:** один абзац-вывод в snapshot: «L7 существует как отдельный рычаг» / «L7
субсумируется батчингом L1/L3». Кода нет.

---

## Волна 1 — дешёвые изоляты (без durability-чекпоинта)

### Этап 1.1 — L2: свернуть record_ts в тот же history.transact · таск #103
**Выигрыш:** ~2× redb drain (убирает 2-ю write-txn/версию); стекается с L1. **S.**

**Текущее состояние (4 пути пишут ts отдельным вызовом):**
- `crates/shamir-tx/src/mvcc_store/mvcc_history.rs:357` `write_committed_to_history`:
  строит `history_ops` (369–376), `self.history.transact(history_ops)` (380),
  ЗАТЕМ отдельно `self.record_ts_at(commit_version, ts_ms).await` (395). ts_ms
  читается из `pending_ts.remove(...)` с fallback `now_millis()` (390–394).
- `crates/shamir-tx/src/mvcc_store/mod.rs:304` `record_ts_at` — это `self.history
  .set(ts_key(version), ms.to_le_bytes()).await` (306–309) = отдельная write-txn.
- `mod.rs:521` `set_versioned`: `history.set(version_key, value)` (540) ЗАТЕМ
  `record_ts(new_v)` (557).
- `mod.rs:595` `set_versioned_many`: `history.transact(history_ops)` (629) ЗАТЕМ
  цикл `for &v in &new_versions { record_ts(v) }` (643–645).
- `mod.rs:678` `delete_versioned`: `history.set(tombstone)` (695–697) ЗАТЕМ
  `record_ts` (после ~700 — сверить).

**Изменение (каждый путь — одна атомарная write-tx data+ts):**
1. `write_committed_to_history`: ПЕРЕНЕСТИ вычисление `ts_ms` (pending_ts.remove
   c fallback) ВЫШЕ построения `history_ops`; добавить в `history_ops`
   `KvOp::Set(ts_key(commit_version), Bytes::from(ts_ms.to_le_bytes().to_vec()))`;
   УБРАТЬ отдельный вызов `record_ts_at` (395). Импорт `use super::ts_key;`.
2. `set_versioned`: заменить `history.set(...)` + `record_ts(new_v)` на ОДИН
   `history.transact(vec![KvOp::Set(version_key, value), KvOp::Set(ts_key(new_v),
   ms_le)])`, где `ms = self.now_millis()` (один раз).
3. `set_versioned_many`: в цикле построения (617–625) ДОБАВИТЬ в `history_ops`
   ts-op на каждую версию; УБРАТЬ цикл record_ts (643–645). `ms = now_millis()`
   один раз на батч (синергия с L13).
4. `delete_versioned`: как `set_versioned` — tombstone + ts в один `transact`.

**Инварианты:**
- ts_key disjoint от version-keys (`TS_TAG=0x00 ≠ VERSION_SEP=0xFF`) — коллизии
  нет; `decode_version_key` уже отвергает ts-keys.
- **Семантика ошибок меняется НАМЕРЕННО:** раньше ts-write был best-effort
  (ошибка глоталась); теперь ts атомарен с data (общий `?`). Это строго лучше
  (нет осиротевшего/пропавшего ts на успешной data-записи). Если есть тест,
  утверждающий «ts best-effort при сбое backend» — пересогласовать его с новым
  атомарным контрактом (это не додж, а корректное изменение инварианта).
- `now_millis()` уважает тест-часы (`set_test_now`) — сохранить.

**Тесты:** в `crates/shamir-tx/src/tests/mvcc_store_tests/` — (a) после
`set_versioned`/`_many`/`delete` в history ровно ОДИН ts-key на версию с верным
значением; (b) age-retention (`vacuum_key` по `max_age_secs`) видит ts и
реклеймит корректно; (c) `version_at_or_before_ts` / `history_of` находят ts.
**Гейт:** `-p shamir-tx` + `./scripts/test.sh @oracle`.
**Done:** 0 отдельных `record_ts*` вызовов на горячих путях записи; ts едет в той
же `transact`; @oracle зелёный.

### Этап 1.2 — L15: fuse double-alloc point-read
**Выигрыш:** −1×16B alloc на point-read; компаундит под L3. **S.**

**Текущее:** `mod.rs:751` `get_current(key: Bytes)`. Вызыватели делают
`id.to_bytes()` (16B alloc, `record_id.rs`), затем `get_current` внутри строит
`encode_version_key(&key, v)` (2-й BytesMut, `version_codec.rs`). `current_version`
(897) уже принимает `&[u8]` (`cells.read(key, ...)` zero-alloc).

**Изменение:** дать point-read путь, принимающий `&[u8]`/`&[u8;16]` вместо
владеемого `Bytes`, чтобы 16B-alloc записи ключа не делался ради чтения, а
аллоцировался ТОЛЬКО version-key. Вариант: `get_current` принимает `&[u8]`
(клонирует в `Bytes` лишь там, где реально нужно — `seed_version`,
`overlay.get`), либо добавить `encode_version_key_from_id(id: &[u8;16], v)`.
Сверить вызыватели (`table_manager_crud.rs:408/429/451/473`).

**Инварианты:** `cells.read` уже принимает `&[u8]` (см. III.2 коммент на 887–896)
— байт-идентичный хэш/лукап. Не менять layout version-key (sorted-range
зависит). overlay-probe (798) и history.get (801) — те же ключи.
**Тесты:** существующие get_current/get_many зелёные; добавить точечный тест
«один alloc на чтение» если есть удобный счётчик, иначе — поведенческий.
**Done:** point-read не делает лишний 16B-alloc; @oracle зелёный.

### Этап 1.3 — L9: has_any_index guard
**Выигрыш:** unindexed-insert путь (~23.9k/s) — убирает per-batch
`all_backends().await` (Vec+scc-scan) + 3 planner-вызова при 0 индексов. **M.**

**Текущее (сверить — research-рефы):** `table_manager_tx_ops.rs:~585`
`insert_tx_many_bytes` безусловно зовёт `index2_registry.all_backends().await`
(`registry.rs:~86`) + 3 planner-вызова (`~604–613`) даже при пустом наборе
индексов. Паттерн уже есть: `has_unique_indexes()` (`index_manager.rs:~191`).

**Изменение:** добавить atomic-флаг `has_any_index()` (под тем же lock'ом, что
мутирует registry — authoritative, как `has_unique_indexes`); при `false`
пропустить весь блок index-planning + `all_backends`.

**Инварианты:** флаг authoritative — stale `false` ⇒ index corruption. Менять
флаг в той же критической секции, что добавляет/удаляет индекс. Indexed-путь —
байт-идентичен.
**Тесты:** insert в unindexed-таблицу (планнер не вызван — проверить через
поведение/счётчик); insert в indexed — индекс по-прежнему пишется; добавление
индекса флипает флаг.
**Done:** unindexed insert не трогает index-planning; @oracle + indexed-тесты зелёные.

### Этап 1.4 — L13: hoist RecordId clock-read из per-row loop
**Выигрыш:** −(N−1) clock-read/batch; pure-win, все backend. **M.**

**Текущее (сверить):** `RecordId::new()` per-row читает
`Utc::now().timestamp_micros()` (`record_id.rs:~28`). В batch
(`table_manager_tx_ops.rs:~553`, `~398`) все строки — один логический момент.

**Изменение:** читать clock ОДИН раз на batch; варьировать random-tail per-row.
Добавить/использовать `RecordId::from_ts(ts)` (layout неизменен: 8B-BE-ts +
8B-rand).
**Инварианты:** distinct random-tail на строку (уже 64 random бит) — сохранить
уникальность id внутри batch; байт-layout для sorted-range неизменен.
**Тесты:** batch-insert даёт уникальные, сортируемые id; ts общий на batch.
**Done:** один clock-read на batch; уникальность сохранена; @oracle зелёный.

### Этап 1.5 — L12: reuse scratch encode-buffer через batch
**Выигрыш:** −(N−1) малых alloc/batch на encode. **M.**

**Текущее (сверить):** `query_value_to_storage_bytes` (`write_exec.rs:~137` →
`messagepack.rs:~865`) делает `rmp_serde::to_vec` = свежий Vec на строку.
**Изменение:** один переиспользуемый `BytesMut`, serialize-each-row,
`split().freeze()` per row; `clear()` между строк — амортизирует allocator до
~1 региона/batch.
**Инварианты:** байт-идентичный вывод ОБЯЗАТЕЛЕН (`messagepack.rs:~842–864`);
`clear` (не повторное использование старых байт) между строками.
**Тесты:** encode батча байт-идентичен per-row encode (round-trip).
**Done:** один scratch-буфер на batch; вывод идентичен; @oracle зелёный.

---

## Волна 2 — структурные победы

### Этап 2.1 — L1: коалесцировать Drainer (1 transact на проход, не на entry)
**Выигрыш:** ~30× redb durable-drain (0.65k → ~40k/s); снимает
undrained-backpressure-тормоз → ack держит memory-скорость под нагрузкой.
**Главный рычаг. L. Durability-критичен. После Sonnet — независимый Opus-ревью.**
**Делать ПОСЛЕ L2** (этап 1.1).

**Текущее состояние (verified):**
- `crates/shamir-engine/src/tx/drainer.rs:131` — `for entry in &entries`: на КАЖДЫЙ
  entry вызывается `replay_v2_entry(entry, repo)` (146), затем per-entry
  `gate.mark_durable(v)` (168), A5-gate `interner_delta_safe_to_truncate` +
  `wal.commit(txn_id)` (181–206). Окно: `dur < v <= vis`, ascending (128).
- `crates/shamir-engine/src/tx/recovery.rs:321` `replay_v2_entry`: применяет
  interner-delta (322, ДО data), затем группирует ops `by_table` и
  `for (table_id, ops) in by_table` (402) вызывает
  `mvcc.write_committed_to_history(&ops, v)` (413) — ОДИН `history.transact` на
  (entry, table).
- Итог: проход из E entries × T таблиц = **E×T transact** (E×T redb write-txn).

**Целевая форма:** на проход — собрать ops ВСЕХ entry, сгруппировать по
`table_id`, выпустить **ОДНУ `history.transact` на таблицу** (version-keys
глобально уникальны `(key,version)` → cross-entry коллизий нет, LWW держится),
затем per-entry финализация.

**Изменение (двух-/трёхфазный drain):**
1. Новый метод `MvccStore::write_committed_batch_to_history(&self, pass: &[(u64 /*commit_version*/, &[KvOp])]) -> DbResult<()>`:
   построить ОДИН `history_ops` = для каждого `(v, ops)` все
   `KvOp::Set(encode_version_key(k, v), val|tombstone)` + (L2) ts-key на каждый
   уникальный `v`; ОДНА `self.history.transact(history_ops)`; затем per-(key,v)
   `publish_cell` (идемпотентно) и `gate.publish_committed_max(max_v)`. Это
   обобщение `write_committed_to_history` на много версий.
2. Drainer `drain_step` (drainer.rs:113): **Фаза A** — по всем entry окна в
   ascending-порядке применить interner-delta per-entry (как сейчас в
   `replay_v2_entry`, ДО data) и накопить `TMap<table_id, Vec<(v, Vec<KvOp>)>>`.
   **Фаза B** — per table ОДИН `write_committed_batch_to_history`. **Фаза C** —
   per-entry ascending: `mark_durable(v)` → A5-gate → `wal.commit(txn_id)` →
   maybe_crash-seam'ы — ТОЛЬКО для entry, чьи таблицы успешно записались; на
   первом сбое таблицы ОСТАНОВИТЬСЯ (contiguity), не финализируя выше.
3. `replay_v2_entry` (per-entry) ОСТАВИТЬ для cold-recovery
   (`recover_inflight_v2`) — там per-entry семантика нужна.

**Инварианты (durability — НЕ нарушать):**
- **Contiguity / stop-on-error:** если transact таблицы для версии v провалился —
  НЕ `mark_durable(v)`, НЕ `wal.commit`, и НЕ финализировать версии > v
  (watermark не должен перепрыгнуть дыру). Сохранить `break`-семантику
  drainer.rs:146–154.
- **Ascending version внутри группы** для LWW (sort на 128 уже есть; в Фазе B
  ops одной таблицы должны нести каждый свою v — ключ `(key,v)` уникален, порядок
  внутри transact не критичен, но НЕ терять ни одной версии).
- **interner-delta ДО data** (recovery.rs:322) — A4 keystone; Фаза A применяет
  delta до накопления ops.
- **A5 truncation-gate per entry** (interner-hwm) сохранить — только
  `wal.commit` отложить, data уже durable.
- **mark_durable ПОСЛЕ landing группы** (не до transact).
- **overlay-GC + WAL-truncate** (drainer.rs:218–271) — оставить как есть, они
  работают по post-pass `durable_watermark`.
- maybe_crash-seам'ы (`drain_replay` 164, `phase7` 215, `pre_truncate` 256,
  `post_truncate` 262) — сохранить смысл (recovery идемпотентно реиграет).

**Durability-чекпоинт (ОБЯЗАТЕЛЬНО):** `./scripts/test.sh @e2e --full` в цикле
(≥10 прогонов) под параллелизмом nextest; крэш-сюита (maybe_crash-точки) должна
сходиться. Любой `SLOW`/`TIMEOUT` — это deadlock/livelock баг, чинить, не
поднимать таймаут.
**Тесты:** drain прохода с E≥3 entry × T≥2 таблицы делает T transact (не E×T) —
проверить через тест-Store-счётчик transact; crash-recovery после Фазы B/C
сходится; backpressure (MAX_UNDRAINED_VERSIONS) перестаёт срабатывать под
write-нагрузкой (замерить drain-throughput до/после).
**Done:** transact/проход = O(таблиц), не O(entry×таблиц); @oracle + @e2e --full
зелёные в цикле; Opus-ревью подтвердил contiguity/durability.

### Этап 2.2 — L3: батчить MVCC read-path (get_many_bytes → 1 Store::get_many)
**Выигрыш:** ~порядок величины на index-чтении K строк. **L. Read-only (dur: none).**
Независим от L1 — можно ∥.

**Текущее (verified):** `crates/shamir-engine/src/table/table_manager_crud.rs`:
- `get_many_bytes` (466): при MvccStore — per-id цикл
  `for id in ids { out.push(mvcc.get_current(id.to_bytes()).await?) }` (471–475).
- `get_many` (422): тот же per-id `get_current` + decode (428–438).
- Иначе (no MVCC) — нативный `self.table.data_store().get_many(keys)` (478).
- `mvcc.get_current` (`mod.rs:751`): floor `gate.last_committed()`, `current_version`
  (cells.read, lock-free), overlay-probe (798), затем `history.get(version_key)` (801).
  Cold-cell (`cur_v==0`) → `seek_latest_version` (range-scan) + overlay (757–786).
  R3: `v>floor` → `get_at(floor)` (792–794).

**Изменение:** новый `MvccStore::get_current_many(keys: &[Bytes]) -> DbResult<Vec<Option<Bytes>>>`:
1. Per-key `current_version` (cells.read — lock-free, БЕЗ await). Партиционировать:
   **warm** (`cur_v>0`) vs **cold** (`cur_v==0`).
2. Warm: применить R3 floor-cap (если `v>floor` — отложить в slow-path `get_at`);
   построить version-keys; сперва overlay-probe (`overlay.get(key,v)`); собрать
   miss-set → ОДНА `self.history.get_many(version_keys_miss)`. Пустое значение
   (tombstone) → `None`.
3. Cold (меньшинство): per-id fallback на текущий `get_current` (cold-start path
   `seek_latest_version`).
4. Собрать результат В ПОРЯДКЕ входных `keys`.
Затем `get_many_bytes`/`get_many` вызывают `get_current_many` (get_many ещё
decode'ит в InnerValue).

**Инварианты (корректность):** сохранить **R3 floor-cap** + **overlay-vs-history
precedence** per-id (overlay-hit перекрывает history) + **cold-cell fallback**
(seek_latest). Не менять семантику tombstone (empty → None). Порядок результата =
порядок ids.
**Тесты:** get_many по смеси warm/cold/tombstone/absent даёт тот же результат, что
N× get_current; один `history.get_many` на warm-miss-set (тест-Store-счётчик);
floor-cap соблюдён (версия выше floor читается как snapshot).
**Done:** warm-batch делает 1 `Store::get_many`, не K× get; результат
байт-идентичен per-id циклу; @oracle зелёный; index-read бенч ↑.

---

## Волна 3 — durability-delicate

### Этап 3.1 — L14+L5: убрать мёртвый __data__ MemBuffer + read-through на __history__
**Выигрыш:** hot version-key read → ~memory-1.6µs на cache-hit; убирает per-commit
cache-mirror + drain_all-scan из дренажа. **L. Durability-чекпоинт.**

> **Этот этап требует discovery-под-шага.** Wiring repo→backend здесь НЕ сверен
> построчно (research-рефы: `repo_types.rs:~384`, `repo_instance.rs:~301–307`,
> `types.rs:~331` `fully_unwrap_store`, `storage_membuffer.rs`). Первый шаг агента
> — прочитать конструкцию репо и подтвердить: (a) `__data__` НЕ пишется для MVCC
> (write идёт через mvcc, `table_manager_crud.rs:190–208`); (b) `__history__`
> сейчас обёрнут MemBuffer, видящим только drain-transact.

**Резолюция конфликта L5↔L14** (взаимоисключающи на `__history__`): НЕ «снять
кэш», а **заменить случайно-унаследованный write-back MemBuffer на намеренный
read-through**:
- **L14:** `__data__`-MemBuffer для MVCC мёртв (data не пишется) → дать MvccStore
  unwrapped backend (`fully_unwrap_store`). `__info__` (indexes) — оставить wrapped.
- **L5:** обернуть `mvcc.history` в **bounded read-through MemBuffer**: version-keyed
  значения **immutable** (новый write = новая версия) → кэшу НЕ нужна
  инвалидация для point-read (read-fill on miss). Использовать MemBuffer
  (bounded/lazy), НЕ CachedStore (full-mirror-on-open неверен для большой history).

**Инварианты:** `resolve_read` идёт overlay→history.get — кэш помогает cold/GC'd
reads; tombstones как negative-cache; memory-budget bounded.
**Durability-чекпоинт:** flush-before-truncate (drainer.rs:251 `has_truncatable`/
`flush_all_history`) ОБЯЗАН дренировать MemBuffer ДО WAL-truncate; recovery
cold-path не должен buffer-and-lose при open. `@e2e --full` в цикле.
**Тесты:** hot version-key read бьёт кэш (счётчик backend-get не растёт); GC'нутая
версия не воскресает из кэша; crash перед truncate не теряет данные.
**Done:** __data__ unwrapped, __history__ read-through; cache-hit на hot read;
@e2e --full зелёный в цикле.

### Этап 3.2 — L6: O(1)/deferred vacuum для CurrentOnly
**Выигрыш:** убирает scan+remove на КАЖДОЙ write при дефолт-retention;
rewrite-heavy ~2-3×. **L. Durability LOW, но корр-инварианты sacred.**

**Текущее (verified):** `crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:43`
`vacuum_key` зовётся после каждой write (`mod.rs:572` set_versioned, `665–667`
set_versioned_many, delete_versioned, apply-пути). Early-return ТОЛЬКО если
`max_count.is_none() && max_age_secs.is_none()` (46–48). Иначе: `scan_prefix_stream`
(69) собирает все версии ключа, sort desc (87), вычисляет anchor (92–100), цикл
reclaim (111–156) с `history.remove(phys_key)` + `history.remove(ts_key)` (143–144)
+ overlay.remove под durable_watermark (153–155).

**Изменение:** для дефолтной CurrentOnly-retention (сверить дефолт в
`mvcc_store/retention.rs:~38`) при `active_snapshots_empty()`: caller знает
старую версию (old_v предыдущего current) → **targeted remove**
`history.remove(encode_version_key(key, old_v))` (+ ts_key) ВМЕСТО
scan_prefix+per-version-loop. Shortcut ТОЛЬКО при
`active_snapshots_empty() && CurrentOnly`; иначе — старый scan-путь (полная
корректность min_alive/anchor). Альтернатива: батчить removes в `transact` или
сдвинуть в GC-tick (deferred) — выбрать по результату 0.1.

**Инварианты (sacred — НЕ нарушать):** `min_alive` floor (версия `>= min_alive`
никогда не реклеймится); anchor (единственная наибольшая `< min_alive` при
живом снапшоте); current версия SACRED (`cur_v`); shortcut ТОЛЬКО без
live-snapshot (`active_snapshots_empty()`). overlay.remove только под
`durable_watermark` (не дропнуть недренированную версию — overlay её
единственная копия).
**Durability-чекпоинт:** vacuum best-effort, но крэш-сходимость и snapshot-чтение
старых версий — `@oracle --full` + крэш-сюита.
**Тесты:** CurrentOnly rewrite-same-key не делает prefix-scan (счётчик); версия
под живым снапшотом не реклеймится (sacred floor); anchor сохранён; append-only
(нет prior версии) — vacuum no-op без scan.
**Done:** дефолт-retention rewrite не сканирует префикс; sacred-инварианты целы;
@oracle --full зелёный.

---

## Волна 4 — backend-независимый floor

### Этап 4.1 — L10: атаковать 254µs engine-pipeline floor (a/b/c, ОТДЕЛЬНЫЕ коммиты)
**Выигрыш:** 2× к 254µs → удваивает single-insert по ВСЕМ backend (3.9k→~7.8k/s).
Наибольший backend-независимый выигрыш. **L. MED-HIGH корр-риск.** Делать
ИЗОЛИРОВАННО после стабилизации Волн 1–3; КАЖДАЯ под-часть — свой коммит.

**Текущее (research-рефы — сверить):** single tx_staged = 254µs vs 5.6µs raw (45×,
backend-независим). `commit.rs:~144/157/650`, `wal_group_commit.rs:~46/169–203`.

**Под-части:**
- **(a)** пропуск changefeed-projection без подписчиков (`commit.rs:~650`) —
  atomic re-check подписчиков. Корр: MED (atomic-проверка подписки).
- **(b)** fast-path group-commit append для uncontended single-committer — без
  `Arc<Waiter>`+`Notify` park (`wal_group_commit.rs:~169–203`). Корр: **MED-HIGH**
  — fast-path ОБЯЗАН сохранить non-yielding Mem-append атомарность
  (`wal_group_commit.rs:~46–55`, `wal_segment.rs:~50–58`); **код УЖЕ откатывал
  writer-task rewrite ровно по этой причине** — не повторять. Сперва замерить
  (0.1): действительно ли uncontended park материален в 254µs, или park дёшев при
  notify_one в том же poll. Если дёшев — (b) НЕ стоит риска, пропустить.
- **(c)** пропуск async `interner_overlay.scan_async` при пустом overlay
  (`commit.rs:~157`). Корр: LOW.

**Durability-чекпоинт:** (a)/(b) трогают commit/WAL — `@e2e --full` в цикле на
каждую под-часть отдельно.
**Done:** каждая под-часть — отдельный коммит с своим гейтом; 254µs floor ↓;
@e2e --full зелёный.

---

## Вне волн — L8: пункт назначения (НЕ в этой волне)

L8 в research помечен «последняя инстанция». Созерцание переформулировало его:
не «swap redb→sled» (sled — тоже общий LSM со своим overhead; sled-batch 19k <
redb-batch 41k), а **purpose-built immutable-log cold-tier (bitcask-формы:
append-сегмент + sparse-индекс)**, возможно слитый с WAL-segment-sealing
(drain = «запечатать сегмент», не «re-insert в дерево»). Делать ТОЛЬКО если
L1+L2 не дотягивают до целевого SLA. Требует полного e2e/crash + range-read
re-bench (redb может быть быстрее на range — пере-замерить ДО любого swap).
Это самостоятельный проект, не этап волны.

**L4/L7 (redb dup-check, table-handle) — НЕ берём:** только store_raw/system-table
выигрыш, engine hot-path их не трогает (transact, не insert). Cleanup по нужде.

---

## Граница — что НЕ трогать (из research §5)

- **memory 5.6µs / 178k-single на диске недостижим** без потери durability — floor
  при нулевом task-hop и нулевой txn. Цель — redb-batch 41k/s (но помни: 41k —
  это потолок B-tree-формы, не закон durability; настоящий floor — sequential
  append, выше; снимается только L8).
- **Per-commit fsync НЕ возвращать.** Дефолт `Buffered` (ack после page-cache,
  fsync deferred) — правильный. Power-loss durability — opt-in batch-fsync на RPC,
  не per-write.
- **block_in_place для sync-get НЕ делать** — starvation/deadlock на
  current-thread runtime; противоречит CLAUDE.md sanction для spawn_blocking.
- **Дублирующий async-backend слой НЕ строить** — Drainer это уже делает.
- **encode/intern НЕ оптимизировать как «проблему»** — low-single-% доля (body
  кодируется один раз, едет refcount-Bytes без memcpy; field-names амортизированы
  per-batch FxHashMap-кэшем; interner fast-path — lock-free DashMap &str get).

---

## Durability-чекпоинт — процедура (для L1, L6, L14+L5, L10 a/b)

```
# Цикл крэш/e2e под нагрузкой (≥10 проходов; SLOW/TIMEOUT = баг, не таймаут-апать):
for i in $(seq 1 10); do
  ./scripts/test.sh @e2e --full > run_$i.log 2>&1; rc=$?
  grep -aE "Summary|FAIL|TIMEOUT|SLOW|ABORT|panic|leak" run_$i.log; echo "exit=$rc"
done
```
- Крэш-сюита бьёт по maybe_crash-точкам (`drain_replay`, `phase7`, `pre_truncate`,
  `post_truncate`) — recovery должна идемпотентно сходиться (WAL = source of truth
  до успеха группы).
- WAL-truncate ТОЛЬКО после history-flush (I1/I2): порядок
  flush_all_history → truncate_below.
- Любой `SLOW`/`TIMEOUT` — deadlock/livelock/backpressure баг: воспроизвести под
  параллелизмом, найти корень (lock-order, bounded-channel без drain, guard
  через .await), ФИКСИТЬ. Никогда не поднимать таймаут.
