בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase E.1 (#241) — `if_exists` на всех drop-ops (идемпотентность DDL)

Кампания **Phase E — Completeness & Operability**, Track A (DDL-lifecycle).
Stage 1 из 9. Strategy: single-context, sequential, commit-per-phase.

## ⛔ Git-запрет (НАРУШЕНИЕ = катастрофа)
NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, или
любую git-команду, мутирующую рабочее дерево или индекс. Только редактируй файлы;
коммитит оркестратор. (2026-06-24 агент сделал `git reset --hard` и стёр часы
работы — восстановление было возможно ТОЛЬКО потому что брифы и тесты были в git.)

## Цель
Единообразная идемпотентность дропов. Сейчас дропы НЕпоследовательны при
отсутствии объекта:
- `handle_drop_db` — no-op, returns `existed:false`.
- `handle_drop_index` — ОШИБКА если нет db/table-родителя.
- Нет флага `if_exists` ни на одном дропе (completeness-ddl G2).
- Create-ops уже имеют `if_not_exists` — зеркалим этот паттерн.

## Заземление (file:line, перепроверь перед правкой)
- DropTableOp — `crates/shamir-query-types/.../table_ops.rs:38` (примерно)
- DropIndexOp — `index_ops.rs:82`
- DropFunctionOp — `function_ops.rs:31`
- DropValidatorOp — `validator_ops.rs:32`
- DropUserOp / DropRoleOp / DropGroupOp — соответствующие *_ops.rs
- DropDbOp / DropRepoOp — уже no-op-on-absent (db_ops.rs / repo_ops.rs)
- Прецедент `if_not_exists` на create: db_ops.rs:17, table_ops.rs:22,
  index_ops.rs:74, repo_ops.rs:20; handler — `handle_create_table`
  (admin_table_index.rs ~37, читает `op.if_not_exists`).
- Handlers дропов: `crates/shamir-db/src/shamir_db/execute/`:
  - admin_db_repo.rs:50 (drop_db), admin_table_index.rs:92 (drop_table),
    admin_table_index.rs:306 (drop_index)
  - admin_function.rs:77, admin_validator.rs:78
  - admin_users_roles.rs:83/235, admin_access.rs:157
- Rust билдеры: `crates/shamir-query-builder/src/ddl/` (drop_table.rs / drop_index.rs / ...)
- TS билдеры: `crates/shamir-client-ts/src/builders/ddl.ts`

ВНИМАНИЕ: номера строк — ориентир, не догма. Открой файл, найди точное место.

## Сделать
1. Добавить `#[serde(default)] pub if_exists: bool` в DropTableOp, DropIndexOp,
   DropFunctionOp, DropValidatorOp, DropUserOp, DropRoleOp, DropGroupOp.
   DropDbOp/DropRepoOp — добавить флаг для симметрии (даже если уже no-op).
2. Семантика: `if_exists=true` + отсутствие объекта ИЛИ родителя → чистый no-op
   (`existed:false`), НЕ ошибка. Без флага — сохранить текущее поведение
   (ошибка либо existed:false как сейчас). Унифицировать handlers перечисленные
   выше под этот контракт.
3. Билдеры: Rust ddl (добавить `.if_exists()` к каждому drop-builder) + TS
   `builders/ddl.ts`. ПАРИТЕТ Rust↔TS обязателен.
4. Тесты:
   - Rust integration в `crates/shamir-db/tests/` (или существующий ddl-тест):
     drop отсутствующего объекта → no-op с if_exists; ошибка без флага.
   - TS unit wire-shape (ddl.test.ts) — поле `if_exists` сериализуется.

## Дисциплина проекта (ОБЯЗАТЕЛЬНО)
- Запросы строятся ТОЛЬКО через query-builder. `serde_json::Value` запрещён
  (исключения: napi/FFI boundary, serde round-trip тесты, WASM-bridge,
  protocol-spec доки — с однострочным комментарием-почему).
- `#[serde(default)]` на новых полях — иначе сломается десериализация старого
  wire-формата.
- Lock-free / async / Fx-hash идеология — но здесь в основном DTO + handler, без
  hot-path concurrency.
- Тесты ТОЛЬКО через `./scripts/test.sh` (raw `cargo test` заблокирован
  perimeter-guard'ом). Узкий прогон:
  `./scripts/test.sh -p shamir-db --full -- <filter>` и
  `./scripts/test.sh -p shamir-query-types`.
- НЕ грепай вывод тестов inline — пиши в файл, грепай файл.
- Один файл = один основной export. mod.rs — только реэкспорты.
- Импорты — в шапке файла.

## Гейт перед сдачей (прогони сам, приложи результат)
```
cargo fmt -p <touched-crates> -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -p shamir-query-types
```
Если clippy падает на ПРЕД-существующих линтах в нетронутом коде — не чини в этом
диффе, сообщи отдельно.

## Что вернуть оркестратору
Структурно: (1) список изменённых файлов; (2) краткое описание контракта
if_exists как реализовал; (3) результат гейта (fmt/clippy/test — вывод или
сводка PASS/FAIL с числами); (4) любые отклонения от брифа и почему. Финальный
текст агента — это и есть отчёт оркестратору (не сообщение пользователю).
НЕ КОММИТЬ — коммитит оркестратор после верификации.
