בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Слой: Права

## Предметная область (что реально гейтится сейчас)

Admin-DDL гейтится `authorize_access(&actor, &path, action)`
(`crates/shamir-db/src/shamir_db/execute/`). Точная картина по коду:

| Операция | ResourcePath | Action | Где |
|---|---|---|---|
| `drop_table` | `ResourcePath::table(...)` | `Delete` | `admin_table_index.rs:100` |
| alter table (retention/buffer) | `ResourcePath::table(...)` | `Write` | `admin_table_index.rs:136,261` |
| `bind_validator` к таблице | `ResourcePath::Table{…}` | `Write` | `admin_validator.rs:177` |
| `create_validator` (wasm/native) | `ResourcePath::FunctionNamespace` | `Create` | `admin_validator.rs:37` |
| **`create_table`** | — | — | **ГЕЙТА НЕТ** — `handle_create_table`→`add_table_as` не зовут `authorize_access` (`table_management.rs:34-75`); пишут только `ResourceMeta::owned_by(actor)` |

> **Важно (исправление):** у `create_table` сегодня **нет** authz-гейта (пред-существующий
> пробел, вне нашей фичи). Поэтому якорь схемо-прав — НЕ create_table, а **`Action::Write` на
> `ResourcePath::table`** (паттерн alter таблицы — реально существует).

## Прозрение: грань код-vs-данные — СТРУКТУРНАЯ

«Строго по-таблично» делает грань структурной, без нового grant-измерения:

```
Declarative-схема = свойство ТАБЛИЦЫ   → право = власть над таблицей (Write на table, как alter)
WASM / Native     = объект NAMESPACE    → право = FunctionNamespace::Create (деплой кода)
```

Это твоё «права получает тот, кто управляет таблицей». Реляц. аналогия: `ALTER TABLE` (схема
столбцов) ≠ `CREATE FUNCTION/TRIGGER` (код). Грань воплощена **местом** (схема в записи таблицы,
код — в namespace) — отдельный grant-флаг не нужен.

## Цели

- Управление declarative-схемой гейтится **`Write` на таблице**, не `FunctionNamespace`.
- Право на namespace-код НЕ даёт менять схему чужой таблицы (нужна Write на таблице).
- Declarative ничего не исполняет → table-Write безопасен; code — узкий namespace-Create.

## План реализации

1. **`SetTableSchemaOp` / `AddSchemaRuleOp` / `RemoveSchemaRuleOp`** (управление схемой) —
   `authorize_access(actor, ResourcePath::table(db,repo,name), Action::Write)`. Это основной,
   надёжно гейтящийся путь (паттерн alter, `admin_table_index.rs:136`).
2. **`CreateTableOp{schema}`** — схема в момент создания наследует гейт самого `create_table`.
   Поскольку у create_table гейта СЕЙЧАС НЕТ, нужно ОДНО из:
   - (рекоменд.) ввести предусловие `authorize_access(actor, ResourcePath::table, Action::Create)`
     в `add_table_as`/`handle_create_table` — отдельное hardening (закрывает и пробел
     create_table, и create-со-схемой); ИЛИ
   - не принимать схему в `create_table` вовсе — только через `set_table_schema` (Write-gated),
     тогда створка схемы всегда под Write.
   В доке — пометить как **новое требование к authz**, не «переиспользование существующего».
3. `meta.inject_into` на запись таблицы покрывает схему (owner/visibility — у таблицы).
4. `FunctionNamespace`-гейты wasm/native не трогаем.
5. Нового grant-измерения нет — грань структурна.

## Тесты

**Rust e2e:**
- actor с `Write` на таблице → `set_table_schema` ок; без власти над таблицей → `access_denied`
  (это надёжный Write-путь);
- actor с `FunctionNamespace`-правом (создаёт wasm/native) НЕ может менять схему чужой таблицы
  без table-Write → `access_denied` (грань держит);
- (если выбран hardening п.2) `create_table` без `Create` на таблице → `access_denied`; иначе
  тест на create-со-схемой-deny НЕ ставить (гейта нет — он стал бы ложно-зелёным).

**ts/js e2e:** клиент с table-Write меняет схему через `set_table_schema`; без неё — отказ.
