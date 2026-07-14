בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ②.1c — RENAME role

Кампания **② DDL-эволюция**, этап ②.1, под-этап **c — role**.
Источник: `docs/dev-artifacts/research/DDL-EVOLUTION-PLAN.md` §②.1c. Объём: M. Риск средний
(name-keyed → rekey ссылок). Пакеты: `shamir-query-types`,
`shamir-query-builder`, `shamir-db`, `shamir-client-ts`.

## Развилка id-vs-name — РЕШЕНА (заземлено)
Роли **name-keyed** (в отличие от групп): `Role { name, permissions }`
(`crates/shamir-query-types/src/auth/types.rs:140`); `User { roles: Vec<String> }`
(`:147-152`) ссылается на роли **по имени**; `GrantRoleOp { grant_role: String,
user: String }` (`:249`). ⇒ RENAME role = **re-key записи роли + rekey всех
ссылок** (у каждого user в `roles`-списке заменить старое имя на новое). Логика
ролей живёт В ХЭНДЛЕРЕ через `roles_table()` / `users_table()` (НЕ в engine-API,
в отличие от group/folder) — см. `handle_create_role`/`handle_drop_role`/
`handle_grant_role` в `crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs:215-456`.

## Задача (одна строка)
DDL-операция переименования роли (re-key roles_table + rekey `roles` во всех
users) — паттерн в шесть точек + тесты.

## Заземление — паттерн (читай ПЕРВЫМ)
Прочитай ЦЕЛИКОМ `admin_users_roles.rs:215-456`
(`handle_create_role`/`handle_drop_role`/`handle_grant_role`) — твой
`handle_rename_role` комбинирует их механику.

1. **Wire-op** — `crates/shamir-query-types/src/auth/types.rs`
   (рядом с `CreateRoleOp` :229, `DropRoleOp` :236, `GrantRoleOp` :249):
   `RenameRoleOp { rename_role: String, to: String }` (source name → dest name).
   Wire: `{ "rename_role": "viewer", "to": "reader" }`. Ре-экспорт через
   `crates/shamir-query-types/src/auth/mod.rs`. Вариант
   `BatchOp::RenameRole(RenameRoleOp)` в `batch/batch_op.rs` (вариант + Serialize
   + Deserialize `has("rename_role")` + admin-classify arm — сверь все 4 места,
   как для `CreateRole`/`DropRole`; импорт типа из `crate::auth::{…}`).

2. **Dispatch** — `crates/shamir-db/src/shamir_db/execute/admin_dispatch.rs`
   (рядом с `BatchOp::CreateRole(op) => self.handle_create_role(op).await,`
   :52 и `DropRole` :53). Добавь
   `BatchOp::RenameRole(op) => self.handle_rename_role(op).await,`.

3. **Handler** — `crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs`
   (рядом с `handle_create_role` / `handle_drop_role` / `handle_grant_role`).
   `handle_rename_role(&self, op: &crate::query::auth::RenameRoleOp)`:
   - **Auth**: `Manage` on `ResourcePath::Root` (зеркаль `handle_create_role`:233).
   - **Guard source-exists**: прочитай `roles_table` фильтром `name == from`
     (как `handle_drop_role`:301-316 if_exists-проба); пусто → typed error
     `not_found` (`format!("Role '{}' not found", from)`).
   - **Guard dest-free**: прочитай `roles_table` фильтром `name == to`; не пусто
     → typed error `already_exists` (`format!("Role '{}' already exists", to)`).
     (Renaming в своё имя — допусти как no-op-OK, если хочешь, но проще
     требовать from != to.)
   - **Re-key записи роли**: возьми существующую запись роли (permissions
     сохрани!), запиши копию под `name == to` через
     `set_via_implicit_tx` + `SetOp{ set:"roles", key:{name:to}, value:<role с
     обновлённым name=to> }` (зеркаль `handle_create_role`:241-263), затем удали
     старую запись `name == from` через `run_implicit_batch_tx` +
     `execute_delete_tx` (зеркаль `handle_drop_role`:330-357). Не забудь
     `table.interner().persist()`.
   - **Rekey ссылок в users**: прочитай ВСЕ users (`ReadQuery::new("users")` без
     фильтра) из `users_table()`; для каждого, чей `roles`-список содержит `from`,
     замени `from`→`to` в списке и перезапиши запись (зеркаль мутацию
     `handle_grant_role`:424-451: `user_qv` → mutate `roles` List →
     `set_via_implicit_tx`). Возьми per-user lock как grant (:386-392
     `admin_user_locks().entry(user).or_insert…`) для каждого мутируемого user.
     Только пользователи, реально содержащие `from`, переписываются.
   - Верни `admin_result(mpack!({ "renamed_role": <from>, "to": <to> }))`.

4. **Engine-метод** — НЕ требуется (роли table-backed, логика в handler). Если
   для чистоты выносишь helper — допустимо, но не обязательно.

5. **Rust-билдер** — `crates/shamir-query-builder/src/ddl/` (где живут
   role-билдеры — найди `create_role`/`drop_role`: `grep -rn "fn create_role\|fn
   drop_role" crates/shamir-query-builder/src`). Добавь
   `rename_role(from: impl Into<String>, to: impl Into<String>) -> BatchOp` по
   образцу `drop_role`. + `Batch::rename_role` helper в `batch/batch.rs` если
   есть зеркальные. Импорты в шапку.

6. **TS-билдер** — найди, где живут `createRole`/`dropRole` в
   `crates/shamir-client-ts/src/core` (вероятно `admin.*`, как группы — НЕ
   предполагай `ddl.*`, проверь grep'ом `grep -rn "dropRole\|DropRoleOp"
   crates/shamir-client-ts/src`). Добавь `RenameRoleOp { rename_role: string; to:
   string }` + `renameRole(from, to)` ПО ФАКТИЧЕСКОМУ образцу соседа `dropRole`
   (тот же файл, тот же union — `AdminOp`/`AuthOp`/что там). Ре-экспорт как сосед.

## Тесты (обязательно)
- **Rust** (рядом с role-тестами — `grep -rln "create_role\|grant_role"
  crates/shamir-db --include=*.rs | grep test`): покрой
  (1) create role + grant юзеру → rename → readback: новое имя в roles_table,
  старое нет; **у юзера в `roles` теперь новое имя, старого нет** (ключевое —
  rekey ссылок). (2) rename роли, которую никто не грантил → ок, users не
  тронуты. (3) гард: rename в занятое имя → `already_exists`. (4) rename
  несуществующей → `not_found`. (5) несколько юзеров с ролью → все
  перексрешены.
- **TS** — wire-shape: `renameRole('viewer','reader')` →
  `{ rename_role:'viewer', to:'reader' }`.

## Гейт (прогони сам, всё зелёное — НЕ коммить)
- `./scripts/test.sh -p shamir-query-types -p shamir-query-builder -p shamir-db
  -- rename_role` (+ при e2e не параллель rust `--full` с e2e — Windows
  file-lock на shamir-server.exe).
- `cargo fmt -p shamir-query-types -p shamir-query-builder -p shamir-db
  -p shamir-engine -- --check` + `cargo clippy --workspace --all-targets
  -- -D warnings` (workspace — новый enum-вариант обязан быть покрыт во всех
  match, вкл. `shamir-engine` `query/auth/session.rs` auth-cache + при нужде
  `query/admin/mod.rs` re-export, как было для RenameGroup).
- TS: `cd crates/shamir-client-ts && npx vitest run <ddl|admin> && npx tsc
  --noEmit` (не вноси НОВЫХ tsc-ошибок сверх 4 pre-existing).

## ЖЁСТКИЕ ПРАВИЛА
- ⛔ НЕ используй инструмент agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER `git reset`/`checkout`/`clean`/`stash`/`restore`/`rm` или любую
  мутирующую git-команду (в т.ч. НЕ удаляй `run.log` и прочие отслеживаемые
  файлы; scratch-логи не плоди в корне/крейте — пиши в /tmp). Только редактируй;
  коммитит оркестратор.
- Surgical, аддитивно, по ФАКТИЧЕСКОМУ образцу соседей. one-file-one-export;
  импорты — в шапку. Билдер-only, без raw serde_json::Value. Тесты — только через
  `./scripts/test.sh`.
- Заверши финальным текстом: изменённые файлы (file:line) + вывод гейта.
