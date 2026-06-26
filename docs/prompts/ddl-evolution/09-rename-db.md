בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ②.1d — RENAME db (каталог-rekey, без переноса файлов)

Кампания **② DDL-эволюция**, этап ②.1d. Источник: `DDL-EVOLUTION-PLAN.md §②.1d`
(читай блок «✅ РЕШЕНО (②.1d-a) — ВАРИАНТ (γ)» ПЕРВЫМ). Объём: M. Риск средний
(**полнота rekey** — главная точка). Пакеты: `shamir-query-types`,
`shamir-query-builder`, `shamir-db`, `shamir-client-ts`.

## Дизайн ②.1d-a (РЕШЕНО) — кратко
RENAME db = **чистый каталог-rekey, БЕЗ fs-move / handle-drain / reopen / crash-
recovery**. Почему: физический путь repo **уже декуплён** от имени db — boot
re-attach берёт путь из persisted `path`-поля записи repo (`core.rs:154-164`,
`record["path"]`), НЕ из имени db. Поэтому rename только переписывает каталожные
строки + in-memory map; открытые handle и `path`-поля НЕ трогаются. Структурно —
**твин RENAME role** (②.1c). Главная точка риска — **ПОЛНОТА**: пропуск каталога с
`db_name` осиротит метаданные.

## Задача (одна строка)
DDL-операция переименования db — паттерн в шесть точек + полный каталог-rekey
(databases/repositories/tables/access-meta + in-memory `dbs`) + тесты.

## Заземление — ШАБЛОН (читай ПЕРВЫМ)
**`crates/shamir-db/src/shamir_db/shamir_db/db_management.rs` `rename_repo_as`
(:184-330)** — точный структурный образец: write-new-before-remove-old
(crash-safe ordering — крэш оставляет новую строку резолвимой), сохранение
`engine`/`path`/`ResourceMeta::from_record` через re-key, rekey repo-записи +
дочерних table-записей. `rename_db` ОБОБЩАЕТ это с (db,repo) на db_name по ВСЕМ
каталогам.

## Точки паттерна
1. **Wire-op** — `crates/shamir-query-types/src/admin/types/repo_ops.rs`
   (рядом с `RenameRepoOp`): `RenameDbOp { rename_db: String, to: String }`.
   Wire: `{ "rename_db": "old", "to": "new" }`. Ре-экспорт `admin/mod.rs` +
   вариант `BatchOp::RenameDb` в `batch/batch_op.rs` (вариант + Serialize +
   Deserialize `has("rename_db")` + admin-classify arm — сверь все 4 места, как
   `RenameRepo`).
2. **Dispatch** — `execute/admin_dispatch.rs` (рядом с
   `BatchOp::RenameRepo(op) => self.handle_rename_repo(op).await,` :31).
   `op @ BatchOp::RenameDb(_) => self.handle_rename_db(op).await,`.
3. **Handler** — `crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs`
   (рядом с `handle_rename_repo`). `handle_rename_db`:
   - Валидируй имя назначения (`validate_name_component`).
   - **Auth**: зеркаль `handle_rename_repo`/`handle_drop_db` — `Action::Write` (или
     как drop_db авторизует db-level; прочитай и повтори) на
     `ResourcePath::Database { db: from }`.
   - Вызови engine `rename_db_as(&op.rename_db, &op.to, self.actor.clone())`.
   - Верни `admin_result(mpack!({ "renamed_db": from, "to": to }))`.
4. **Engine-метод** — `db_management.rs` (рядом с `rename_repo_as`, `remove_db`
   :49, `create_db_as` :16). `pub async fn rename_db_as(&self, from: &str, to:
   &str, actor: Actor) -> DbResult<()>` + тонкий `rename_db(from,to)` →
   `Actor::System`:
   - **Гарды**: `from != SYSTEM_DB_NAME` (`remove_db:50` защищает его — зеркаль,
     верни `DbError`); source существует (`has_db(from)` / `load_database(from)`);
     назначение свободно (`!has_db(to)` И `load_database(to).is_none()`).
   - **(1) In-memory rekey**: вынь `DbInstance` из `dbs` по `from`, вставь под
     `to` (`dbs.remove(from)` → `dbs.insert(to, inst)`). Открытые repo-handle
     едут с инстансом — НЕ трогаем (их `path` неизменен). Если у `DbInstance`
     есть внутреннее поле `name` — обнови (сверь, как `rename_repo` обновляет
     repo `name`).
   - **(2) databases-registry**: `load_database(from)` → склонируй запись,
     обнови поле `"name"` на `to`, `save_database(to, &rec, &ResourceMeta::
     from_record(&rec))` ПЕРЕД `remove_database(from)` (write-before-remove).
   - **(3) rekey дочерних каталогов по `db_name`** — для КАЖДОГО каталога с
     колонкой `db_name`: `load_*` → отбери строки `db_name==from` → перепиши
     поле `db_name`→`to`, сохрани новую (preserve все прочие поля + meta через
     `from_record`) ПЕРЕД удалением старой:
     - **repositories**: `load_repositories()` → для каждой с `db_name==from`:
       `save_repository(to, repo_name, engine, path.as_deref(), &meta)` +
       `remove_repository(from, repo_name)`. **`path` НЕ менять** (физ-локация та
       же — ключевой инвариант (γ)).
     - **tables**: `load_tables()` → для каждой с `db_name==from`:
       `save_table(to, repo_name, table_name, enable_indexes, &meta)` +
       `remove_table(from, repo_name, table_name)`.
     - **⚠ ПОЛНОТА — обязателен grep-аудит:** `grep -nE "db_name: &str|\"db_name\""
       crates/shamir-db/src/shamir_db/system_store.rs` — перечисли ВСЕ
       `save/load/remove`-методы и `*_meta`-записи, несущие `db_name`. Rekey
       КАЖДЫЙ найденный каталог (включая `save_database_meta`/`save_repository_meta`/
       `save_table_meta`-access-meta, если они хранят db_name отдельно от строки).
       Если schema/retention/buffer/validator-bindings/index-meta несут `db_name`
       или (db,repo,table)-ключ — rekey и их. Ничего не пропусти: пропуск =
       осиротевшие метаданные после rename.

5. **Rust-билдер** — `crates/shamir-query-builder/src/ddl/` (рядом с
   `rename_repo` — найди файл: `rename_repo.rs`/`res.rs`). Добавь
   `rename_db(from, to) -> BatchOp` по образцу `rename_repo`. one-file-one-export;
   `Batch::rename_db` helper если есть зеркальные.
6. **TS-билдер** — найди, где `renameRepo`/`RenameRepoOp` (grep
   `renameRepo|RenameRepoOp` в `client-ts/src/core`). Добавь `RenameDbOp` +
   `renameDb(from, to)` по ФАКТИЧЕСКОМУ образцу соседа, тот же union/файл.
   Ре-экспорт как сосед.
+ shamir-engine `query/auth/session.rs` auth-cache match-arm + при нужде
  `query/admin/mod.rs` re-export для `RenameDbOp` (workspace-clippy поймает иначе,
  как было для RenameGroup/RenameRole).

## Тесты (обязательно — ПОЛНОТА критична)
Rust e2e/integration (рядом с rename-тестами — `grep -rln "rename_repo\|rename_db"
crates/shamir-db --include=*.rs | grep test`; смотри `tests/ddl_wire_e2e/`):
- **Главный (полнота):** создать db с repo + table + index + schema + явным
  owner/group/mode (access-meta) → `rename_db` → readback ПОД НОВЫМ именем: все
  переехали (repo/table/schema/index есть под `new`, под `old` нет), данные в
  таблице целы (handle не тронут), ACL (owner/mode) сохранён.
- **durable reboot** (если есть reopen-харнес, как `wire_created_repo_is_durable_across_reopen`):
  rename → reopen → db поднимается под `new` именем у того же физ-каталога, данные
  целы.
- **гарды:** rename несуществующей → `NotFound`; rename в занятое имя →
  `KeyExists`/typed; rename SYSTEM_DB → отказ.
- **данные целы:** insert в таблицу → rename db → read той же таблицы под новым
  именем → запись на месте (подтверждает, что handle/path не тронуты).
- TS wire-shape: `renameDb('old','new')` → `{ rename_db:'old', to:'new' }`.

## Гейт (прогони сам, всё зелёное — НЕ коммить)
- `./scripts/test.sh -p shamir-query-types -p shamir-query-builder -p shamir-db
  --full -- rename_db` (e2e; НЕ параллель rust `--full` с другими e2e — Windows
  file-lock на `shamir-server.exe`).
- `cargo fmt -p shamir-query-types -p shamir-query-builder -p shamir-db
  -p shamir-engine -- --check` + `cargo clippy --workspace --all-targets -- -D warnings`.
- TS: `cd crates/shamir-client-ts && npx vitest run <ddl|admin> && npx tsc --noEmit`
  (не вноси НОВЫХ tsc-ошибок сверх 4 pre-existing).

## ЖЁСТКИЕ ПРАВИЛА
- ⛔ НЕ используй инструмент agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER `git reset`/`checkout`/`clean`/`stash`/`restore`/`rm` или мутирующую
  git-команду (НЕ удаляй `run.log`/отслеживаемые; scratch-логи — в /tmp). НЕ коммить.
- ⛔ НЕ переноси/переименовывай файлы на диске, НЕ закрывай/переоткрывай сторы,
  НЕ трогай `path`-поля. Только каталог-rekey + in-memory map. Физ-локация
  неизменна (вариант γ).
- Surgical, по образцу `rename_repo_as`. one-file-one-export; импорты в шапку.
  Билдер-only, без raw `serde_json::Value`. Тесты — только через `./scripts/test.sh`.
- Заверши финальным текстом: изменённые файлы (file:line) + **полный список
  каталогов, которые rekey-ишь** (доказательство полноты по grep-аудиту) + вывод гейта.
