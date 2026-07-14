בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Слой: Хранение — в каталоговой записи таблицы (вариант A)

## Предметная область

Таблица имеет **каталоговую запись** — schema-less `QueryValue::Map`; пишется `save_table_meta`
(`system_store.rs:817`, произвольная Map целиком, durable-flush), читается все разом
`load_tables()` (`system_store.rs:399`) и по одной (single-table read ~`:740`). Declarative-схема
(вариант A) живёт **в этой записи** — НЕ в `system/validators`.

## Цели

- Схема — часть определения таблицы; durable без re-register.
- Стабильный, уникальный, переживающий reopen `RecordId` для схема-валидатора.
- Материализация — на том же жизненном цикле, что code-валидаторы (boot + DDL), не «per-resolve».

## Форма записи таблицы

```text
tables[ (repo, name) ] = Map {
  …existing config,
  "schema": List [ Map{ "path": List[Int…], "type": Str, "max": Int, "required": Bool, … } ],
  "schema_validator_id": Str(RecordId),     // персистентный id (генерится RecordId::new() при 1й компиляции)
  "schema_version": Int,                     // монотонный — optimistic-concurrency ALTER (02-…)
  + meta.inject_into → visibility/security/secret_grants (право — таблицы, 05-…)
}
```

- `schema[].path` — список целочисленных id (репо-интерн, `04-…`), на проводе/в каталоге как
  `List[Int(i64)]`; на validate/compile оборачиваются в `InternerKey::new(id)` и
  де-интернируются id→name на compile (`01-/04-…`).
- **`schema_validator_id`** — НЕ детерминированный-из-имени (`RecordId::system` режет до 12 байт →
  коллизия): генерится `RecordId::new()` при первой компиляции и **персистится здесь**;
  восстанавливается на reopen (как boot восстанавливает `_id` code-валидаторов из каталога,
  `core.rs:330-350`). Стабильно + уникально + durable.

## Материализация — compile-on-BOOT + compile-on-DDL (не «on-open»)

Точка «после `set_validator_registry`» НЕВЕРНА: `set_validator_registry` зовётся на **каждый**
`resolve`/`get_table` (`table_resolver.rs:23`, `table_management.rs:147`), повторный `register`
упадёт `AlreadyExists`. Реальный lifecycle — как у code-валидаторов:

1. **Boot-pass в `init()`** (после `load_validators()`): пройти `load_tables()`, для каждой
   записи со `schema`:
   ```rust
   let rules = parse_schema(rec.get("schema"), repo_interner)?;     // List[Int] → Vec<FieldRule(names)>
   let id = record_id_from(rec.get("schema_validator_id"));         // персистентный
   reg.register(id, schema_validator_name(repo, table), Arc::new(SchemaValidator::from_rules(rules)))?;
   table_mgr.add_validator_binding(ValidatorBinding{ validator_id:id, ops:ALL_WRITE_OPS, priority:500 });
   ```
2. **Инкрементально на DDL** (`handle_create_table`+schema, `set_table_schema`/add/remove):
   при первом задании схемы — сгенерировать `schema_validator_id`, записать в каталог,
   `register` + `add_validator_binding`; при ALTER — переписать `schema`, bump `schema_version`,
   `replace_artifact` под тем же id.

## ALTER — consistency-model + атомарность version

RCU-replace артефакта под тем же id; write-path читает `validator_bindings.load_full()` и
`reg.get_by_id` раздельно (оба атомарны) → in-flight запись видит **старый ИЛИ новый артефакт
целиком**, не рваное. **`schema_version` optimistic-concurrency:** `save_table_meta` пишет всю
Map (не CAS), поэтому DDL-схемо-операции **сериализуются per-table lock** (как `admin_user_locks`
RMW, `core.rs:418`): прочитать version под локом → сверить `expected_version` → записать → bump.
Без лока `expected_version` давал бы ложную защиту.

**Против существующих строк:** ALTER валидирует ТОЛЬКО последующие записи (реляц. ALTER без
`VALIDATE`).

## DROP таблицы

Снять авто-`ValidatorBinding` + `reg.remove(schema_validator_id)` (иначе утечка id/имени).

## Интроспекция

`get_table_schema(repo, table)` (`02-…`) — `schema` из записи таблицы, де-интерн id→name. НЕ
через `list_validators`. `ArtifactKind::Declarative` (`artifact_kind.rs:26` сейчас `Wasm|Native`)
— добавить вариант для пометки рантайм-binding (пункт плана Фазы A).

## План реализации

1. `ArtifactKind::Declarative` — вариант + ветки в `as_str` (`"declarative"`) И `parse_kind`
   (`"declarative" => Declarative`; иначе fail-safe в `Wasm` деградирует declarative-строку —
   `artifact_kind.rs:50`).
2. Запись `schema`/`schema_validator_id`/`schema_version` при create/alter (`02-…`), под
   per-table lock.
3. Boot-pass `load_tables()` в `init()` + DDL-инкремент → register + auto-binding (priority 500).
4. `parse_schema(List[Int], repo_interner) -> Vec<FieldRule(names)>`.
5. DROP: снять binding + `reg.remove(id)`.

## Тесты

**Unit:** `schema` List ↔ `Vec<FieldRule>`; де-интерн id→name; `schema_validator_id` persist/restore.

**Rust e2e** (durable Fjall, reopen-retry с `Locked`):
- create_table.schema → boot-/DDL-материализация → write валидируется;
- **reopen** → `schema_validator_id` восстановлен (тот же id, без коллизий), схема валидирует;
- `set_table_schema`/add/remove → RCU-replace, `schema_version` растёт; `expected_version`
  mismatch → `version_conflict` (под per-table lock — нет lost-update);
- ALTER не трогает старые строки; DROP → binding/артефакт убраны (нет утечки);
- таблица без схемы — пишет свободно.
