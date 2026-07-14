בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Слой: Интернирование — per-repo

## Предметная область

Имена полей интернируются в `u64`-id. Интернер **per-repo** (НЕ per-table):
`repo_instance.rs:73` — *«Per-repo string interner (Stage I — moved from per-table to
per-repo)»*; `interner_ops.rs` — интернер живёт на `RepoInstance`, маршрутизация по **имени
репо**. Append-only, монотонный, идемпотентный.

Рельсы клиент↔интернер уже есть:
- **`InternerTouchOp{ interner_touch: repo, names }`** — идемпотентно интернирует, возвращает
  id (ключ — имя **репо**).
- **`InternerDumpOp{ interner_dump: repo, since? }`** — инкрементный дамп id→name.
- **`crates/shamir-client/src/interner_cache.rs`** (+ ambient-sync) — клиентский name↔id кэш,
  **repo-scoped**.

## Цель (директива)

> Билдеры (Rust/JS/TS) **ничего не знают про интернирование**. Упаковка — клиентский слой. На
> провод и в каталог идут **интернированные id**.

И, после Phase 0 (`08-…`): **валидатор тоже не знает про интернирование** — читает поля
ПО ИМЕНИ через `RecordFields`. Связка:

```
билдер (имена) → клиент пакует id (interner_cache, per-repo) → провод/каталог несут id
   → compile-on-open ДЕ-ИНТЕРНИРУЕТ id→name ОДИН РАЗ → SchemaValidator с by-name правилами
   → validate-time: RecordFields.get(имя) (Phase 0), интерн скрыт
```

## Архитектура упаковки

```
┌─ Билдер ─────────────────────────────────────────────────────────┐
│  field(["address","zip"]).string().len(5)                          │  ← плоские ИМЕНА
└───────────────────────────────────────┬───────────────────────────┘
                                         │ FieldRuleDto{ path:["address","zip"], … }
┌─ Клиентский packing-слой (per-repo) ───▼───────────────────────────┐
│  interner_cache.resolve_or_touch(repo, names) → [37,80]            │  ← интернирует против РЕПО
└───────────────────────────────────────┬───────────────────────────┘
                                         │ CreateTableOp/SetTableSchemaOp{ schema:[{path:[37,80]}] }
┌─ Провод / каталог таблицы ─────────────▼───────────────────────────┐
│  path = [u64,u64] ИНТЕРНИРОВАН (id репо), лежит в записи таблицы.   │
└───────────────────────────────────────┬───────────────────────────┘
                                         │ compile-on-open
┌─ Движок ───────────────────────────────▼───────────────────────────┐
│  parse_schema де-интернирует id→name (ОДИН раз) → FieldRule(names). │
│  validate: RecordFields.get(name) — by-name, интерн скрыт (Phase 0).│
└────────────────────────────────────────────────────────────────────┘
```

Решения:
- **Интернирование на DDL-time (create_table/set_table_schema) — не на hot-path.** Round-trip
  `InternerTouchOp` для незнакомых имён допустим; поля, которых ещё нет в данных,
  интернируются заранее (идемпотентно).
- **Де-интерн id→name — ОДИН раз на compile-on-open** (а не на каждую запись). Валидатор далее
  by-name.
- **Embedded:** packing против локального интернера репо напрямую, без сети.
- **Ошибки** (`ValidationError.field: Vec<String>`) — имена, доступны сразу (правила by-name).

## Граница ответственности

| Кто | Знает про интернирование |
|---|---|
| Билдер (rust/js/ts) | **ничего** — только имена |
| Packing-слой (`interner_cache`, per-repo) | резолвит/туч'ит против интернера репо |
| Провод / каталог таблицы | несёт/хранит id |
| compile-on-open | де-интернирует id→name один раз |
| `SchemaValidator` / `RecordFields` (Phase 0) | by-name, интерн скрыт |

## План реализации

1. Packing-хук: перед отправкой `CreateTableOp{schema}`/`SetTableSchemaOp` прогнать пути через
   `resolve_or_touch(repo, names)` — **новый хелпер поверх `FieldMap`** (`missing_names` →
   `InternerTouchOp` → `insert_entry`; такого готового метода нет, собрать из блоков кэша).
2. Wire-форма `FieldRuleDto.path` — `Vec<u64>` (id репо). Билдер строит `Vec<String>`; packing
   подменяет.
3. Embedded — против локального интернера репо.
4. Сервер: путь приходит id; валидирует существование id в интернере репо (иначе
   `unknown_field_id`); кладёт в запись таблицы.
5. compile-on-open: `parse_schema` де-интернирует id→name (`03-…`).

## Тесты

**Unit** (`interner_cache_tests` + packing): хит без round-trip; промах → touch → кэш пополнен;
плоский/вложенный путь → id-последовательности (per-repo).

**Rust e2e:** create_table.schema с новым именем → клиент туч'ит против репо → каталог таблицы
хранит id → compile-on-open де-интернирует → validate by-name матчит; embedded — против
локального интернера репо, без сети.

**ts/js e2e** (`06-…`): билдер именами → клиент-пакер интернирует (per-repo) → провод id; dev
интернирования не касается.
