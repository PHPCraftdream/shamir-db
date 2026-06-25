בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# E.4-followon — поэтапный план (RenameRepo / RenameIndex + populated-table MVCC-overlay миграция)

Follow-on кампании Phase E (после E.4 Object 1 `RENAME TABLE`, коммит `a7dcda5`).
Декомпозиция исходной задачи #250 на три leaf-фазы **F.1 → F.2 → F.3**
(эскалация риска). Каждая фаза — отдельный коммит, с тестовой лестницей
**вплоть до e2e TS**.

> Реализацию начинать по слову пользователя. Брифы — prompt-first в
> `docs/prompts/ddl-lifecycle/` под git ДО старта делегированных стадий.

---

## Контекст и барьер (заземлено чтением кода)

- **Барьер** (`crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:240`):
  `rename_table_as` отказывает при `mvcc.cell_count() > 0` — rename таблицы с
  данными запрещён, чтобы не потерять их молча.
- **Где живут данные** (`crates/shamir-tx/src/mvcc_store/mod.rs`): `MvccStore` =
  `cells` (key→версия, in-memory, rebuildable; :117), `overlay`
  (`VersionedOverlay`, не-дренированный хвост `(durable_watermark,
  visibility_watermark]`; :139), `history` (durable Store, но вакуумится при
  `Retention::current_only` — остаётся только текущая версия).
- **Почему store-level copy не переносит данные**: `flush_history()` (:353) —
  это `self.history.flush()` (сброс буферов Store), **НЕ** дренаж overlay→history.
  Материализацию overlay→history делает `write_committed_to_history`
  (`mvcc_store/mvcc_history.rs:457`) / фоновый Drainer. `rename_table_stores`
  (`crates/shamir-engine/src/repo/repo_instance.rs:380`) зовёт `flush_history`,
  поэтому `__history__<from>` пуст → copy переносит пустоту.
- **Модель для расширения**: `handle_rename_table` (`admin_table_index.rs`) +
  `rename_table_as` (`table_management.rs:178`) + `rename_table_stores`
  (`repo_instance.rs:380`) + `Repo::copy_store` (`storage/types.rs`).
  Вторичный образец — `handle_rename_function`/`handle_rename_validator`.

---

## Общие принципы (все фазы)

- **Prompt-first**: бриф в `docs/prompts/ddl-lifecycle/` под git до старта;
  commit-per-object; делегированным агентам git-мутации запрещены.
- **Гейт каждой фазы**: `fmt --all --check` + `clippy --workspace --all-targets
  -D warnings` + `./scripts/test.sh` (нужные крейты) + **e2e TS** против
  свежесобранного release-сервера (`cargo build --release -p shamir-server`,
  бинарь в `CARGO_TARGET_DIR=.cargo-target/release`; ~15-20 мин — закладывать).
- **Дисциплина**: builder-only (`serde_json::Value` запрещён), `#[serde(default)]`
  на новых полях, scc `len()` запрещён (mirror/annotate), один-файл-один-export,
  тесты только через `./scripts/test.sh`, вывод тестов в файл (не inline-grep).
- **Тестовая лестница на фазу**: (1) Rust unit (где есть чистая логика) →
  (2) Rust integration через `db.execute` (серверный путь) → (3) TS wire-shape
  unit (билдер) → (4) **TS e2e** через сервер.

---

## Phase F.1 — RenameIndex  (самый чистый, без MVCC; быстрая победа)

**Цель**: переименование индекса в рамках таблицы без потери данных индекса.

**Подход** (заземлить `table/table_manager_index_mgmt.rs`): индексы в
`index_manager` keyed по имени → interned id. RenameIndex = rekey записи
(preserve id, как RenameTable). Учесть виды: regular / unique / sorted / index2
(как перечислял E.2 cascade). Если rekey сложен — fallback drop+rebuild под новым
именем (индекс — производные данные), но rekey предпочтительнее.

**Срез**:
1. `RenameIndexOp { rename_index, to, table, repo }` (`admin/types/index_ops.rs`).
2. `BatchOp::RenameIndex` (`batch_op.rs`) + dispatch (`admin_dispatch.rs`) +
   permission routing (`session.rs`, `Action::Write` на таблицу) +
   `handle_rename_index` (`admin_table_index.rs`, рядом с `handle_rename_table`).
3. Билдеры: Rust `ddl/rename_index.rs` + `batch.rs`; TS `ddl.ts` + тип.

**Тесты**:
- *Rust integration* (`tests/rename_index_e2e.rs`): таблица+данные+индекс →
  запрос использует индекс (можно проверить через EXPLAIN из E.7:
  `plan_type=IndexScan`, `index_used`) → rename → запрос всё ещё использует
  индекс под новым именем; старое имя индекса не резолвится; refuse
  destination-exists.
- *TS wire-unit* (`ddl.test.ts`): `renameIndex` сериализуется.
- *TS e2e* (`__tests__/e2e-rename-index.test.ts`): createIndex → insert → rename
  → query, assert результаты.

**Объём**: S-M. **Коммит**: `feat(ddl): E.4-followon — RENAME INDEX`.

---

## Phase F.2 — populated-table MVCC-overlay миграция  (ядро, снятие барьера)

**Цель**: снять guard `cell_count>0`, чтобы RENAME TABLE работал для таблиц
**с данными**.

**Шаг 0 — исследование (обязательно перед кодом), выбор стратегии**:
- **Стратегия A (рекомендуется) — force-drain → copy → cold-start**: перед
  `copy_store` форсировать материализацию overlay→history (через
  `write_committed_to_history` / синхронный прогон Drainer для таблицы), чтобы
  `__history__<from>` содержал текущую версию каждого живого ключа. `copy_store`
  переносит history; новый `MvccStore` при cold-start восстанавливает `cells`
  range-scan'ом из скопированного history (`mod.rs:117`). Под `current_only`
  после дренажа в history остаётся текущая версия → данные целы. **Плюс**:
  переиспользует cold-start-машинерию, не трогает внутренности overlay.
  **Минус**: нужен надёжный синхронный «drain this table now» примитив —
  найти/собрать.
- **Стратегия B — прямой перенос состояния**: сконструировать новый `MvccStore`
  над скопированным history и перенести `cells`+`overlay` напрямую. Сложнее
  (watermark-инварианты), вторжение в `shamir-tx` API. Только если A нереализуема.

**Шаг 1 — реализация (Стратегия A)**:
- Заменить `flush_history()` в `rename_table_stores` (`repo_instance.rs:380`) на
  реальный синхронный дренаж таблицы перед `copy_store` (новый
  `MvccStore::drain_to_history()` или прогон overlay-версий через
  `write_committed_to_history` + advance watermark + `gc_overlay_to`).
- Убрать guard `cell_count>0` в `rename_table_as` (`table_management.rs:240`).
  Оставить guard'ы schema-bearing и destination-exists (они про другое).
- **schema-bearing** (под-вопрос): если хотим rename таблиц со схемой — отдельный
  sub-шаг (мигрировать auto-bound schema-validator под новый путь; образец
  RenameValidator). Иначе оставить guard'ом с явным follow-on.

**Тесты** (durability-критичная зона — лестница глубже):
- *Rust unit* (`shamir-tx`): `drain_to_history` — overlay после дренажа пуст,
  history содержит текущие версии; повторный дренаж идемпотентен.
- *Rust integration* (`rename_table_e2e.rs`: переписать
  `rename_table_refuses_populated` → `rename_table_migrates_populated`): таблица
  + N строк → rename → старое имя пусто, **новое резолвится со ВСЕМИ данными**
  (insert/update/read-back), индексы работают; дописать строку в новую таблицу и
  прочитать (overlay новой таблицы жив).
- *Rust integration — durability*: после rename данные переживают «перечитывание»
  таблицы (drop in-memory TableManager → cold-start из history); сверить с
  паттерном `tests/crash_recovery.rs`.
- *TS e2e* (`__tests__/e2e-rename-table.test.ts`): createTable → insert
  несколько строк → rename → query новой таблицы возвращает все строки; старое
  имя не резолвится; insert/update в новую таблицу работает.

**Объём**: **L** (ядро, durability-критично, вторжение в `shamir-tx`).
**Коммит**: `feat(engine,tx): E.4-followon — populated-table rename via overlay drain-migration`.

---

## Phase F.3 — RenameRepo  (опирается на F.2)

**Цель**: переименование репозитория с сохранением его таблиц и их данных.

**Шаг 0 — исследование**: проследить namespacing стора репозитория на уровне
`DbInstance` (`db_instance.rs`) — стора таблиц физически namespaced по имени репо
(тогда rename repo = N×`rename_table_stores` с drain) или репо — логический ключ
в реестре (тогда rekey реестра + reverse-index, как RenameTable). От ответа
зависит объём.

**Подход**: rekey записи репо (system_store + in-memory реестр в DbInstance +
reverse-index), preserve id. Для populated-таблиц внутри — переиспользовать
drain-миграцию из F.2 (почему F.3 после F.2). Если стора namespaced по репо —
пройтись `copy_store`+drain по каждой таблице и переключить config.

**Срез**: `RenameRepoOp` (`admin/types/repo_ops.rs`) + dispatch + permission
(`Action::Write` на репо/db) + `handle_rename_repo` (`admin_db_repo.rs`, рядом с
drop_repo) + билдеры Rust+TS.

**Тесты**:
- *Rust integration* (`tests/rename_repo_e2e.rs`): репо с таблицей+данными+индексом
  → rename repo → старое имя репо не резолвится, новое резолвится, таблицы и их
  данные целы, индексы работают.
- *TS wire-unit* (`ddl.test.ts`) + *TS e2e* (`__tests__/e2e-rename-repo.test.ts`):
  createRepo(tables) → insert → rename repo → query через новый репо возвращает
  данные.

**Объём**: M (после F.2), либо L если стора namespaced по репо.
**Коммит**: `feat(ddl): E.4-followon — RENAME REPO`.

---

## Тестовая инфраструктура (e2e TS на всех фазах)
- Образец — `crates/shamir-client-ts/src/__tests__/e2e-{fts,vector,call}.test.ts`
  + `e2e-harness.ts` (startServer/connectAdmin/setupDb). Сигнатуры:
  `createIndex(name, table, [['field']], opts)`, `dropTable(signer, db, repo,
  table)` (HMAC).
- Каждая фаза добавляет новые ops на wire → пересборка release-сервера ПЕРЕД
  e2e обязательна.

## Риски и решения
| Риск | Митигация |
|---|---|
| F.2 drain-примитив отсутствует/небезопасен | Шаг 0 решает A vs B; если A нереализуема — guard остаётся, документируем как глубокий follow-on |
| F.2 потеря данных при rename | Durability-тест (cold-start re-read) обязателен ДО снятия guard |
| F.3 объём (стора namespaced по репо) | Шаг 0 определяет; если N×copy — оценка L, можно сузить до «repo без populated-таблиц» с guard (как E.4) |
| schema-bearing таблицы | Отдельный sub-шаг (миграция schema-validator) или оставить guard'ом |

## Последовательность и зависимости
- **F.1** независима (быстрая победа, набивает DTO/dispatch/builder/e2e-мускул).
- **F.2** независима от F.1 (ядро; де-риск рано).
- **F.3** blockedBy **F.2** (переиспользует drain-миграцию для populated-таблиц).
- Рекомендация: **F.1 → F.2 → F.3**.

> Заменяет исходную umbrella-задачу #250 тремя leaf-задачами F.1/F.2/F.3.
