בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase E.5 (#245) — RETURNING-симметрия для INSERT/DELETE

Кампания **Phase E — Completeness & Operability**, Track B (OQL-surface).
Независима от DDL-трека. Близнец keyset (engine-ready surface).

## ⛔ Git-запрет (НАРУШЕНИЕ = катастрофа)
NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, или
любую git-команду, мутирующую рабочее дерево/индекс. Только редактируй файлы;
коммитит оркестратор. (2026-06-24 агент сделал `git reset --hard` и стёр часы.)

## Твой файл-сет (НЕ выходи за него — параллельно работают другие агенты)
- `crates/shamir-query-types/src/**/write/types.rs`, `write_result.rs`
- `crates/shamir-engine/**` — execute_delete_tx / execute_update_tx (read-back)
- `crates/shamir-query-builder/src/write/{delete,insert}.rs` (+ их tests)
- `crates/shamir-client-ts/src/core/builders/write.ts` + соответствующий тип
- тесты: `crates/shamir-db/tests/**` (integration) + TS write-builder unit
НЕ трогай: ddl-файлы (admin_table_index.rs, drop_*.rs, ddl.ts), e2e-harness,
__tests__/e2e — там другие агенты.

## Цель
Асимметрия returning (completeness-oql M7):
- UpdateOp имеет `select: Option<UpdateSelect>` (return_mode All/Changed/Unchanged
  + fields projection) — write/types.rs ~79-81.
- DeleteOp (types.rs ~99) НЕ имеет returning вообще.
- InsertOp (types.rs ~47) возвращает вставленные записи (write_result.rs ~12),
  но без fields-projection.

## Сделать
1. **DeleteOp**: добавить `#[serde(default)] select: Option<DeleteSelect>` (или
   переиспользовать структуру-проекцию) → вернуть удалённые записи с опц.
   проекцией полей. Движок при delete уже читает байты строк (execute_delete_tx)
   → returning дёшев. Изучи как UpdateSelect прокидывается в execute_update_tx и
   повтори для delete; результат — в WriteResult.records.
2. **InsertOp**: добавить опц. fields-projection (симметрия UpdateSelect.fields)
   для возвращаемых вставленных записей.
3. Билдеры: Rust `write/delete.rs` (`.returning()` / `.returning_fields()`),
   `write/insert.rs`; TS `builders/write.ts`. Прецедент: Update уже имеет
   `returning`/`returning_fields` в Rust билдере. Паритет Rust↔TS.
4. Тесты: integration — delete с returning возвращает удалённые строки; insert
   с projection возвращает только выбранные поля. + TS unit wire-shape.

## Дисциплина проекта (ОБЯЗАТЕЛЬНО)
- Запросы — ТОЛЬКО query-builder; `serde_json::Value` запрещён (док-исключения с
  однострочным комментом-почему).
- `#[serde(default)]` на новых полях — обратная совместимость wire.
- Тесты ТОЛЬКО через `./scripts/test.sh` (raw cargo test заблокирован
  perimeter-guard'ом; bash). Узко:
  `./scripts/test.sh -p shamir-db -p shamir-query-types -p shamir-query-builder`
  и `./scripts/test.sh -p shamir-db --full -- returning`.
  НЕ грепай вывод тестов inline — пиши в файл, грепай файл.
- Один файл = один основной export; импорты в шапке; mod.rs только реэкспорты.
- В тестах JSON-литералы многострочные и с отступами.

## Гейт перед сдачей (прогони сам)
```
cargo fmt -p <touched> -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -p shamir-query-types -p shamir-query-builder
```

## Что вернуть
(1) изменённые файлы; (2) контракт returning для delete+insert; (3) гейт с
числами PASS/FAIL; (4) отклонения. НЕ КОММИТЬ — коммитит оркестратор.
Заверши финальным assistant-сообщением с этим отчётом.
