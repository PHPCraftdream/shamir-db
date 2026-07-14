בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Слой: Клиент — Rust / JS / TS билдеры

## Предметная область

Билдеры: **Rust** — `crates/shamir-query-builder` (`ddl/`); **TS/JS** — `shamir-client-ts`
поверх `shamir-client-node` (napi). Правило: всегда билдер, не raw `serde_json`. Клиент несёт
packing-слой с `interner_cache` (`04-…`), **per-repo**, интернирующий пути прозрачно.

Declarative-схема — свойство таблицы → билдер **табличный**.

## Цели

- Единый чистый API на всех языках: объявить/менять/читать схему таблицы плоскими именами +
  именованными ограничениями. Идентичная форма Rust ↔ TS/JS.
- Билдер слеп к интернированию (packing пакует id перед проводом, per-repo).
- Без бойлерплейта — fluent `field(path).type().constraint()`.

## Форма (зеркальная)

Rust:
```rust
create_table("users").in_repo("main").schema([
    field(["email"]).string().max(255).required(),
    field(["age"]).int().min(0).max(150),
]);
set_table_schema("main","users").rules([ … ]).expected_version(3);  // whole-replace
add_schema_rule("main","users").rule(field(["nickname"]).string().max(64));
remove_schema_rule("main","users").path(["nickname"]);
get_table_schema("main","users");        // → правила (имена), schema_version
list_validators();                       // глобальные code: name/kind/source/bound_in
```

TS/JS (зеркально, camelCase):
```ts
createTable("users").inRepo("main").schema([
  field(["email"]).string().max(255).required(),
  field(["age"]).int().min(0).max(150),
]);
setTableSchema("main","users").rules([ … ]).expectedVersion(3);
addSchemaRule("main","users").rule(field(["nickname"]).string().max(64));
removeSchemaRule("main","users").path(["nickname"]);
getTableSchema("main","users");
listValidators();
```

Обе формы строят table-scoped wire-ops с ПЛОСКИМИ путями; packing-слой подменяет пути на
интернированные id репо перед отправкой.

## План реализации

1. **Rust** (`ddl/`): `CreateTable::schema`, `set_table_schema().rules().expected_version()`,
   `add_schema_rule`, `remove_schema_rule`, `get_table_schema`, `list_validators`/`get_validator`
   + `field(path)` fluent (общий с embedded `01-…`).
2. **DTO** (`query-types`): `FieldRuleDto`/`ConstraintsDto` (napi/FFI — задокумент. исключение из
   builder-only: десериализация запроса).
3. **TS/JS** (`shamir-client-ts`): зеркальный fluent; типы тегов/ограничений типизированы
   (`string()` не даёт числового `.min`).
4. **Packing** (`shamir-client` interner_cache, per-repo): для schema-DDL — резолв путей → id;
   TS/JS через node-binding к тому же слою.

## Тесты

**ts/js e2e** (TS client suite, реальный сервер TLS+SCRAM):
- собрать схему на TS → `createTable().schema(...)` → невалидная запись отвергнута (правильные
  `field`/`code`), валидная принята; `setTableSchema`/`add`/`remove` меняют; `getTableSchema`
  возвращает имена (не id); `listValidators` — source для wasm-из-source;
- dev на TS интернирования не касается (имена; на проводе — id);
- mixed: declarative-схема + bound wasm на одной таблице.

**Rust client e2e** (`shamir-client`): тот же сценарий rust-билдером.

**Unit** (`query-builder` + ts): билдеры → корректные wire-ops; типобезопасность тегов;
`expected_version`.
