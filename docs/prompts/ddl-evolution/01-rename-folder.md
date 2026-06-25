בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ②.1a — RENAME function-folder

Кампания **② DDL-эволюция**, этап ②.1 (RENAME-остаток), под-этап **a — folder**.
Источник: `docs/research/DDL-EVOLUTION-PLAN.md` §②.1. Объём: S. Риск низкий
(ближайший к готовому `rename_function`). Пакеты: `shamir-query-types`,
`shamir-query-builder`, `shamir-db`, `shamir-client-ts`.

## Задача (одна строка)
Добавить DDL-операцию **переименования function-folder** — полный паттерн в
шесть точек (wire-op → dispatch → handler → engine-метод → Rust-билдер →
TS-билдер) + тесты, по образцу пяти готовых RENAME-ops.

## Заземление — готовый паттерн (читай эти файлы ПЕРВЫМ, по порядку)
Пять RENAME-ops уже реализованы; folder — шестой по тому же шаблону.

1. **Wire-op DTO** — `crates/shamir-query-types/src/admin/types/function_ops.rs`
   содержит `RenameFunctionOp { rename_function: String, to: String }` и
   `CreateFunctionFolderOp { create_function_folder: Vec<String> }`. Добавь
   рядом `RenameFunctionFolderOp`. Поля — путь источника и путь назначения,
   оба как `Vec<String>` сегментов (folder адресуется сегментами, как в
   `CreateFunctionFolderOp`). Wire-shape:
   `{ "rename_function_folder": ["a","b"], "to": ["a","c"] }`.
   - Зарегистрируй тип в `crates/shamir-query-types/src/admin/types/mod.rs` и
     ре-экспортни через `crates/shamir-query-types/src/admin/mod.rs` (сверь, как
     это сделано для `RenameFunctionOp` / `CreateFunctionFolderOp`).
   - Добавь вариант `RenameFunctionFolder(RenameFunctionFolderOp)` в
     `crates/shamir-query-types/src/batch/batch_op.rs` (enum `BatchOp`) — сверь
     соседние `RenameFunction` / `CreateFunctionFolder` варианты.

2. **Dispatch** — `crates/shamir-db/src/shamir_db/execute/admin_dispatch.rs:77`
   (рядом с `op @ BatchOp::CreateFunctionFolder(_) => …`). Добавь
   `op @ BatchOp::RenameFunctionFolder(_) => self.handle_rename_function_folder(op).await,`.

3. **Handler** — `crates/shamir-db/src/shamir_db/execute/admin_function.rs`
   (рядом с `handle_rename_function` :150 и `handle_create_function_folder`
   :191). Новый `handle_rename_function_folder`:
   - Валидируй сегменты пути (`validate_name_component`, как в
     `handle_create_function_folder`), оба пути непустые.
   - Auth: `Action::Write` на `ResourcePath::FunctionFolder { path: <from
     segments> }` (folder сам — ресурс; сверь, как `handle_rename_function`
     авторизует `ResourcePath::Function`). Родитель назначения должен быть
     доступен на `Action::Create`, как в `handle_create_function_folder`
     (parent = `FunctionNamespace` если один сегмент, иначе
     `FunctionFolder { path: parent }`).
   - Вызови новый engine-метод `rename_function_folder_as(&from, &to,
     self.actor.clone())`.
   - Верни `admin_result(mpack!({ "renamed_function_folder": …from…,
     "to": …to… }))` (List сегментов, как в `handle_create_function_folder`).

4. **Engine-метод** — `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs`
   (рядом с `create_function_folder_as` :377 и `rename_function_as` :274).
   `pub async fn rename_function_folder_as(&self, from: &[String], to: &[String],
   actor: Actor) -> DbResult<()>`:
   - **Folders — path-keyed записи** в system_store (`path_key = segments.join("/")`).
     API: `system_store.rs` — `load_function_folder(path_key)` :1035,
     `save_function_folder(path_key, record, meta)` :981,
     `remove_function_folder(path_key)` :1003, `load_function_folders()` :1020.
   - **Семантика rekey** (folders — независимое ACL-дерево; функции
     плоско-именованы в FunctionNamespace и НЕ адресуются через folder-путь —
     НЕ трогай функции):
     - `from_key = from.join("/")`, `to_key = to.join("/")`.
     - **Гарды** (typed error, НЕ оставляй dangling state): источник
       `load_function_folder(from_key)` должен существовать; назначение
       `load_function_folder(to_key)` должно отсутствовать.
     - **Rekey самого folder + всех потомков**: пройди `load_function_folders()`,
       отбери записи, чей `path` == `from_key` ЛИБО начинается с `from_key + "/"`
       (вложенные подпапки). Для каждой: вычисли новый ключ заменой префикса
       `from_key` → `to_key`, обнови поля `path` и `segments` в записи,
       `remove_function_folder(old)` + `save_function_folder(new, rec,
       existing_meta)` сохраняя `ResourceMeta::from_record(&rec)` (как
       `rename_function_as` сохраняет owner/group/mode :303-314).
     - Порядок: удаляй старые ключи и пиши новые так, чтобы не было коллизии
       (собери список миграций, потом примени). Назначение-гард уже гарантирует,
       что `to`-поддерево свободно.
   - Также добавь тонкий `pub async fn rename_function_folder(&self, from,
     to) -> DbResult<()>` → `rename_function_folder_as(.., Actor::System)`
     (зеркало `rename_function` :269).

5. **Rust-билдер** — `crates/shamir-query-builder/src/ddl/function.rs`
   (рядом с `rename_function` :112 и `create_function_folder` :120). Добавь
   `pub fn rename_function_folder(from: impl IntoIterator<Item = impl Into<String>>,
   to: impl IntoIterator<Item = impl Into<String>>) -> BatchOp` →
   `BatchOp::RenameFunctionFolder(RenameFunctionFolderOp { … })` (свободная
   функция, как `rename_function` / `create_function_folder`; импорт типа — в
   шапку файла). Сверь, что `ddl/mod.rs` ре-экспортит (если он ре-экспортит
   `create_function_folder`).

6. **TS-билдер** — `crates/shamir-client-ts/src/core/builders/ddl.ts`. Добавь
   `renameFunctionFolder(from: string[], to: string[])` по образцу соседнего
   `renameFunction` / `createFunctionFolder` (тип op — в
   `crates/shamir-client-ts/src/core/types/ddl.ts`, добавь
   `RenameFunctionFolderOp { rename_function_folder: string[]; to: string[] }`).
   Сверь экспорт через `index.ts`, если тип/функция должны быть публичны.

## Тесты (обязательно)
- **Rust e2e/unit** — рядом с тестами на `create_function_folder` /
  `rename_function` (найди их: `grep -rn "create_function_folder\|rename_function"
  crates/shamir-db/src --include=*.rs -l` и смотри `tests/`-каталоги). Покрой:
  (1) create folder `["a","b"]` → rename `["a","b"]`→`["a","c"]` → readback
  через `list_function_folders()`: `a/c` есть, `a/b` нет, `a` остался.
  (2) **nested rekey**: create `["a","b","c"]`, rename `["a","b"]`→`["a","x"]`
  → `a/x` и `a/x/c` есть, `a/b*` нет.
  (3) гарды: rename несуществующего → typed error; rename в занятый путь →
  typed error.
- **TS** — wire-shape unit в `builders/__tests__/ddl.test.ts` (по образцу
  `renameFunction`-теста): `renameFunctionFolder(['a','b'],['a','c'])` →
  `{ rename_function_folder:['a','b'], to:['a','c'] }`.

## Гейт (прогони сам, всё зелёное — НЕ коммить)
- Rust: `./scripts/test.sh -p shamir-query-types -p shamir-query-builder -p shamir-db`
  (lib). При e2e — `./scripts/test.sh @e2e -- function_folder` если есть. НЕ
  параллель rust `--full` с e2e (Windows file-lock на shamir-server.exe).
- `cargo fmt -p shamir-query-types -p shamir-query-builder -p shamir-db -- --check`
  + `cargo clippy -p shamir-query-types -p shamir-query-builder -p shamir-db
  --all-targets -- -D warnings`.
- TS: `cd crates/shamir-client-ts && npx vitest run ddl && npx tsc --noEmit`
  (tsc: не вноси НОВЫХ ошибок сверх pre-existing).

## ЖЁСТКИЕ ПРАВИЛА
- ⛔ НЕ используй инструмент agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER `git reset`/`checkout`/`clean`/`stash`/`restore`/`rm` или любую
  мутирующую git-команду. Только редактируй файлы; коммитит оркестратор.
- Surgical, аддитивно, по образцу соседей. one-file-one-export; импорты — в шапку.
  Запросы — только через билдер. Никакого raw `serde_json::Value`.
- Тесты — ТОЛЬКО через `./scripts/test.sh` (raw `cargo test` заблокирован).
- Заверши финальным текстом: список изменённых файлов с file:line + вывод
  прогонов гейта (Rust lib PASS, fmt/clippy чисто, vitest ddl PASS, tsc до/после).
