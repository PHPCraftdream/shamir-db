בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Слой: Тесты — unit + rust-e2e + ts/js-e2e

## Предметная область

Тесты — ТОЛЬКО через `./scripts/test.sh` (nextest, per-test timeout). Организация: `tests/` на
модуль, `mod.rs` — манифест; e2e — `crates/<crate>/tests/`. Durable reopen — lock-retry-helper
с `"Cannot acquire lock" | "already open" | "Locked"`. JSON-литералы — через билдер.

## Цели

Доказать корректность по-табличной вертикали + Phase 0 (by-name контракт) тремя уровнями.

## Пирамида

### Unit (по слоям)
- **Phase 0** (`08-…`): `ViewFields.get(["a","b"])` by-name ↔ `materialize_at`; `OwnedFields`
  строковый лукап; отсутствует → `None`; типизированные геттеры.
- **Движок** (`01-…`): каждый тег vs реальный `Value`-вариант (`Int`/`F64`/`Dec`/`Big`/`Bin`/
  `Bool`/`Str`/`Set`/`Null`/`List`/`Map`); `Int + unsigned` (≥0); каждое ограничение (границы);
  вложенный путь by-name; `required`/`nullable`; накопление ошибок; пустая схема.
- **Хранение** (`03-…`): `schema` List ↔ `Vec<FieldRule>`; де-интерн id→name; `schema_version`.
- **Интернирование** (`04-…`): `resolve_or_touch` хит/промах (per-repo); путь→id.
- **DDL/билдер** (`02-/06-…`): все ops → корректные wire-ops; `expected_version`; типобезоп. тегов.

### Rust e2e (`crates/shamir-db/tests/declarative_schema_e2e.rs`)
1. `create_table.schema` (или embedded `table().set_schema()`) → невалидная запись отвергнута
   (правильные `field`/`code`), валидная принята.
2. **Durable reopen** → схема в записи таблицы переживает, валидирует сразу (без re-register).
3. **Update-модель:** `set_table_schema` (whole-replace) меняет; `add_schema_rule`/
   `remove_schema_rule` — surgical; `set_table_schema([])` — clear; `expected_version` mismatch →
   `version_conflict`; RCU-replace артефакта.
4. **ALTER-семантика:** ужесточённое правило валидирует ТОЛЬКО новые записи, старые не трогаются.
5. **DROP таблицы:** binding/синтет.артефакт убраны; повторный create той же таблицы ок.
6. **Интернирование:** новое имя поля → туч против репо → каталог хранит id → compile-on-open
   де-интернирует → validate by-name матчит.
7. **Права:** актор с правом на таблицу задаёт схему — ок; без власти над таблицей —
   `access_denied`; `FunctionNamespace`-право не даёт менять чужую схему.
8. **Интроспекция:** `get_table_schema` возвращает имена; `list_validators` — `source` для
   wasm-из-source, `None` для native.
9. **Mixed:** declarative-схема + bound native + bound wasm на одной таблице — по `priority`,
   ошибки накапливаются, `stop` уважается.
10. Таблица БЕЗ схемы — пишет свободно (нет регресса).

### Phase 0 миграция (`08-…`)
- `native_parity_e2e`-валидаторы переписаны на by-name `RecordFields` — те же accept/reject
  (регресс-гард); DELETE-путь без полного `to_query_value`; `@server --full` зелёный.

### ts/js e2e (TS client suite, реальный сервер)
- `createTable().schema()` → reject/accept; `setTableSchema`/`add`/`remove`/clear;
  `getTableSchema` → имена; `listValidators` → source; dev не касается интернирования; mixed с
  wasm; клиент без власти над таблицей — отказ.

## Матрица покрытия (цель)

| Измерение | Кейсы |
|---|---|
| Phase 0 | ViewFields/OwnedFields by-name; миграция native без де-интерна |
| Типы | string,int(+unsigned),f64,dec,bool,bin (+list/map/set/null/any) |
| Ограничения | max,min,len,required,nullable,unsigned, конфликт len+min/max |
| Путь | плоский, вложенный, отсутствующий, Null |
| Хранение | create.schema→reopen (durable); schema_version |
| Update | whole-replace, add, remove, clear, expected_version conflict |
| ALTER | только новые строки; RCU-replace |
| DROP | binding/артефакт cleanup |
| Интерн | per-repo: новое/известное имя; embedded vs wire |
| Права | таблица allow/deny; namespace-право ≠ власть над схемой |
| Интроспекция | get_table_schema (имена); list_validators (source/kind) |
| Композиция | declarative × bound {native,wasm} по priority |
| Языки | Rust embedded, Rust client, TS/JS client |

## План реализации
1. Unit — с каждым слоем (TDD-red перед реализацией).
2. `declarative_schema_e2e.rs` — rust e2e (embedded + durable + update-модель + ALTER/DROP +
   authz + mixed).
3. Phase 0 миграционные тесты + регресс-гард паритета.
4. TS-suite — declarative-сценарии.
5. Гейт каждого слоя: `fmt --all --check` + `clippy --workspace --all-targets -D warnings` +
   `./scripts/test.sh` (+ `--full`).
