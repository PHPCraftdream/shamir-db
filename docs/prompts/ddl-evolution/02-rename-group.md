בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ②.1b — RENAME group

Кампания **② DDL-эволюция**, этап ②.1, под-этап **b — group**.
Источник: `docs/research/DDL-EVOLUTION-PLAN.md` §②.1b. Объём: S. Риск низкий.
Пакеты: `shamir-query-types`, `shamir-query-builder`, `shamir-db`,
`shamir-client-ts`.

## Развилка id-vs-name — РЕШЕНА (заземлено)
Группы **id-keyed**: `create_group(name) -> group_id` (u64, монотонный счётчик
`next_group_id`); `save_group(group_id, name, members)` хранит по gid; **члены и
ресурсы ссылаются на неизменный `group_id`** (`add_group_member(group_id,
user_id)`; `ResourceMeta.group = gid`). ⇒ RENAME group = **смена display-name в
записи по gid, БЕЗ rekey любых ссылок** (тривиальный случай). Файл:
`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:196-270`.

## Задача (одна строка)
DDL-операция переименования группы (по GroupRef id|name → новое имя) — паттерн в
шесть точек по образцу `DropGroup` / `RenameFunction`, + тесты.

## Заземление — паттерн (читай ПЕРВЫМ)
1. **Wire-op** — `crates/shamir-query-types/src/admin/access.rs`:
   - `enum GroupRef` :74 (`Id { id } | Name { name }`).
   - `DropGroupOp { drop_group: GroupRef, if_exists }` :135 — образец ссылки на
     группу по GroupRef.
   - Добавь рядом `RenameGroupOp { rename_group: GroupRef, to: String }`.
     Wire: `{ "rename_group": { "name": "devs" }, "to": "engineers" }` или
     `{ "rename_group": { "id": 3 }, "to": "engineers" }`.
   - Ре-экспорт через `crates/shamir-query-types/src/admin/mod.rs`. Добавь
     вариант `RenameGroup(RenameGroupOp)` в `batch/batch_op.rs` (enum BatchOp +
     Serialize-arm + Deserialize-дискриминатор `has("rename_group")` + admin
     classification match-arm — сверь все четыре места, как для соседних group-ops
     `CreateGroup`/`DropGroup`).

2. **Dispatch** — `crates/shamir-db/src/shamir_db/execute/admin_dispatch.rs`
   (рядом с `BatchOp::DropGroup(op) => self.handle_drop_group(op).await,` :68).
   Добавь `op @ BatchOp::RenameGroup(_) => self.handle_rename_group(op).await,`
   (или `BatchOp::RenameGroup(op) =>` если хэндлер принимает op напрямую — сверь
   стиль соседних group-хэндлеров).

3. **Handler** — `crates/shamir-db/src/shamir_db/execute/admin_access.rs`
   (рядом с `handle_create_group` / `handle_drop_group`). `handle_rename_group`:
   - **Auth — ТОЧНО зеркаль `handle_drop_group`/`handle_create_group`** (тот же
     admin-уровень; прочитай их и повтори).
   - Резолвь источник: `self.shamir.resolve_group_id(&op.rename_group)` (errors
     NotFound, если группа отсутствует) — учти `if_exists`-аналога НЕТ, источник
     обязан существовать.
   - Вызови engine `rename_group(&op.rename_group, &op.to)` (см. ниже).
   - Верни `admin_result(mpack!({ "renamed_group": …, "to": … }))` — сверь форму
     результата соседних group-хэндлеров (что они кладут: gid? name?).

4. **Engine-метод** — `access_control.rs` (рядом с `create_group` :196,
   `drop_group` :233, `resolve_group_id` :254). Новый
   `pub async fn rename_group(&self, group_ref: &GroupRef, to: &str) -> DbResult<()>`:
   - `gid = self.resolve_group_id(group_ref).await?` (NotFound если нет).
   - **Гард уникальности имени**: просканируй `system_store.load_groups()`; если
     существует группа с `name == to` И её `group_id != gid` → `DbError::KeyExists`
     (`format!("group '{}' already exists", to)`). (Renaming в собственное имя —
     no-op-OK либо тоже допустимо; сделай идемпотентно: если единственный матч
     это сам gid — не ошибка.)
   - `members = self.group_members(gid).await?` (`access_control.rs:273`).
   - `self.system_store.save_group(gid, to, &members).await?` — перезапись записи
     с новым именем, члены сохранены. (Сверь сигнатуру `save_group` в
     `system_store.rs`; create_group зовёт `save_group(group_id, name, &[])`.)
   - Опц. тонкий публичный хелпер если нужен симметрии — не обязателен.

5. **Rust-билдер** — `crates/shamir-query-builder/src/ddl/access_control.rs`
   (рядом с `drop_group` / `create_group`). Добавь
   `rename_group(group: impl Into<GroupRef>, to: impl Into<String>) -> …` по
   образцу `drop_group` (GroupRef-приём id|name). Если есть `Batch::*`-хелперы
   для group-ops в `batch/batch.rs` — добавь зеркальный. Импорты в шапку,
   one-file-one-export.

6. **TS-билдер** — `crates/shamir-client-ts/src/core/builders/ddl.ts` +
   `types/ddl.ts`: `RenameGroupOp { rename_group: GroupRef; to: string }`
   (сверь, как TS моделирует `GroupRef` для dropGroup) + функция
   `renameGroup(group, to)` по образцу `dropGroup`. Ре-экспорт через `index.ts`,
   как сделано для соседей.

## Тесты (обязательно)
- **Rust** (рядом с group-тестами — `grep -rn "create_group\|drop_group"
  crates/shamir-db/src --include=*.rs -l`, смотри `tests/`): покрой
  (1) create group + add members → rename by name → readback: новое имя
  резолвится в ТОТ ЖЕ gid, члены сохранены, старое имя больше не резолвится.
  (2) rename by id → ок. (3) гард: rename в занятое имя другой группы →
  `KeyExists`. (4) rename несуществующей → `NotFound`. (5) **ключевое — ссылки
  не сломаны**: ресурс с `group = gid` после rename всё ещё принадлежит группе
  (gid неизменен).
- **TS** — wire-shape в `ddl.test.ts`: `renameGroup({name:'devs'},'engineers')`
  → `{ rename_group:{name:'devs'}, to:'engineers' }`.

## Гейт (прогони сам, всё зелёное — НЕ коммить)
- `./scripts/test.sh -p shamir-query-types -p shamir-query-builder -p shamir-db`
  (+ `-- rename_group` для фокуса).
- `cargo fmt -p shamir-query-types -p shamir-query-builder -p shamir-db -- --check`
  + `cargo clippy --workspace --all-targets -- -D warnings` (workspace — новый
  enum-вариант обязан быть покрыт во всех match, вкл. `shamir-engine`
  `query/auth/session.rs` auth-cache классификацию — добавь `RenameGroup` в
  нужный arm по образцу соседних admin-ops).
- TS: `cd crates/shamir-client-ts && npx vitest run ddl && npx tsc --noEmit`
  (не вноси НОВЫХ tsc-ошибок сверх 4 pre-existing).

## ЖЁСТКИЕ ПРАВИЛА
- ⛔ НЕ используй инструмент agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER `git reset`/`checkout`/`clean`/`stash`/`restore`/`rm` или любую
  мутирующую git-команду (в т.ч. НЕ удаляй `run.log` и прочие отслеживаемые
  файлы). Только редактируй; коммитит оркестратор.
- Surgical, аддитивно, по образцу соседей. Билдер-only, без raw serde_json::Value.
  Тесты — только через `./scripts/test.sh`.
- Заверши финальным текстом: изменённые файлы (file:line) + вывод гейта.
